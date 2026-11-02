//! Turns fuzzer bytes into a [`Program`].
//!
//! This is a hand-written `Arbitrary` impl rather than `#[derive(Arbitrary)]` on
//! the recursive types, because generation has to carry context:
//! - **AST depth.** Monty rejects source nested deeper than 30 levels in debug
//!   builds (`MAX_NESTING_DEPTH` in `crates/monty/src/parse.rs`). [`Scope::depth`]
//!   mirrors Monty's count (one level per statement and expression node, plus
//!   extra for `elif`, multi-item `with`, `and`/`or` chains and comparison chains).
//! - **Legality.** `break`/`continue` only in loops, `return` only in functions,
//!   `await` only in `async def`, no walrus in comprehensions, lambdas or class
//!   bodies, `nonlocal` only for an enclosing function's parameters.
//! - **Names in scope.** Parameters, lambda and comprehension variables.
//!
//! Every choice is arranged so that exhausted input picks the simplest option
//! (index 0, `false`, the minimum count). Generation therefore always terminates,
//! and short inputs give small programs. Global budgets ([`STMT_BUDGET`],
//! [`EXPR_BUDGET`]) cap program size however long the input is.

use std::mem;

use arbitrary::{Arbitrary, Result, Unstructured};

use crate::{
    ast::{
        Arg, BinOp, BoolOp, ClassDef, CmpOp, CompKind, Comprehension, Dataclass, DictItem, Elt, Expr, FPart,
        FunctionDef, Handler, Index, MatchCase, Method, Name, Params, Pattern, Program, SpecPart, Stmt, Target,
        UnaryOp,
    },
    catalog::{self, FAMILIES, FAMILY_WEIGHTS, Family, Func, MODULES, PatternKey},
};

/// Highest AST depth the generator uses; Monty's debug-build limit is 30, the rest is margin.
const MAX_DEPTH: u16 = 27;
/// Most top-level statements in a program.
const MAX_TOP_LEVEL: u32 = 10;
/// Most statements in one block.
const MAX_BLOCK: u32 = 4;
/// Most statements in a whole program.
const STMT_BUDGET: u32 = 32;
/// Most compound expressions in a whole program (leaves are free).
const EXPR_BUDGET: u32 = 240;
/// Most nested compound expressions, which keeps single expressions readable.
const MAX_EXPR_NEST: u8 = 5;
/// Most elements in a collection literal.
const MAX_ELEMS: u32 = 4;
/// Most nested `eval`/`exec` strings.
const MAX_EVAL_NEST: u8 = 2;

impl<'a> Arbitrary<'a> for Program {
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        Gen::new(u).program()
    }
}

/// Generation state shared by the whole program.
struct Gen<'a, 'u> {
    u: &'u mut Unstructured<'a>,
    stmts_left: u32,
    exprs_left: u32,
    next_guard: u16,
}

/// Where in the program a node is being generated; cloned (and adjusted) for children.
#[derive(Clone, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent context flags, not a state machine"
)]
struct Scope {
    /// AST levels used by the ancestors of the node being generated.
    depth: u16,
    /// Compound expressions above this one.
    nest: u8,
    in_loop: bool,
    in_func: bool,
    in_async: bool,
    in_finally: bool,
    in_comp: bool,
    in_lambda: bool,
    in_class: bool,
    /// Set for parameter defaults and decorators, where a walrus is pointless and risky.
    no_walrus: bool,
    /// 0 at module level, 1 in a function or method, 2 in a function nested in one.
    func_level: u8,
    /// `eval`/`exec` strings enclosing this node. Each level escapes the inner source
    /// again (backslashes double), so this is capped at [`MAX_EVAL_NEST`].
    eval_nest: u8,
    /// Readable names besides the globals: parameters, `self`, lambda and comprehension variables.
    locals: Vec<Name>,
    /// Parameters of the innermost function, the candidates for `nonlocal` in a nested function.
    func_params: Vec<Name>,
}

impl Scope {
    /// The scope of a top-level statement: inside the printer's `try` wrapper (one level).
    fn top_level() -> Self {
        Self {
            depth: 1,
            ..Self::default()
        }
    }

    /// The same context, `levels` deeper in the AST.
    fn down(&self, levels: u16) -> Self {
        let mut scope = self.clone();
        scope.depth += levels;
        scope
    }

    /// Like [`Scope::down`], for the children of a compound expression.
    fn sub(&self, levels: u16) -> Self {
        let mut scope = self.down(levels);
        scope.nest += 1;
        scope
    }

    /// Whether `levels` more AST levels still fit under [`MAX_DEPTH`].
    fn has_room(&self, levels: u16) -> bool {
        self.depth + levels <= MAX_DEPTH
    }

    /// Whether an assignment expression is legal (and useful) here.
    fn walrus_ok(&self) -> bool {
        !(self.in_comp || self.in_lambda || self.in_class || self.no_walrus)
    }

    /// Whether `await` is legal here: in `async def`, outside comprehensions and lambdas.
    fn await_ok(&self) -> bool {
        self.in_async && !self.in_comp && !self.in_lambda
    }
}

/// Operand types for [`Gen::binop`] and comparisons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Int,
    Float,
    Str,
    Bytes,
    List,
    Tuple,
    Set,
}

/// A small count as AST levels (counts here never exceed a handful).
fn levels(count: u32) -> u16 {
    u16::try_from(count).unwrap_or(u16::MAX)
}

/// A length as a `count` bound.
fn len_u32(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

/// `weight` if `enabled`, else 0 (disabling that choice).
fn on(enabled: bool, weight: u32) -> u32 {
    if enabled { weight } else { 0 }
}

/// Shorthand for a call expression.
fn call(func: Expr, args: Vec<Arg>) -> Expr {
    Expr::Call {
        func: Box::new(func),
        args,
    }
}

/// Shorthand for `module.name(args)`.
fn mod_call(module: Name, name: Name, args: Vec<Arg>) -> Expr {
    call(Expr::ModAttr(module, name), args)
}

impl<'a, 'u> Gen<'a, 'u> {
    /// Starts generation with full statement and expression budgets.
    fn new(u: &'u mut Unstructured<'a>) -> Self {
        Self {
            u,
            stmts_left: STMT_BUDGET,
            exprs_left: EXPR_BUDGET,
            next_guard: 0,
        }
    }

    /// Generates the top-level statements.
    fn program(mut self) -> Result<Program> {
        let scope = Scope::top_level();
        let count = self.count(1, MAX_TOP_LEVEL)?;
        let mut body = Vec::new();
        for _ in 0..count {
            if self.stmts_left == 0 || self.u.is_empty() {
                break;
            }
            body.push(self.stmt(&scope)?);
        }
        Ok(Program { body })
    }

    // === Statements ===

    /// One statement, chosen among those legal and affordable in `s`.
    fn stmt(&mut self, s: &Scope) -> Result<Stmt> {
        self.stmts_left = self.stmts_left.saturating_sub(1);
        let compound = s.has_room(8) && self.stmts_left > 0;
        let weights = [
            10,                                   // expression statement
            20,                                   // assignment
            6,                                    // augmented assignment
            on(compound, 8),                      // if
            on(compound, 7),                      // for
            on(compound, 3),                      // while
            on(compound, 5),                      // try
            2,                                    // raise
            2,                                    // assert
            on(compound && s.func_level < 2, 5),  // def
            on(compound && s.func_level == 0, 3), // class
            on(compound, 3),                      // with
            on(compound, 4),                      // match
            1,                                    // import
            on(s.in_func && !s.in_finally, 4),    // return
            on(s.in_loop && !s.in_finally, 3),    // break / continue
            1,                                    // pass
        ];
        let e = s.down(1);
        Ok(match self.pick(&weights)? {
            0 => Stmt::Expr(self.expr(&e)?),
            1 => Stmt::Assign {
                target: self.target(&e)?,
                value: self.expr(&e)?,
            },
            2 => self.aug_assign(&e)?,
            3 => self.if_stmt(s)?,
            4 => self.for_stmt(s)?,
            5 => self.while_stmt(s)?,
            6 => self.try_stmt(s)?,
            7 => self.raise_stmt(&e)?,
            8 => Stmt::Assert {
                test: self.expr(&e)?,
                msg: self.maybe(30, |g| g.expr(&e))?,
            },
            9 => self.function_def(s)?,
            10 => self.class_def(s)?,
            11 => self.with_stmt(s)?,
            12 => self.match_stmt(s)?,
            13 => self.import_stmt()?,
            14 => Stmt::Return(self.maybe(80, |g| g.expr(&e))?),
            15 => {
                if self.flag(50)? {
                    Stmt::Break
                } else {
                    Stmt::Continue
                }
            }
            _ => Stmt::Pass,
        })
    }

    /// A block of 1..=[`MAX_BLOCK`] statements (fewer once the statement budget runs out).
    fn block(&mut self, s: &Scope) -> Result<Vec<Stmt>> {
        let count = self.count(1, MAX_BLOCK)?;
        let mut body = Vec::new();
        for _ in 0..count {
            if self.stmts_left == 0 {
                break;
            }
            body.push(self.stmt(s)?);
        }
        Ok(body)
    }

    /// `target op= value` on a name, attribute or item.
    fn aug_assign(&mut self, s: &Scope) -> Result<Stmt> {
        let target = match self.pick(&[7, 1, 2])? {
            0 => Target::Name(self.assignable_name(s)?),
            1 => Target::Attr(self.attr_owner(s)?, self.choose(catalog::ATTRS)?),
            _ => Target::Subscript(self.item_owner(s)?, self.index(&s.sub(1))?),
        };
        let op = match BinOp::arbitrary(self.u)? {
            BinOp::MatMult => BinOp::Add, // Monty rejects `@=` at compile time
            op => op,
        };
        Ok(Stmt::AugAssign {
            target,
            op,
            value: self.expr(s)?,
        })
    }

    /// `if` with up to two `elif`s; each `elif` costs one extra AST level (Monty nests them).
    fn if_stmt(&mut self, s: &Scope) -> Result<Stmt> {
        let inner = s.down(1);
        let test = self.expr(&inner)?;
        let body = self.block(&inner)?;
        let elif_count = self.count(0, 2)?;
        let mut elifs = Vec::new();
        for level in 1..=elif_count {
            let elif_scope = s.down(1 + levels(level));
            elifs.push((self.expr(&elif_scope)?, self.block(&elif_scope)?));
        }
        let orelse = if self.flag(40)? {
            self.block(&s.down(1 + levels(elif_count)))?
        } else {
            vec![]
        };
        Ok(Stmt::If {
            test,
            body,
            elifs,
            orelse,
        })
    }

    /// A `for` loop over a value that is usually iterable.
    fn for_stmt(&mut self, s: &Scope) -> Result<Stmt> {
        let target = self.loop_target()?;
        let iter = self.iterable(&s.down(1))?;
        let mut inner = s.down(1);
        inner.in_loop = true;
        let body = self.block(&inner)?;
        let orelse = if self.flag(15)? {
            self.block(&s.down(1))?
        } else {
            vec![]
        };
        Ok(Stmt::For {
            target,
            iter,
            body,
            orelse,
            guard: self.guard(),
        })
    }

    /// A `while` loop; the printer's guard stops it after a few iterations.
    fn while_stmt(&mut self, s: &Scope) -> Result<Stmt> {
        let test = self.expr(&s.down(1))?;
        let mut inner = s.down(1);
        inner.in_loop = true;
        let body = self.block(&inner)?;
        let orelse = if self.flag(10)? {
            self.block(&s.down(1))?
        } else {
            vec![]
        };
        Ok(Stmt::While {
            test,
            body,
            orelse,
            guard: self.guard(),
        })
    }

    /// `try` with 0..=2 handlers (a `finally` is forced when there are none, as Python requires).
    fn try_stmt(&mut self, s: &Scope) -> Result<Stmt> {
        let inner = s.down(1);
        let body = self.block(&inner)?;
        let handler_count = self.count(0, 2)?;
        let mut handlers = Vec::new();
        for i in 0..handler_count {
            let bare = i + 1 == handler_count && self.flag(10)?; // bare `except:` must be last
            let types = if bare { vec![] } else { self.exception_types()? };
            let name = if !bare && self.flag(40)? {
                Some(self.choose(catalog::VARS)?)
            } else {
                None
            };
            let body = self.block(&s.down(2))?;
            handlers.push(Handler { types, name, body });
        }
        let orelse = if handler_count > 0 && self.flag(25)? {
            self.block(&inner)?
        } else {
            vec![]
        };
        let finalbody = if handler_count == 0 || self.flag(30)? {
            let mut fin = inner.clone();
            fin.in_finally = true; // no break/continue/return in `finally` (PEP 765)
            self.block(&fin)?
        } else {
            vec![]
        };
        Ok(Stmt::Try {
            body,
            handlers,
            orelse,
            finalbody,
        })
    }

    /// `raise`, bare or with an exception (optionally `from` a cause).
    fn raise_stmt(&mut self, s: &Scope) -> Result<Stmt> {
        let exc = match self.pick(&[1, 5, 3, 2])? {
            0 => None, // bare `raise`
            1 => {
                let args = if self.flag(60)? {
                    vec![Arg::Pos(Expr::Str(self.str_lit()?))]
                } else {
                    vec![]
                };
                Some(call(Expr::Name(self.choose(catalog::RAISABLE)?), args))
            }
            2 => Some(Expr::Name(self.choose(catalog::RAISABLE)?)),
            _ => Some(self.expr(s)?),
        };
        let cause = if exc.is_some() && self.flag(20)? {
            Some(if self.flag(30)? { Expr::None } else { self.expr(s)? })
        } else {
            None
        };
        Ok(Stmt::Raise { exc, cause })
    }

    /// A function (async 15% of the time), usually followed by a call to it.
    fn function_def(&mut self, s: &Scope) -> Result<Stmt> {
        let level = s.func_level + 1;
        let name = self.choose(catalog::FUNCS)?;
        let is_async = self.flag(15)?;
        let decorators = if self.flag(15)? {
            vec![self.decorator()?]
        } else {
            vec![]
        };
        let params = if level == 1 {
            self.params(s, catalog::PARAMS, "pa", "pk")?
        } else {
            self.params(s, catalog::NESTED_PARAMS, "qa", "qk")?
        };
        let param_names = params.names();
        let mut body_scope = Scope {
            depth: s.depth + 2,
            in_func: true,
            in_async: is_async,
            func_level: level,
            locals: s.locals.clone(),
            func_params: param_names.clone(),
            eval_nest: s.eval_nest,
            ..Scope::default()
        };
        body_scope.locals.extend(&param_names);

        let mut body = Vec::new();
        if self.flag(15)? {
            body.push(Stmt::Global(self.distinct_names(catalog::VARS, 2)?));
        } else if !s.func_params.is_empty() && self.flag(20)? {
            // Legal because nested functions use q-names, never the outer p-names.
            body.push(Stmt::Nonlocal(self.distinct_names(&s.func_params, 1)?));
        }
        body.extend(self.block(&body_scope)?);
        if self.flag(50)? {
            body.push(Stmt::Return(Some(self.expr(&body_scope.down(1))?)));
        }
        let call = if self.flag(70)? {
            Some(self.user_args(&s.down(2), &param_names)?)
        } else {
            None
        };
        Ok(Stmt::FunctionDef(Box::new(FunctionDef {
            name,
            is_async,
            decorators,
            params,
            body,
            call,
        })))
    }

    /// Positional parameters from `names` (defaults on a suffix), maybe `*vararg` and `**kwarg`.
    fn params(&mut self, s: &Scope, names: &'static [Name], vararg: Name, kwarg: Name) -> Result<Params> {
        let count = self.count(0, len_u32(names.len()))? as usize;
        let defaults = self.count(0, len_u32(count))? as usize;
        let mut default_scope = s.down(2);
        default_scope.no_walrus = true;
        let mut positional = Vec::new();
        for (i, name) in names[..count].iter().enumerate() {
            let default = if i >= count - defaults {
                Some(self.expr(&default_scope)?)
            } else {
                None
            };
            positional.push((*name, default));
        }
        Ok(Params {
            positional,
            vararg: if self.flag(15)? { Some(vararg) } else { None },
            kwarg: if self.flag(10)? { Some(kwarg) } else { None },
        })
    }

    /// A function decorator: a `functools` cache or a user function.
    fn decorator(&mut self) -> Result<Expr> {
        Ok(match self.pick(&[3, 2, 2, 1])? {
            0 => Expr::ModAttr("functools", "cache"),
            1 => Expr::ModAttr("functools", "lru_cache"),
            2 => {
                let size = Expr::Int(self.choose(catalog::SMALL_INTS)?);
                mod_call("functools", "lru_cache", vec![Arg::Kw("maxsize", size)])
            }
            _ => Expr::Name(self.choose(catalog::FUNCS)?),
        })
    }

    /// A class with fields and methods (maybe a dataclass), usually instantiated right after.
    fn class_def(&mut self, s: &Scope) -> Result<Stmt> {
        let name = self.choose(catalog::CLASSES)?;
        let dataclass = if self.flag(25)? {
            Some(Dataclass {
                frozen: self.flag(30)?,
                eq: self.maybe(20, |g| g.flag(50))?,
            })
        } else {
            None
        };
        let field_scope = Scope {
            depth: s.depth + 2,
            in_class: true,
            eval_nest: s.eval_nest,
            ..Scope::default()
        };
        let mut fields = Vec::new();
        for attr in &catalog::ATTRS[..self.count(0, 2)? as usize] {
            let annotation = if dataclass.is_some() || self.flag(10)? {
                Some(self.choose(catalog::ANNOTATIONS)?)
            } else {
                None
            };
            let value = if annotation.is_none() || self.flag(60)? {
                Some(self.expr(&field_scope)?)
            } else {
                None
            };
            fields.push((*attr, annotation, value));
        }

        let mut methods: Vec<Method> = Vec::new();
        for _ in 0..self.count(1, 4)? {
            let (method_name, arity) = self.choose(catalog::CLASS_METHODS)?;
            if methods.iter().any(|m| m.name == method_name) {
                continue;
            }
            methods.push(self.method(s, method_name, arity)?);
        }
        let instantiate = if self.flag(70)? {
            let target = self.choose(catalog::VARS)?;
            Some((target, self.user_args(&s.down(2), catalog::ATTRS)?))
        } else {
            None
        };
        Ok(Stmt::ClassDef(Box::new(ClassDef {
            name,
            dataclass,
            fields,
            methods,
            instantiate,
        })))
    }

    /// One method. `__init__` usually starts by setting the instance attributes.
    fn method(&mut self, s: &Scope, name: Name, arity: Option<u8>) -> Result<Method> {
        let params = match arity {
            Some(n) => Params {
                positional: catalog::PARAMS[..usize::from(n)].iter().map(|p| (*p, None)).collect(),
                ..Params::default()
            },
            None => self.params(s, catalog::PARAMS, "pa", "pk")?,
        };
        let mut locals = vec!["self"];
        locals.extend(params.names());
        let body_scope = Scope {
            depth: s.depth + 3,
            in_func: true,
            func_level: 1,
            func_params: locals.clone(),
            locals,
            eval_nest: s.eval_nest,
            ..Scope::default()
        };
        let mut body = Vec::new();
        if name == "__init__" {
            for attr in &catalog::ATTRS[..self.count(0, 2)? as usize] {
                body.push(Stmt::Assign {
                    target: Target::Attr(Expr::Name("self"), attr),
                    value: self.expr(&body_scope.down(1))?,
                });
            }
            if self.flag(40)? {
                body.extend(self.block(&body_scope)?);
            }
        } else {
            if self.flag(40)? {
                body.extend(self.block(&body_scope)?);
            }
            body.push(Stmt::Return(Some(self.expr(&body_scope.down(1))?)));
        }
        Ok(Method { name, params, body })
    }

    /// `with` over 1..=2 context managers; each extra item costs one AST level.
    fn with_stmt(&mut self, s: &Scope) -> Result<Stmt> {
        let count = self.count(1, 2)?;
        let item_scope = s.down(levels(count));
        let mut items = Vec::new();
        for _ in 0..count {
            let context = if self.flag(60)? {
                let args = self.positional_args(&item_scope.sub(1), 0, 2)?;
                call(Expr::Name(self.choose(catalog::CLASSES)?), args)
            } else {
                self.expr(&item_scope)?
            };
            let target = if self.flag(40)? {
                Some(self.choose(catalog::VARS)?)
            } else {
                None
            };
            items.push((context, target));
        }
        Ok(Stmt::With {
            items,
            body: self.block(&item_scope)?,
        })
    }

    /// `match` with 1..=3 cases; only the last may be irrefutable (Python rejects it elsewhere).
    fn match_stmt(&mut self, s: &Scope) -> Result<Stmt> {
        let subject = self.expr(&s.down(1))?;
        let case_count = self.count(1, 3)?;
        let case_scope = s.down(2);
        let mut cases = Vec::new();
        for i in 0..case_count {
            let refutable = i + 1 < case_count || !self.flag(30)?;
            let mut bound = Vec::new();
            let pattern = self.pattern(&case_scope, refutable, &mut bound, 0)?;
            let guard = self.maybe(20, |g| g.expr(&case_scope))?;
            let body = self.block(&case_scope)?;
            cases.push(MatchCase { pattern, guard, body });
        }
        Ok(Stmt::Match { subject, cases })
    }

    /// `import m [as v]` or `from m import name [as v]` for a bundled module.
    fn import_stmt(&mut self) -> Result<Stmt> {
        let module = self.choose_module(false)?;
        Ok(match self.pick(&[1, 1, 2])? {
            0 => Stmt::Import {
                module: module.name,
                alias: None,
            },
            1 => Stmt::Import {
                module: module.name,
                alias: Some(self.choose(catalog::VARS)?),
            },
            _ => {
                let names: Vec<Name> = module
                    .funcs
                    .iter()
                    .map(|f| f.name)
                    .chain(module.consts.iter().copied())
                    .filter(|n| !n.contains('.'))
                    .collect();
                Stmt::ImportFrom {
                    module: module.name,
                    name: self.choose(&names)?,
                    alias: if self.flag(70)? {
                        Some(self.choose(catalog::VARS)?)
                    } else {
                        None
                    },
                }
            }
        })
    }

    // === Patterns ===

    /// A pattern; `bound` collects capture names so each is bound once per pattern.
    fn pattern(&mut self, s: &Scope, refutable: bool, bound: &mut Vec<Name>, nest: u8) -> Result<Pattern> {
        let deeper = nest < 2 && s.has_room(3);
        let weights = [
            5,                 // literal
            on(!refutable, 3), // capture
            on(!refutable, 2), // wildcard
            on(deeper, 3),     // sequence
            on(deeper, 2),     // mapping
            on(deeper, 3),     // class
            2,                 // or of literals
            on(deeper, 1),     // as
        ];
        let child = s.down(1);
        Ok(match self.pick(&weights)? {
            0 => Pattern::Value(self.pattern_literal()?),
            1 => self.fresh_capture(bound)?.map_or(Pattern::Wildcard, Pattern::Capture),
            2 => Pattern::Wildcard,
            3 => {
                let mut items = Vec::new();
                for _ in 0..self.count(0, 3)? {
                    items.push(self.pattern(&child, false, bound, nest + 1)?);
                }
                let star = if self.flag(30)? {
                    let position = self.count(0, len_u32(items.len()))? as usize;
                    Some((position, self.fresh_capture(bound)?))
                } else {
                    None
                };
                Pattern::Sequence(items, star)
            }
            4 => {
                let mut pairs = Vec::new();
                let mut used = Vec::new();
                for _ in 0..self.count(0, 2)? {
                    let key = self.choose(catalog::PATTERN_KEYS)?;
                    if used.contains(&key) {
                        continue; // duplicate keys are a SyntaxError
                    }
                    used.push(key);
                    let key = match key {
                        PatternKey::Int(i) => Expr::Int(i),
                        PatternKey::Str(text) => Expr::Str(text.to_owned()),
                    };
                    pairs.push((key, self.pattern(&child, false, bound, nest + 1)?));
                }
                let rest = if self.flag(20)? {
                    self.fresh_capture(bound)?
                } else {
                    None
                };
                Pattern::Mapping(pairs, rest)
            }
            5 => {
                if self.flag(60)? {
                    let cls = self.choose(catalog::PATTERN_CLASSES)?;
                    let args = if self.flag(50)? {
                        vec![self.pattern(&child, false, bound, nest + 1)?]
                    } else {
                        vec![]
                    };
                    Pattern::Class {
                        cls,
                        args,
                        kwargs: vec![],
                    }
                } else {
                    let cls = self.choose(catalog::CLASSES)?;
                    let mut kwargs = Vec::new();
                    for attr in catalog::ATTRS {
                        if self.flag(40)? {
                            kwargs.push((*attr, self.pattern(&child, false, bound, nest + 1)?));
                        }
                    }
                    Pattern::Class {
                        cls,
                        args: vec![],
                        kwargs,
                    }
                }
            }
            6 => {
                let mut alternatives = Vec::new();
                for _ in 0..self.count(2, 3)? {
                    alternatives.push(Pattern::Value(self.pattern_literal()?));
                }
                Pattern::Or(alternatives)
            }
            _ => {
                let inner = self.pattern(&child, refutable, bound, nest + 1)?;
                match self.fresh_capture(bound)? {
                    Some(name) => Pattern::As(Box::new(inner), name),
                    None => inner,
                }
            }
        })
    }

    /// A literal allowed in patterns (signed numbers print without parentheses there).
    fn pattern_literal(&mut self) -> Result<Expr> {
        Ok(match self.pick(&[4, 3, 1, 1, 1])? {
            0 => Expr::Int(self.choose(catalog::SMALL_INTS)?),
            1 => Expr::Str(self.choose(&["", "a", "ab", "x", "é"])?.to_owned()),
            2 => Expr::None,
            3 => Expr::Bool(self.flag(50)?),
            _ => Expr::Bytes(self.choose(catalog::BYTES)?.to_vec()),
        })
    }

    /// A global name not yet bound in the current pattern, if any is left.
    fn fresh_capture(&mut self, bound: &mut Vec<Name>) -> Result<Option<Name>> {
        let free: Vec<Name> = catalog::VARS.iter().copied().filter(|v| !bound.contains(v)).collect();
        if free.is_empty() {
            return Ok(None);
        }
        let name = self.choose(&free)?;
        bound.push(name);
        Ok(Some(name))
    }

    // === Targets and names ===

    /// An assignment target: a name, attribute, item or unpacking tuple.
    fn target(&mut self, s: &Scope) -> Result<Target> {
        Ok(match self.pick(&[14, 2, 4, 3])? {
            0 => Target::Name(self.assignable_name(s)?),
            1 => Target::Attr(self.attr_owner(s)?, self.choose(catalog::ATTRS)?),
            2 => Target::Subscript(self.item_owner(s)?, self.index(&s.sub(1))?),
            _ => self.tuple_target()?,
        })
    }

    /// `v0, *v1`-style unpacking target: 2..=3 distinct globals, at most one starred.
    fn tuple_target(&mut self) -> Result<Target> {
        let count = self.count(2, 3)?;
        let names = self.distinct_names(catalog::VARS, count)?;
        let star = if names.len() >= 2 && self.flag(30)? {
            Some(self.count(0, len_u32(names.len()) - 1)? as usize)
        } else {
            None
        };
        Ok(Target::Tuple(names, star))
    }

    /// A `for` target: one global or a pair of them.
    fn loop_target(&mut self) -> Result<Target> {
        Ok(if self.flag(20)? {
            Target::Tuple(self.distinct_names(catalog::VARS, 2)?, None)
        } else {
            Target::Name(self.choose(catalog::VARS)?)
        })
    }

    /// The object whose attribute is assigned: `self` in methods, else usually the
    /// instance variable `v6`, sometimes any expression.
    fn attr_owner(&mut self, s: &Scope) -> Result<Expr> {
        Ok(if s.locals.contains(&"self") && self.flag(60)? {
            Expr::Name("self")
        } else if self.flag(60)? {
            Expr::Name("v6")
        } else {
            self.expr(&s.sub(1))?
        })
    }

    /// The object of a subscript assignment: usually the list or dict variable.
    fn item_owner(&mut self, s: &Scope) -> Result<Expr> {
        Ok(if self.flag(60)? {
            Expr::Name(self.choose(&["v2", "v3"])?)
        } else {
            self.expr(&s.sub(1))?
        })
    }

    /// A name that may be assigned here: a global, or a parameter of the current function.
    fn assignable_name(&mut self, s: &Scope) -> Result<Name> {
        let params = s
            .func_params
            .iter()
            .copied()
            .filter(|p| *p != "self")
            .collect::<Vec<_>>();
        if !params.is_empty() && self.flag(25)? {
            self.choose(&params)
        } else {
            self.choose(catalog::VARS)
        }
    }

    /// A name that can be read here.
    fn readable_name(&mut self, s: &Scope) -> Result<Name> {
        let local_weight = len_u32(s.locals.len()) * 3;
        Ok(match self.pick(&[16, local_weight, 2])? {
            0 => self.choose(catalog::VARS)?,
            1 => self.choose(&s.locals)?,
            _ => self.choose(catalog::USER_CALLABLES)?,
        })
    }

    /// Up to `count` distinct names from `pool` (at least one).
    fn distinct_names(&mut self, pool: &[Name], count: u32) -> Result<Vec<Name>> {
        let mut names = Vec::new();
        for _ in 0..count.max(1) {
            let name = self.choose(pool)?;
            if !names.contains(&name) {
                names.push(name);
            }
        }
        Ok(names)
    }

    /// One or two exception names for an `except` clause.
    fn exception_types(&mut self) -> Result<Vec<Name>> {
        let count = self.count(1, 2)?;
        self.distinct_names(catalog::CATCHABLE, count)
    }

    // === Expressions ===

    /// An expression; leaves become more likely the deeper the nesting.
    fn expr(&mut self, s: &Scope) -> Result<Expr> {
        if self.exprs_left == 0 || s.nest >= MAX_EXPR_NEST || !s.has_room(3) || self.u.is_empty() {
            return self.leaf(s);
        }
        self.exprs_left -= 1;
        let c = s.sub(1);
        let weights = [
            18 + 25 * u32::from(s.nest),        // leaf
            8,                                  // collection literal
            10,                                 // binary operator
            3,                                  // unary operator
            on(s.has_room(5), 3),               // and / or
            on(s.has_room(5), 6),               // comparison
            2,                                  // conditional expression
            12,                                 // builtin call
            on(s.has_room(5), 14),              // method call
            10,                                 // module function call
            5,                                  // user function call
            on(s.has_room(4), 3),               // attribute
            on(s.has_room(4), 6),               // subscript
            on(s.has_room(4), 4),               // f-string
            3,                                  // %-format / str.format
            on(s.has_room(6), 4),               // comprehension
            on(s.has_room(4), 2),               // lambda
            on(s.walrus_ok(), 2),               // walrus
            on(s.await_ok(), 3),                // await
            on(s.eval_nest < MAX_EVAL_NEST, 1), // eval / exec
            2,                                  // module constant
            on(s.has_room(5), 4),               // typed value (deque, Decimal, date, ...)
        ];
        Ok(match self.pick(&weights)? {
            0 => self.leaf(s)?,
            1 => self.collection(&c)?,
            2 => self.binop(&c)?,
            3 => Expr::Unary(UnaryOp::arbitrary(self.u)?, Box::new(self.expr(&c)?)),
            4 => {
                let count = self.count(2, 3)?;
                let operands = s.sub(levels(count)); // Monty folds `a or b or c` into nested nodes
                let op = BoolOp::arbitrary(self.u)?;
                let mut values = Vec::new();
                for _ in 0..count {
                    values.push(self.expr(&operands)?);
                }
                Expr::BoolOp(op, values)
            }
            5 => {
                let count = self.count(1, 2)?;
                let operands = s.sub(levels(count) + 1); // chains become nested `and`s
                let kind = if self.flag(50)? { Some(self.kind()?) } else { None };
                let left = self.operand(kind, &operands)?;
                let mut rest = Vec::new();
                for _ in 0..count {
                    rest.push((CmpOp::arbitrary(self.u)?, self.operand(kind, &operands)?));
                }
                Expr::Compare(Box::new(left), rest)
            }
            6 => Expr::IfExp {
                body: Box::new(self.expr(&c)?),
                test: Box::new(self.expr(&c)?),
                orelse: Box::new(self.expr(&c)?),
            },
            7 => self.builtin_call(&c)?,
            8 => self.method_call(&c)?,
            9 => self.module_call(&c)?,
            10 => self.user_call(&c)?,
            11 => self.attribute(&c)?,
            12 => self.subscript(&c)?,
            13 => self.fstring(&c)?,
            14 => self.format_expr(&c)?,
            15 => self.comprehension(&c)?,
            16 => {
                let lambda = self.lambda(&c, 0)?;
                if self.flag(50)? {
                    let args = self.positional_args(&c, 0, 2)?;
                    call(lambda, args)
                } else {
                    lambda
                }
            }
            17 => Expr::Walrus(self.choose(catalog::VARS)?, Box::new(self.expr(&c)?)),
            18 => Expr::Await(Box::new(self.awaitable(&c)?)),
            19 => self.eval_exec(s)?,
            20 => self.module_const()?,
            _ => {
                let family = self.family()?;
                self.typed_value(family, &c)?
            }
        })
    }

    /// A name or a literal.
    fn leaf(&mut self, s: &Scope) -> Result<Expr> {
        Ok(match self.pick(&[10, 4, 3, 2, 1, 1, 1, 1])? {
            0 => Expr::Name(self.readable_name(s)?),
            1 => self.int_lit()?,
            2 => Expr::Str(self.str_lit()?),
            3 => self.float_lit()?,
            4 => Expr::Bytes(self.bytes_lit()?),
            5 => Expr::Bool(self.flag(50)?),
            6 => Expr::None,
            _ => match self.pick(&[3, 2, 2, 1])? {
                0 => Expr::Raw(self.choose(catalog::RAW_LITERALS)?),
                1 => Expr::List(vec![]),
                2 => Expr::Dict(vec![]),
                _ => Expr::Ellipsis,
            },
        })
    }

    /// An integer: small, a boundary value, or any `i64`.
    fn int_lit(&mut self) -> Result<Expr> {
        Ok(match self.pick(&[6, 4, 1])? {
            0 => Expr::Int(self.choose(catalog::SMALL_INTS)?),
            1 => Expr::Int(self.choose(catalog::INTS)?),
            _ => Expr::Int(i128::from(i64::arbitrary(self.u)?)),
        })
    }

    /// A float: a boundary value or any bit pattern (NaN payloads, subnormals).
    fn float_lit(&mut self) -> Result<Expr> {
        Ok(Expr::Float(if self.flag(25)? {
            f64::from_bits(u64::arbitrary(self.u)?)
        } else {
            self.choose(catalog::FLOATS)?
        }))
    }

    /// A string from the pool, or a short one built from tricky characters.
    fn str_lit(&mut self) -> Result<String> {
        if self.flag(35)? {
            let len = self.count(0, 6)?;
            let mut text = String::new();
            for _ in 0..len {
                text.push(self.choose(catalog::STR_CHARS)?);
            }
            Ok(text)
        } else {
            Ok(self.choose(catalog::STRS)?.to_owned())
        }
    }

    /// Bytes from the pool, or a few arbitrary bytes.
    fn bytes_lit(&mut self) -> Result<Vec<u8>> {
        if self.flag(35)? {
            let len = self.count(0, 6)?;
            let mut bytes = Vec::new();
            for _ in 0..len {
                bytes.push(u8::arbitrary(self.u)?);
            }
            Ok(bytes)
        } else {
            Ok(self.choose(catalog::BYTES)?.to_vec())
        }
    }

    /// A list, tuple, set or dict literal.
    fn collection(&mut self, s: &Scope) -> Result<Expr> {
        let count = self.count(0, MAX_ELEMS)?;
        Ok(match self.pick(&[3, 2, 2, 3])? {
            0 => Expr::List(self.elts(s, count)?),
            1 => Expr::Tuple(self.elts(s, count)?),
            2 => Expr::Set(self.elts(s, count.max(1))?),
            _ => {
                let mut items = Vec::new();
                for _ in 0..count {
                    items.push(if s.has_room(2) && self.flag(10)? {
                        DictItem::Unpack(self.expr(&s.down(1))?)
                    } else {
                        let key = if self.flag(50)? { self.leaf(s)? } else { self.expr(s)? };
                        DictItem::Pair(key, self.expr(s)?)
                    });
                }
                Expr::Dict(items)
            }
        })
    }

    /// Elements of a list/tuple/set literal, occasionally starred.
    fn elts(&mut self, s: &Scope, count: u32) -> Result<Vec<Elt>> {
        let mut elts = Vec::new();
        for _ in 0..count {
            elts.push(if s.has_room(2) && self.flag(10)? {
                Elt::Star(self.iterable(&s.down(1))?)
            } else {
                Elt::Item(self.expr(s)?)
            });
        }
        Ok(elts)
    }

    /// An expression that is usually iterable: a collection variable, a fresh collection or
    /// range, sometimes anything.
    fn iterable(&mut self, s: &Scope) -> Result<Expr> {
        Ok(match self.pick(&[3, on(s.has_room(4), 4), 2])? {
            0 => Expr::Name(self.choose(&["v1", "v2", "v3", "v4", "v5"])?),
            1 => {
                let family = self.choose(&[
                    Family::List,
                    Family::Tuple,
                    Family::Str,
                    Family::Dict,
                    Family::Set,
                    Family::Range,
                    Family::Bytes,
                    Family::Deque,
                    Family::DictView,
                ])?;
                self.typed_value(family, s)?
            }
            _ => self.expr(s)?,
        })
    }

    /// A call argument: any expression, with plain ints and strings favoured since most
    /// parameters want one of those.
    fn arg_value(&mut self, s: &Scope) -> Result<Expr> {
        Ok(match self.pick(&[6, 2, 2])? {
            0 => self.expr(s)?,
            1 => self.int_lit()?,
            _ => Expr::Str(self.str_lit()?),
        })
    }

    /// A binary operation; half the time both operands share a type the operator supports.
    fn binop(&mut self, s: &Scope) -> Result<Expr> {
        if self.flag(50)? {
            let left = self.expr(s)?;
            let op = BinOp::arbitrary(self.u)?;
            return Ok(Expr::BinOp(Box::new(left), op, Box::new(self.expr(s)?)));
        }
        let kind = self.kind()?;
        let op = match kind {
            Kind::Int | Kind::Float => BinOp::arbitrary(self.u)?,
            Kind::Set => self.choose(&[BinOp::BitOr, BinOp::BitAnd, BinOp::BitXor, BinOp::Sub])?,
            Kind::Str | Kind::Bytes | Kind::List | Kind::Tuple => {
                if self.flag(60)? {
                    BinOp::Add
                } else {
                    self.choose(&[BinOp::Mult, BinOp::Mod, BinOp::Add])?
                }
            }
        };
        let left = self.operand(Some(kind), s)?;
        let right = match (op, kind) {
            (BinOp::Mult, Kind::Str | Kind::Bytes | Kind::List | Kind::Tuple) => self.int_lit()?,
            (BinOp::Mod, Kind::Str | Kind::Bytes) => self.expr(s)?,
            _ => self.operand(Some(kind), s)?,
        };
        Ok(Expr::BinOp(Box::new(left), op, Box::new(right)))
    }

    /// An operand type for [`Gen::binop`] and comparisons.
    fn kind(&mut self) -> Result<Kind> {
        self.choose(&[
            Kind::Int,
            Kind::Float,
            Kind::Str,
            Kind::Bytes,
            Kind::List,
            Kind::Tuple,
            Kind::Set,
        ])
    }

    /// An operand of type `kind` (a literal, the variable holding that type, or a fresh value),
    /// or any expression when `kind` is `None`.
    fn operand(&mut self, kind: Option<Kind>, s: &Scope) -> Result<Expr> {
        let Some(kind) = kind else { return self.expr(s) };
        Ok(match (kind, self.pick(&[4, 2, 1])?) {
            (_, 2) => self.expr(s)?,
            (Kind::Int, 0) => self.int_lit()?,
            (Kind::Int, _) => Expr::Name("v0"),
            (Kind::Float, _) => self.float_lit()?,
            (Kind::Str, 0) => Expr::Str(self.str_lit()?),
            (Kind::Str, _) => Expr::Name("v1"),
            (Kind::Bytes, 0) => Expr::Bytes(self.bytes_lit()?),
            (Kind::Bytes, _) => Expr::Name("v5"),
            (Kind::List, 0) => self.typed_value(Family::List, s)?,
            (Kind::List, _) => Expr::Name("v2"),
            (Kind::Tuple, 0) => self.typed_value(Family::Tuple, s)?,
            (Kind::Tuple, _) => Expr::Name("v4"),
            (Kind::Set, _) => self.typed_value(Family::Set, s)?,
        })
    }

    /// A builtin call; a few builtins get arguments of the type they need.
    fn builtin_call(&mut self, s: &Scope) -> Result<Expr> {
        let func = self.choose(catalog::BUILTINS)?;
        let args = match func.name {
            "isinstance" => vec![Arg::Pos(self.expr(s)?), Arg::Pos(self.type_spec()?)],
            "getattr" | "hasattr" | "setattr" => {
                let mut args = vec![
                    Arg::Pos(self.expr(s)?),
                    Arg::Pos(Expr::Str(self.choose(catalog::GETATTR_NAMES)?.to_owned())),
                ];
                if func.name == "setattr" || (func.name == "getattr" && self.flag(30)?) {
                    args.push(Arg::Pos(self.expr(s)?));
                }
                args
            }
            "format" => vec![Arg::Pos(self.expr(s)?), Arg::Pos(Expr::Str(self.spec_text()?))],
            _ => self.args(s, func)?,
        };
        Ok(call(Expr::Name(func.name), args))
    }

    /// The second argument of `isinstance`: a type or a tuple of types.
    fn type_spec(&mut self) -> Result<Expr> {
        Ok(if self.flag(25)? {
            let names = self.distinct_names(catalog::TYPE_NAMES, 2)?;
            Expr::Tuple(names.into_iter().map(|n| Elt::Item(Expr::Name(n))).collect())
        } else {
            Expr::Name(self.choose(catalog::TYPE_NAMES)?)
        })
    }

    /// A method call on a receiver that usually has the method's type.
    fn method_call(&mut self, s: &Scope) -> Result<Expr> {
        let family = self.family()?;
        let members = family.members();
        let method = self.choose(members.methods)?;
        let recv = if family == Family::Str && method.name == "format" && self.flag(70)? {
            Expr::Str(self.choose(catalog::BRACE_FORMATS)?.to_owned())
        } else {
            self.receiver(family, &s.down(1))?
        };
        Ok(Expr::MethodCall {
            recv: Box::new(recv),
            method: method.name,
            args: self.args(s, method)?,
        })
    }

    /// A stdlib function call; `re`, `json`, `tomllib` and `datetime` get fitting inputs.
    fn module_call(&mut self, s: &Scope) -> Result<Expr> {
        let module = self.choose_module(true)?;
        let func = self.choose(module.funcs)?;
        let args = match (module.name, func.name) {
            ("re", name) if name != "escape" && self.flag(70)? => {
                let mut args = vec![Arg::Pos(Expr::Str(self.choose(catalog::REGEXES)?.to_owned()))];
                args.extend(self.args(
                    s,
                    Func {
                        min: func.min - 1,
                        max: func.max - 1,
                        ..func
                    },
                )?);
                args
            }
            ("json", "loads") if self.flag(70)? => vec![Arg::Pos(self.text_from(catalog::JSON_TEXTS)?)],
            ("tomllib", "loads") if self.flag(70)? => vec![Arg::Pos(self.text_from(catalog::TOML_TEXTS)?)],
            ("datetime", "date" | "datetime") if self.flag(70)? => {
                let count = if func.name == "date" { 3 } else { self.count(3, 6)? };
                let mut args = Vec::new();
                for _ in 0..count {
                    args.push(Arg::Pos(Expr::Int(self.choose(catalog::DATE_INTS)?)));
                }
                args
            }
            _ => self.args(s, func)?,
        };
        Ok(mod_call(module.name, func.name, args))
    }

    /// A call of a user function or class, a lambda, or whatever a variable holds.
    fn user_call(&mut self, s: &Scope) -> Result<Expr> {
        let func = match self.pick(&[8, on(s.has_room(4), 1), 1])? {
            0 => Expr::Name(self.choose(catalog::USER_CALLABLES)?),
            1 => self.lambda(s, 0)?,
            _ => Expr::Name(self.readable_name(s)?),
        };
        let args = self.user_args(s, catalog::USER_KWS)?;
        Ok(call(func, args))
    }

    /// Arguments for a user-defined callable: some positionals, then distinct keywords from `kws`.
    fn user_args(&mut self, s: &Scope, kws: &[Name]) -> Result<Vec<Arg>> {
        let mut args = self.positional_args(s, 0, 3)?;
        if !kws.is_empty() && self.flag(30)? {
            for kw in self.distinct_names(kws, 2)? {
                args.push(Arg::Kw(kw, self.expr(s)?));
            }
        }
        Ok(args)
    }

    /// `min..=max` plain positional arguments.
    fn positional_args(&mut self, s: &Scope, min: u32, max: u32) -> Result<Vec<Arg>> {
        let count = self.count(min, max)?;
        let mut args = Vec::new();
        for _ in 0..count {
            args.push(Arg::Pos(self.expr(s)?));
        }
        Ok(args)
    }

    /// Arguments for a catalog callable: usually its arity, callbacks where it takes them,
    /// sometimes `*x`, then distinct keywords and sometimes `**x` (positionals always first).
    fn args(&mut self, s: &Scope, func: Func) -> Result<Vec<Arg>> {
        let count = if self.flag(85)? {
            self.count(u32::from(func.min), u32::from(func.max))?
        } else {
            self.count(0, 3)?
        };
        let mut args = Vec::new();
        for i in 0..count {
            args.push(if func.callback.map(u32::from) == Some(i) {
                Arg::Pos(self.callable(s)?)
            } else if s.has_room(2) && self.flag(6)? {
                Arg::Star(self.iterable(&s.down(1))?)
            } else {
                Arg::Pos(self.arg_value(s)?)
            });
        }
        if !func.kws.is_empty() && self.flag(30)? {
            for kw in self.distinct_names(func.kws, 2)? {
                let value = if catalog::CALLBACK_KWS.contains(&kw) {
                    self.callable(s)?
                } else {
                    self.expr(s)?
                };
                args.push(Arg::Kw(kw, value));
            }
        }
        if s.has_room(2) && self.flag(3)? {
            args.push(Arg::DoubleStar(self.expr(&s.down(1))?));
        }
        Ok(args)
    }

    /// Something callable: a builtin, a lambda, a user function or class.
    fn callable(&mut self, s: &Scope) -> Result<Expr> {
        Ok(match self.pick(&[4, on(s.has_room(4), 4), 2, 1])? {
            0 => Expr::Name(self.choose(catalog::CALLBACK_NAMES)?),
            1 => self.lambda(s, 1)?,
            2 => Expr::Name(self.choose(catalog::USER_CALLABLES)?),
            _ => self.expr(s)?,
        })
    }

    /// A lambda with `min_params..=2` parameters; defaults are evaluated in the outer scope.
    fn lambda(&mut self, s: &Scope, min_params: u32) -> Result<Expr> {
        let count = self.count(min_params, 2)? as usize;
        let defaults = self.count(0, len_u32(count))? as usize;
        let mut default_scope = s.down(2);
        default_scope.no_walrus = true;
        let mut params = Vec::new();
        for (i, name) in catalog::LAMBDA_PARAMS[..count].iter().enumerate() {
            let default = if i >= count - defaults {
                Some(self.expr(&default_scope)?)
            } else {
                None
            };
            params.push((*name, default));
        }
        let mut body_scope = s.down(2);
        body_scope.in_lambda = true;
        body_scope.locals.extend(&catalog::LAMBDA_PARAMS[..count]);
        Ok(Expr::Lambda {
            params,
            body: Box::new(self.expr(&body_scope)?),
        })
    }

    /// An attribute read, usually one the receiver's type has.
    fn attribute(&mut self, s: &Scope) -> Result<Expr> {
        let family = self.family()?;
        let attrs = family.members().attrs;
        Ok(if attrs.is_empty() || self.flag(10)? {
            Expr::Attr(Box::new(self.expr(s)?), self.choose(catalog::ATTRS)?)
        } else {
            let recv = self.receiver(family, &s.down(1))?;
            Expr::Attr(Box::new(recv), self.choose(attrs)?)
        })
    }

    /// `obj[index]` or `obj[slice]`, usually on a sequence or dict.
    fn subscript(&mut self, s: &Scope) -> Result<Expr> {
        let obj = match self.pick(&[3, 2, 2])? {
            0 => Expr::Name(self.choose(catalog::VARS)?),
            1 => {
                let family = self.choose(&[Family::List, Family::Str, Family::Tuple, Family::Bytes, Family::Dict])?;
                self.typed_value(family, s)?
            }
            _ => self.expr(s)?,
        };
        Ok(Expr::Subscript(Box::new(obj), Box::new(self.index(s)?)))
    }

    /// An index or a slice; small and boundary integers are favoured.
    fn index(&mut self, s: &Scope) -> Result<Index> {
        Ok(if self.flag(35)? {
            Index::Slice(
                self.maybe(60, |g| g.slice_bound(s))?,
                self.maybe(60, |g| g.slice_bound(s))?,
                self.maybe(25, |g| g.slice_bound(s))?,
            )
        } else if self.flag(60)? {
            Index::Expr(self.int_lit()?)
        } else {
            Index::Expr(self.expr(s)?)
        })
    }

    /// One part of a slice, usually an integer.
    fn slice_bound(&mut self, s: &Scope) -> Result<Expr> {
        if self.flag(70)? {
            self.int_lit()
        } else {
            self.expr(&s.down(1))
        }
    }

    /// An f-string of literal text and fields with conversions, `=` and format specs.
    fn fstring(&mut self, s: &Scope) -> Result<Expr> {
        let mut parts = Vec::new();
        for _ in 0..self.count(1, 3)? {
            parts.push(if self.flag(65)? {
                let expr = self.expr(&s.sub(1))?;
                let debug = self.flag(10)?;
                let conv = match self.pick(&[7, 1, 1, 1])? {
                    0 => None,
                    1 => Some('r'),
                    2 => Some('s'),
                    _ => Some('a'),
                };
                let spec = if self.flag(40)? { self.format_spec(s)? } else { vec![] };
                FPart::Field {
                    expr,
                    debug,
                    conv,
                    spec,
                }
            } else {
                FPart::Lit(self.choose(catalog::FSTRING_TEXT)?.to_owned())
            });
        }
        Ok(Expr::FString(parts))
    }

    /// A format spec: from the pool, or built field by field, maybe with nested `{width}`.
    fn format_spec(&mut self, s: &Scope) -> Result<Vec<SpecPart>> {
        if self.flag(40)? {
            return Ok(vec![SpecPart::Lit(self.choose(catalog::FORMAT_SPECS)?.to_owned())]);
        }
        let nested_ok = s.has_room(4);
        let mut parts = Vec::new();
        let mut text = self.spec_prefix()?;
        if self.flag(40)? {
            if nested_ok && self.flag(15)? {
                parts.push(SpecPart::Lit(mem::take(&mut text)));
                parts.push(SpecPart::Field(self.expr(&s.sub(2))?));
            } else {
                text.push_str(&self.spec_number(false)?);
            }
        }
        if self.flag(10)? {
            text.push(self.choose(&[',', '_'])?);
        }
        if self.flag(30)? {
            text.push('.');
            if nested_ok && self.flag(15)? {
                parts.push(SpecPart::Lit(mem::take(&mut text)));
                parts.push(SpecPart::Field(self.expr(&s.sub(2))?));
            } else {
                text.push_str(&self.spec_number(false)?);
            }
        }
        if self.flag(60)? {
            text.push(self.choose(catalog::FORMAT_TYPES)?);
        }
        parts.push(SpecPart::Lit(text));
        Ok(parts)
    }

    /// A complete spec as plain text, for `format(x, spec)`.
    fn spec_text(&mut self) -> Result<String> {
        if self.flag(50)? {
            return Ok(self.choose(catalog::FORMAT_SPECS)?.to_owned());
        }
        let mut text = self.spec_prefix()?;
        if self.flag(40)? {
            text.push_str(&self.spec_number(true)?);
        }
        if self.flag(30)? {
            text.push('.');
            text.push_str(&self.spec_number(true)?);
        }
        if self.flag(60)? {
            text.push(self.choose(catalog::FORMAT_TYPES)?);
        }
        Ok(text)
    }

    /// `[[fill]align][sign][z][#][0]`.
    fn spec_prefix(&mut self) -> Result<String> {
        let mut text = String::new();
        if self.flag(30)? {
            if self.flag(50)? {
                text.push(self.choose(catalog::FILL_CHARS)?);
            }
            text.push(self.choose(&['<', '>', '^', '='])?);
        }
        if self.flag(20)? {
            text.push(self.choose(&['+', '-', ' '])?);
        }
        if self.flag(5)? {
            text.push('z');
        }
        if self.flag(10)? {
            text.push('#');
        }
        if self.flag(10)? {
            text.push('0');
        }
        Ok(text)
    }

    /// A width or precision: usually small, sometimes huge. Widths beyond `usize` only when
    /// `overflow` is set: Monty rejects them at compile time inside f-string literals.
    fn spec_number(&mut self, overflow: bool) -> Result<String> {
        Ok(if self.flag(10)? {
            let huge: &[&str] = if overflow {
                &["1000", "100000000", "4294967296", "99999999999999999999"]
            } else {
                &["1000", "100000000", "4294967296"]
            };
            self.choose(huge)?.to_owned()
        } else {
            self.count(0, 30)?.to_string()
        })
    }

    /// `'%d' % x` or `'{0}'.format(x)`.
    fn format_expr(&mut self, s: &Scope) -> Result<Expr> {
        Ok(if self.flag(50)? {
            let fmt = Expr::Str(self.choose(catalog::PERCENT_FORMATS)?.to_owned());
            let rhs = match self.pick(&[2, 2, 1])? {
                0 => self.expr(s)?,
                1 => {
                    let count = self.count(1, 3)?;
                    Expr::Tuple(self.elts(s, count)?)
                }
                _ => Expr::Dict(vec![DictItem::Pair(Expr::Str("a".to_owned()), self.expr(s)?)]),
            };
            Expr::BinOp(Box::new(fmt), BinOp::Mod, Box::new(rhs))
        } else {
            let fmt = Expr::Str(self.choose(catalog::BRACE_FORMATS)?.to_owned());
            Expr::MethodCall {
                recv: Box::new(fmt),
                method: "format",
                args: self.user_args(s, &["a"])?,
            }
        })
    }

    /// A list/set/dict comprehension or generator expression with 1..=2 `for` clauses.
    fn comprehension(&mut self, s: &Scope) -> Result<Expr> {
        let gen_count = self.count(1, 2)?;
        let mut inner = s.down(levels(gen_count));
        inner.in_comp = true;
        let mut gens = Vec::new();
        for i in 0..gen_count as usize {
            let target = if gen_count == 1 && self.flag(25)? {
                Target::Tuple(catalog::COMP_VARS.to_vec(), None)
            } else {
                Target::Name(catalog::COMP_VARS[i])
            };
            let iter = self.iterable(&inner)?; // sees only the variables bound so far
            let mut cond_scope = inner.clone();
            match &target {
                Target::Tuple(names, _) => cond_scope.locals.extend(names),
                _ => cond_scope.locals.push(catalog::COMP_VARS[i]),
            }
            let ifs = if self.flag(30)? {
                vec![self.expr(&cond_scope)?]
            } else {
                vec![]
            };
            inner = cond_scope;
            gens.push(Comprehension { target, iter, ifs });
        }
        let kind = match self.pick(&[3, 1, 1, 2])? {
            0 => CompKind::List,
            1 => CompKind::Set,
            2 => CompKind::Dict,
            _ => CompKind::Gen,
        };
        let elt = Box::new(self.expr(&inner)?);
        let value = if matches!(kind, CompKind::Dict) {
            Some(Box::new(self.expr(&inner)?))
        } else {
            None
        };
        Ok(Expr::Comp { kind, elt, value, gens })
    }

    /// The operand of `await`: usually a coroutine.
    fn awaitable(&mut self, s: &Scope) -> Result<Expr> {
        Ok(match self.pick(&[3, 1, 1, 1])? {
            0 => {
                let args = self.positional_args(s, 0, 2)?;
                call(Expr::Name(self.choose(catalog::FUNCS)?), args)
            }
            1 => mod_call("asyncio", "sleep", vec![Arg::Pos(Expr::Int(0))]),
            2 => {
                let first = call(Expr::Name("f2"), vec![]);
                let second = self.expr(s)?;
                mod_call("asyncio", "gather", vec![Arg::Pos(first), Arg::Pos(second)])
            }
            _ => self.expr(s)?,
        })
    }

    /// `eval` of an expression or `exec` of statements; the inner source is parsed afresh,
    /// so its AST depth starts over, while the size budgets keep counting.
    fn eval_exec(&mut self, s: &Scope) -> Result<Expr> {
        Ok(if self.flag(60)? {
            let inner = Scope {
                depth: 1,
                nest: s.nest + 1,
                eval_nest: s.eval_nest + 1,
                locals: s.locals.clone(),
                ..Scope::default()
            };
            Expr::Eval(Box::new(self.expr(&inner)?))
        } else {
            let inner = Scope {
                depth: 1,
                eval_nest: s.eval_nest + 1,
                ..Scope::default()
            };
            let mut body = Vec::new();
            for _ in 0..self.count(1, 2)? {
                if self.stmts_left == 0 {
                    break;
                }
                body.push(self.stmt(&inner)?);
            }
            Expr::Exec(body)
        })
    }

    /// A module constant such as `math.pi` or `re.IGNORECASE`.
    fn module_const(&mut self) -> Result<Expr> {
        let with_consts: Vec<_> = MODULES.iter().filter(|m| !m.consts.is_empty()).collect();
        let module = self.choose(&with_consts)?;
        Ok(Expr::ModAttr(module.name, self.choose(module.consts)?))
    }

    // === Typed receivers ===

    /// A receiver type, weighted towards the core types.
    fn family(&mut self) -> Result<Family> {
        Ok(FAMILIES[self.pick(&FAMILY_WEIGHTS)?])
    }

    /// A receiver for `family`'s methods: its prologue variable, a fresh value of the type,
    /// or (rarely typed) any expression.
    fn receiver(&mut self, family: Family, s: &Scope) -> Result<Expr> {
        let home = family.home();
        Ok(match self.pick(&[on(home.is_some(), 4), on(s.has_room(4), 5), 1])? {
            0 => Expr::Name(home.unwrap_or("v0")),
            1 => self.typed_value(family, s)?,
            _ => self.expr(s)?,
        })
    }

    /// A freshly built value of `family`'s type.
    fn typed_value(&mut self, family: Family, s: &Scope) -> Result<Expr> {
        let c = s.sub(1);
        Ok(match family {
            Family::Str => Expr::Str(self.str_lit()?),
            Family::Bytes => Expr::Bytes(self.bytes_lit()?),
            Family::List => {
                let count = self.count(0, MAX_ELEMS)?;
                Expr::List(self.elts(&c, count)?)
            }
            Family::Tuple => {
                let count = self.count(0, MAX_ELEMS)?;
                Expr::Tuple(self.elts(&c, count)?)
            }
            Family::Set => {
                let count = self.count(1, MAX_ELEMS)?;
                Expr::Set(self.elts(&c, count)?)
            }
            Family::Dict => {
                let mut items = Vec::new();
                for _ in 0..self.count(0, MAX_ELEMS)? {
                    items.push(DictItem::Pair(self.leaf(&c)?, self.expr(&c)?));
                }
                Expr::Dict(items)
            }
            Family::DictView => {
                let dict = Expr::Name("v3");
                let method = self.choose(&["keys", "values", "items"])?;
                Expr::MethodCall {
                    recv: Box::new(dict),
                    method,
                    args: vec![],
                }
            }
            Family::Deque => {
                let count = self.count(0, MAX_ELEMS)?;
                let items = Expr::List(self.elts(&c.sub(1), count)?);
                let mut args = vec![Arg::Pos(items)];
                if self.flag(30)? {
                    args.push(Arg::Kw("maxlen", self.int_lit()?));
                }
                mod_call("collections", "deque", args)
            }
            Family::Counter => {
                let source = if self.flag(50)? {
                    Expr::Str(self.str_lit()?)
                } else {
                    self.expr(&c)?
                };
                mod_call("collections", "Counter", vec![Arg::Pos(source)])
            }
            Family::RePattern => {
                let regex = Expr::Str(self.choose(catalog::REGEXES)?.to_owned());
                mod_call("re", "compile", vec![Arg::Pos(regex)])
            }
            Family::ReMatch => {
                let regex = Expr::Str(self.choose(catalog::REGEXES)?.to_owned());
                let subject = Expr::Str(self.str_lit()?);
                let func = self.choose(&["search", "match", "fullmatch"])?;
                mod_call("re", func, vec![Arg::Pos(regex), Arg::Pos(subject)])
            }
            Family::Date | Family::DateTime => {
                let (func, count) = if family == Family::Date {
                    ("date", 3)
                } else {
                    ("datetime", self.count(3, 7)?)
                };
                let mut args = Vec::new();
                for _ in 0..count {
                    args.push(Arg::Pos(Expr::Int(self.choose(catalog::DATE_INTS)?)));
                }
                mod_call("datetime", func, args)
            }
            Family::TimeDelta => {
                let kw = self.choose(&["days", "seconds", "microseconds", "hours", "weeks"])?;
                let value = if self.flag(50)? {
                    self.int_lit()?
                } else {
                    self.float_lit()?
                };
                mod_call("datetime", "timedelta", vec![Arg::Kw(kw, value)])
            }
            Family::Decimal => {
                let value = match self.pick(&[2, 1, 1])? {
                    0 => Expr::Str(
                        self.choose(&["1.10", "-0", "NaN", "sNaN", "Infinity", "1E+2", "0.000001", "3.14"])?
                            .to_owned(),
                    ),
                    1 => self.int_lit()?,
                    _ => self.expr(&c)?,
                };
                mod_call("decimal", "Decimal", vec![Arg::Pos(value)])
            }
            Family::Hash => {
                let algo = self.choose(&["md5", "sha1", "sha256", "sha3_256", "sha512", "shake_128"])?;
                mod_call("hashlib", algo, vec![Arg::Pos(Expr::Bytes(self.bytes_lit()?))])
            }
            Family::Path => {
                let path = self.choose(&["a/b.txt", "/usr/lib/../bin/", "", ".", "x.tar.gz", "/"])?;
                mod_call("pathlib", "Path", vec![Arg::Pos(Expr::Str(path.to_owned()))])
            }
            Family::NamedTuple => {
                let class = mod_call(
                    "collections",
                    "namedtuple",
                    vec![
                        Arg::Pos(Expr::Str("P".to_owned())),
                        Arg::Pos(Expr::Str("a0 a1".to_owned())),
                    ],
                );
                let args = self.positional_args(&c, 2, 2)?;
                call(class, args)
            }
            Family::Random => mod_call("random", "Random", vec![Arg::Pos(self.int_lit()?)]),
            Family::Instance => {
                let args = self.positional_args(&c, 0, 2)?;
                call(Expr::Name(self.choose(catalog::CLASSES)?), args)
            }
            Family::Complex => {
                if self.flag(50)? {
                    Expr::Raw(self.choose(&["2j", "1.5j", "0j", "1e300j"])?)
                } else {
                    let args = self.positional_args(&c, 1, 2)?;
                    call(Expr::Name("complex"), args)
                }
            }
            Family::Range => {
                let mut args = Vec::new();
                for _ in 0..self.count(1, 3)? {
                    args.push(Arg::Pos(if self.flag(70)? {
                        Expr::Int(self.choose(catalog::SMALL_INTS)?)
                    } else {
                        self.expr(&c)?
                    }));
                }
                call(Expr::Name("range"), args)
            }
        })
    }

    /// One of `pool`'s strings as a literal.
    fn text_from(&mut self, pool: &[&str]) -> Result<Expr> {
        Ok(Expr::Str(self.choose(pool)?.to_owned()))
    }

    /// A module, optionally only among those with callables.
    fn choose_module(&mut self, with_funcs: bool) -> Result<&'static catalog::Module> {
        let modules: Vec<&'static catalog::Module> =
            MODULES.iter().filter(|m| !with_funcs || !m.funcs.is_empty()).collect();
        self.choose(&modules)
    }

    // === Primitive choices (all pick the simplest option on exhausted input) ===

    /// Index of a weighted choice; zero weights disable an option. Exhausted input
    /// picks the first enabled option.
    fn pick(&mut self, weights: &[u32]) -> Result<usize> {
        let total: u32 = weights.iter().sum();
        let mut roll = self.u.int_in_range(0..=total.saturating_sub(1))?;
        for (i, &weight) in weights.iter().enumerate() {
            if roll < weight {
                return Ok(i);
            }
            roll -= weight;
        }
        Ok(0)
    }

    /// True with roughly `percent`% probability; false on exhausted input.
    fn flag(&mut self, percent: u8) -> Result<bool> {
        Ok(self.u.int_in_range(0..=99u8)? >= 100 - percent.min(100))
    }

    /// A count in `min..=max`; `min` on exhausted input.
    fn count(&mut self, min: u32, max: u32) -> Result<u32> {
        self.u.int_in_range(min..=max.max(min))
    }

    /// One element of a non-empty slice; the first on exhausted input.
    fn choose<T: Copy>(&mut self, items: &[T]) -> Result<T> {
        Ok(*self.u.choose(items)?)
    }

    /// `Some(f())` with `percent`% probability.
    fn maybe<T>(&mut self, percent: u8, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<Option<T>> {
        if self.flag(percent)? {
            f(self).map(Some)
        } else {
            Ok(None)
        }
    }

    /// A fresh loop-guard id.
    fn guard(&mut self) -> u16 {
        let id = self.next_guard;
        self.next_guard += 1;
        id
    }
}
