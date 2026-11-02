//! Prints a [`Program`] as Python source.
//!
//! Syntax validity lives here and only here: indentation is tracked, empty
//! blocks print `pass`, literals are escaped (the output is pure ASCII), and
//! parentheses follow Python's precedence table. Parentheses are added only where
//! needed: Monty's exact AST-depth check ignores them, but its pre-parse scan of
//! sources over 4 KiB counts brackets, so redundant ones would cost budget.
//!
//! The printer also records which modules the program references and imports
//! exactly those in the prologue.

use std::{collections::BTreeSet, fmt::Write, slice};

use crate::ast::{
    Arg, BinOp, BoolOp, CmpOp, CompKind, DictItem, Elt, Expr, FPart, Index, Params, Pattern, Program, SpecPart, Stmt,
    Target, UnaryOp,
};

/// How many iterations a generated loop runs before its hidden guard breaks out.
pub const LOOP_LIMIT: u32 = 8;

/// Defines every name the generator may read before the generated statements run.
/// The variables start with distinct types; the functions and classes accept any arguments.
const PROLOGUE: &str = "\
v0 = 1
v1 = 'ab'
v2 = [1, 2]
v3 = {'a': 1}
v4 = (1, 'x')
v5 = b'xy'
def f0(*pa, **pk):
    return pa
def f1(*pa, **pk):
    return pk
async def f2(*pa, **pk):
    return pa
class C0:
    def __init__(self, *pa, **pk):
        self.a0 = pa
        self.a1 = pk
class C1:
    def __init__(self, *pa, **pk):
        pass
v6 = C0(1, 2)
_ok = 0
";

/// Prints the program: imports, prologue, the wrapped statements, and a final `_ok`
/// so `MontyRun::run` returns how many top-level statements completed.
#[must_use]
pub fn program_source(program: &Program) -> String {
    let mut body = Printer::default();
    for stmt in &program.body {
        body.top_level(stmt);
    }
    let mut out = String::new();
    for module in &body.modules {
        let _ = writeln!(out, "import {module}");
    }
    out.push_str(PROLOGUE);
    out.push_str(&body.out);
    out.push_str("_ok\n");
    out
}

/// Python operator precedence, weakest first. `Walrus` is below everything, so a
/// walrus is always parenthesized.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Prec {
    Walrus,
    Lambda,
    IfExp,
    Or,
    And,
    Not,
    Cmp,
    BitOr,
    BitXor,
    BitAnd,
    Shift,
    Arith,
    Term,
    Unary,
    Pow,
    Await,
    Primary,
    Atom,
}

impl Prec {
    /// The next tighter level, required of the right operand of a left-associative operator.
    fn tighter(self) -> Self {
        match self {
            Self::Walrus => Self::Lambda,
            Self::Lambda => Self::IfExp,
            Self::IfExp => Self::Or,
            Self::Or => Self::And,
            Self::And => Self::Not,
            Self::Not => Self::Cmp,
            Self::Cmp => Self::BitOr,
            Self::BitOr => Self::BitXor,
            Self::BitXor => Self::BitAnd,
            Self::BitAnd => Self::Shift,
            Self::Shift => Self::Arith,
            Self::Arith => Self::Term,
            Self::Term => Self::Unary,
            Self::Unary => Self::Pow,
            Self::Pow => Self::Await,
            Self::Await => Self::Primary,
            Self::Primary | Self::Atom => Self::Atom,
        }
    }
}

impl BinOp {
    /// The operator as written in Python.
    fn symbol(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mult => "*",
            Self::MatMult => "@",
            Self::Div => "/",
            Self::FloorDiv => "//",
            Self::Mod => "%",
            Self::Pow => "**",
            Self::LShift => "<<",
            Self::RShift => ">>",
            Self::BitOr => "|",
            Self::BitXor => "^",
            Self::BitAnd => "&",
        }
    }

    /// The operator's precedence level.
    fn prec(self) -> Prec {
        match self {
            Self::Add | Self::Sub => Prec::Arith,
            Self::Mult | Self::MatMult | Self::Div | Self::FloorDiv | Self::Mod => Prec::Term,
            Self::Pow => Prec::Pow,
            Self::LShift | Self::RShift => Prec::Shift,
            Self::BitOr => Prec::BitOr,
            Self::BitXor => Prec::BitXor,
            Self::BitAnd => Prec::BitAnd,
        }
    }
}

impl CmpOp {
    /// The comparison as written in Python.
    fn symbol(self) -> &'static str {
        match self {
            Self::Eq => "==",
            Self::NotEq => "!=",
            Self::Lt => "<",
            Self::LtE => "<=",
            Self::Gt => ">",
            Self::GtE => ">=",
            Self::Is => "is",
            Self::IsNot => "is not",
            Self::In => "in",
            Self::NotIn => "not in",
        }
    }
}

/// Accumulates source text plus the modules it references.
#[derive(Default)]
struct Printer {
    out: String,
    indent: usize,
    modules: BTreeSet<&'static str>,
}

impl Printer {
    /// Wraps a top-level statement so an exception only skips it, and counts it when it completes.
    fn top_level(&mut self, stmt: &Stmt) {
        self.line("try:");
        self.indent += 1;
        self.stmt(stmt);
        self.line("_ok += 1");
        self.indent -= 1;
        self.line("except Exception:");
        self.line("    pass");
    }

    /// Appends one line at the current indentation.
    fn line(&mut self, text: &str) {
        for _ in 0..self.indent {
            self.out.push_str("    ");
        }
        self.out.push_str(text);
        self.out.push('\n');
    }

    /// An indented block; `pass` when empty.
    fn block(&mut self, stmts: &[Stmt]) {
        self.indent += 1;
        if stmts.is_empty() {
            self.line("pass");
        }
        for stmt in stmts {
            self.stmt(stmt);
        }
        self.indent -= 1;
    }

    /// Prints one statement (several lines for compound statements).
    fn stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Expr(e) => {
                let text = self.expr(e, Prec::Lambda);
                self.line(&text);
            }
            Stmt::Assign { target, value } => {
                let line = format!("{} = {}", self.target(target), self.expr(value, Prec::Lambda));
                self.line(&line);
            }
            Stmt::AugAssign { target, op, value } => {
                let line = format!(
                    "{} {}= {}",
                    self.target(target),
                    op.symbol(),
                    self.expr(value, Prec::Lambda)
                );
                self.line(&line);
            }
            Stmt::If {
                test,
                body,
                elifs,
                orelse,
            } => {
                let header = format!("if {}:", self.expr(test, Prec::IfExp));
                self.line(&header);
                self.block(body);
                for (test, body) in elifs {
                    let header = format!("elif {}:", self.expr(test, Prec::IfExp));
                    self.line(&header);
                    self.block(body);
                }
                self.else_block(orelse);
            }
            Stmt::For {
                target,
                iter,
                body,
                orelse,
                guard,
            } => {
                let header = format!("for {} in {}:", self.target(target), self.expr(iter, Prec::IfExp));
                self.guarded_loop(*guard, &header, body, orelse);
            }
            Stmt::While {
                test,
                body,
                orelse,
                guard,
            } => {
                let header = format!("while {}:", self.expr(test, Prec::IfExp));
                self.guarded_loop(*guard, &header, body, orelse);
            }
            Stmt::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                self.line("try:");
                self.block(body);
                for handler in handlers {
                    let mut header = match handler.types.as_slice() {
                        [] => "except".to_owned(),
                        [one] => format!("except {one}"),
                        many => format!("except ({})", many.join(", ")),
                    };
                    if let Some(name) = handler.name {
                        let _ = write!(header, " as {name}");
                    }
                    header.push(':');
                    self.line(&header);
                    self.block(&handler.body);
                }
                self.else_block(orelse);
                if !finalbody.is_empty() || handlers.is_empty() {
                    self.line("finally:");
                    self.block(finalbody);
                }
            }
            Stmt::Raise { exc, cause } => {
                let mut line = "raise".to_owned();
                if let Some(exc) = exc {
                    let _ = write!(line, " {}", self.expr(exc, Prec::IfExp));
                    if let Some(cause) = cause {
                        let _ = write!(line, " from {}", self.expr(cause, Prec::IfExp));
                    }
                }
                self.line(&line);
            }
            Stmt::Assert { test, msg } => {
                let mut line = format!("assert {}", self.expr(test, Prec::IfExp));
                if let Some(msg) = msg {
                    let _ = write!(line, ", {}", self.expr(msg, Prec::IfExp));
                }
                self.line(&line);
            }
            Stmt::FunctionDef(def) => {
                for decorator in &def.decorators {
                    let line = format!("@{}", self.expr(decorator, Prec::Primary));
                    self.line(&line);
                }
                let keyword = if def.is_async { "async def" } else { "def" };
                let header = format!("{keyword} {}({}):", def.name, self.params(&def.params, None));
                self.line(&header);
                self.block(&def.body);
                if let Some(args) = &def.call {
                    let call = format!("{}({})", def.name, self.args(args));
                    let line = if def.is_async {
                        self.modules.insert("asyncio");
                        format!("asyncio.run({call})")
                    } else {
                        call
                    };
                    self.line(&line);
                }
            }
            Stmt::ClassDef(class) => {
                if let Some(dataclass) = class.dataclass {
                    self.modules.insert("dataclasses");
                    let mut options = Vec::new();
                    if dataclass.frozen {
                        options.push("frozen=True".to_owned());
                    }
                    if let Some(eq) = dataclass.eq {
                        options.push(format!("eq={}", if eq { "True" } else { "False" }));
                    }
                    let line = if options.is_empty() {
                        "@dataclasses.dataclass".to_owned()
                    } else {
                        format!("@dataclasses.dataclass({})", options.join(", "))
                    };
                    self.line(&line);
                }
                self.line(&format!("class {}:", class.name));
                self.indent += 1;
                if class.fields.is_empty() && class.methods.is_empty() {
                    self.line("pass");
                }
                for (name, annotation, value) in &class.fields {
                    let mut line = (*name).to_owned();
                    if let Some(annotation) = annotation {
                        let _ = write!(line, ": {annotation}");
                    }
                    if let Some(value) = value {
                        let _ = write!(line, " = {}", self.expr(value, Prec::IfExp));
                    }
                    self.line(&line);
                }
                for method in &class.methods {
                    let header = format!("def {}({}):", method.name, self.params(&method.params, Some("self")));
                    self.line(&header);
                    self.block(&method.body);
                }
                self.indent -= 1;
                if let Some((target, args)) = &class.instantiate {
                    let line = format!("{target} = {}({})", class.name, self.args(args));
                    self.line(&line);
                }
            }
            Stmt::With { items, body } => {
                let items: Vec<String> = items
                    .iter()
                    .map(|(context, target)| {
                        let context = self.expr(context, Prec::IfExp);
                        match target {
                            Some(target) => format!("{context} as {target}"),
                            None => context,
                        }
                    })
                    .collect();
                self.line(&format!("with {}:", items.join(", ")));
                self.block(body);
            }
            Stmt::Match { subject, cases } => {
                let header = format!("match {}:", self.expr(subject, Prec::Primary));
                self.line(&header);
                self.indent += 1;
                for case in cases {
                    let mut header = format!("case {}", pattern_text(&case.pattern));
                    if let Some(guard) = &case.guard {
                        let _ = write!(header, " if {}", self.expr(guard, Prec::IfExp));
                    }
                    header.push(':');
                    self.line(&header);
                    self.block(&case.body);
                }
                self.indent -= 1;
            }
            Stmt::Import { module, alias } => match alias {
                Some(alias) => self.line(&format!("import {module} as {alias}")),
                None => self.line(&format!("import {module}")),
            },
            Stmt::ImportFrom { module, name, alias } => match alias {
                Some(alias) => self.line(&format!("from {module} import {name} as {alias}")),
                None => self.line(&format!("from {module} import {name}")),
            },
            Stmt::Return(value) => match value {
                Some(value) => {
                    let line = format!("return {}", self.expr(value, Prec::Lambda));
                    self.line(&line);
                }
                None => self.line("return"),
            },
            Stmt::Global(names) => self.line(&format!("global {}", names.join(", "))),
            Stmt::Nonlocal(names) => self.line(&format!("nonlocal {}", names.join(", "))),
            Stmt::Break => self.line("break"),
            Stmt::Continue => self.line("continue"),
            Stmt::Pass => self.line("pass"),
        }
    }

    /// An `else:` block, omitted when empty.
    fn else_block(&mut self, orelse: &[Stmt]) {
        if !orelse.is_empty() {
            self.line("else:");
            self.block(orelse);
        }
    }

    /// A loop whose body starts by counting iterations and breaking after [`LOOP_LIMIT`],
    /// so generated loops stay short without depending on the time limit.
    fn guarded_loop(&mut self, guard: u16, header: &str, body: &[Stmt], orelse: &[Stmt]) {
        self.line(&format!("_g{guard} = 0"));
        self.line(header);
        self.indent += 1;
        self.line(&format!("_g{guard} += 1"));
        self.line(&format!("if _g{guard} > {LOOP_LIMIT}:"));
        self.line("    break");
        for stmt in body {
            self.stmt(stmt);
        }
        self.indent -= 1;
        self.else_block(orelse);
    }

    /// A parameter list; `first` is `self` for methods.
    fn params(&mut self, params: &Params, first: Option<&str>) -> String {
        let mut parts: Vec<String> = first.map(str::to_owned).into_iter().collect();
        for (name, default) in &params.positional {
            parts.push(match default {
                Some(default) => format!("{name}={}", self.expr(default, Prec::Or)),
                None => (*name).to_owned(),
            });
        }
        if let Some(vararg) = params.vararg {
            parts.push(format!("*{vararg}"));
        }
        if let Some(kwarg) = params.kwarg {
            parts.push(format!("**{kwarg}"));
        }
        parts.join(", ")
    }

    /// An assignment target.
    fn target(&mut self, target: &Target) -> String {
        match target {
            Target::Name(name) => (*name).to_owned(),
            Target::Attr(obj, attr) => format!("{}.{attr}", self.receiver(obj)),
            Target::Subscript(obj, index) => format!("{}[{}]", self.receiver(obj), self.index(index)),
            Target::Tuple(names, star) => names
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    if *star == Some(i) {
                        format!("*{name}")
                    } else {
                        (*name).to_owned()
                    }
                })
                .collect::<Vec<_>>()
                .join(", "),
        }
    }

    /// `e` as source, parenthesized if it binds more loosely than `min` requires.
    fn expr(&mut self, e: &Expr, min: Prec) -> String {
        let (text, prec) = self.expr_prec(e);
        if prec < min { format!("({text})") } else { text }
    }

    /// The receiver of `.attr` or `[index]`: a primary, and number literals always
    /// parenthesized (`1.real` does not tokenize).
    fn receiver(&mut self, e: &Expr) -> String {
        let (text, prec) = self.expr_prec(e);
        let numeric = matches!(e, Expr::Int(_) | Expr::Float(_) | Expr::Raw(_));
        if prec < Prec::Primary || numeric {
            format!("({text})")
        } else {
            text
        }
    }

    /// `e` as source plus its precedence.
    fn expr_prec(&mut self, e: &Expr) -> (String, Prec) {
        match e {
            Expr::Name(name) => ((*name).to_owned(), Prec::Atom),
            Expr::Int(value) => (value.to_string(), if *value < 0 { Prec::Unary } else { Prec::Atom }),
            Expr::Float(value) => float_literal(*value),
            Expr::Str(text) => (str_literal(text, false), Prec::Atom),
            Expr::Bytes(bytes) => (bytes_literal(bytes), Prec::Atom),
            Expr::Bool(value) => ((if *value { "True" } else { "False" }).to_owned(), Prec::Atom),
            Expr::None => ("None".to_owned(), Prec::Atom),
            Expr::Ellipsis => ("...".to_owned(), Prec::Atom),
            Expr::Raw(text) => ((*text).to_owned(), Prec::Atom),
            Expr::List(elts) => (format!("[{}]", self.elts(elts)), Prec::Atom),
            Expr::Tuple(elts) => {
                let text = match elts.as_slice() {
                    [one] => format!("({},)", self.elts(slice::from_ref(one))),
                    _ => format!("({})", self.elts(elts)),
                };
                (text, Prec::Atom)
            }
            Expr::Set(elts) => (format!("{{{}}}", self.elts(elts)), Prec::Atom),
            Expr::Dict(items) => {
                let items: Vec<String> = items
                    .iter()
                    .map(|item| match item {
                        DictItem::Pair(key, value) => {
                            format!("{}: {}", self.expr(key, Prec::Or), self.expr(value, Prec::IfExp))
                        }
                        DictItem::Unpack(value) => format!("**{}", self.expr(value, Prec::BitOr)),
                    })
                    .collect();
                (format!("{{{}}}", items.join(", ")), Prec::Atom)
            }
            Expr::BinOp(left, op, right) => {
                let prec = op.prec();
                let (left_min, right_min) = if *op == BinOp::Pow {
                    (Prec::Await, Prec::Unary) // `**` is right-associative and binds `-` on its right
                } else {
                    (prec, prec.tighter())
                };
                let text = format!(
                    "{} {} {}",
                    self.expr(left, left_min),
                    op.symbol(),
                    self.expr(right, right_min)
                );
                (text, prec)
            }
            Expr::Unary(op, operand) => match op {
                UnaryOp::Not => (format!("not {}", self.expr(operand, Prec::Not)), Prec::Not),
                UnaryOp::Neg => (format!("-{}", self.expr(operand, Prec::Unary)), Prec::Unary),
                UnaryOp::Pos => (format!("+{}", self.expr(operand, Prec::Unary)), Prec::Unary),
                UnaryOp::Invert => (format!("~{}", self.expr(operand, Prec::Unary)), Prec::Unary),
            },
            Expr::BoolOp(op, values) => {
                let (prec, word) = match op {
                    BoolOp::And => (Prec::And, " and "),
                    BoolOp::Or => (Prec::Or, " or "),
                };
                let parts: Vec<String> = values.iter().map(|v| self.expr(v, prec.tighter())).collect();
                (parts.join(word), prec)
            }
            Expr::Compare(left, rest) => {
                let mut text = self.expr(left, Prec::BitOr);
                for (op, right) in rest {
                    let _ = write!(text, " {} {}", op.symbol(), self.expr(right, Prec::BitOr));
                }
                (text, Prec::Cmp)
            }
            Expr::IfExp { body, test, orelse } => {
                let text = format!(
                    "{} if {} else {}",
                    self.expr(body, Prec::Or),
                    self.expr(test, Prec::Or),
                    self.expr(orelse, Prec::IfExp)
                );
                (text, Prec::IfExp)
            }
            Expr::Call { func, args } => {
                let text = format!("{}({})", self.expr(func, Prec::Primary), self.args(args));
                (text, Prec::Primary)
            }
            Expr::MethodCall { recv, method, args } => {
                let text = format!("{}.{method}({})", self.receiver(recv), self.args(args));
                (text, Prec::Primary)
            }
            Expr::Attr(obj, attr) => (format!("{}.{attr}", self.receiver(obj)), Prec::Primary),
            Expr::ModAttr(module, attr) => {
                self.modules.insert(*module);
                (format!("{module}.{attr}"), Prec::Primary)
            }
            Expr::Subscript(obj, index) => {
                let text = format!("{}[{}]", self.receiver(obj), self.index(index));
                (text, Prec::Primary)
            }
            Expr::FString(parts) => (self.fstring(parts), Prec::Atom),
            Expr::Comp { kind, elt, value, gens } => {
                let mut text = match (kind, value) {
                    (CompKind::Dict, Some(value)) => {
                        format!("{}: {}", self.expr(elt, Prec::Or), self.expr(value, Prec::IfExp))
                    }
                    _ => self.expr(elt, Prec::IfExp),
                };
                for comp in gens {
                    let _ = write!(
                        text,
                        " for {} in {}",
                        self.target(&comp.target),
                        self.expr(&comp.iter, Prec::Or)
                    );
                    for cond in &comp.ifs {
                        let _ = write!(text, " if {}", self.expr(cond, Prec::Or));
                    }
                }
                let text = match kind {
                    CompKind::List => format!("[{text}]"),
                    CompKind::Set | CompKind::Dict => format!("{{{text}}}"),
                    CompKind::Gen => format!("({text})"),
                };
                (text, Prec::Atom)
            }
            Expr::Lambda { params, body } => {
                let params: Vec<String> = params
                    .iter()
                    .map(|(name, default)| match default {
                        Some(default) => format!("{name}={}", self.expr(default, Prec::Or)),
                        None => (*name).to_owned(),
                    })
                    .collect();
                let body = self.expr(body, Prec::Lambda);
                let text = if params.is_empty() {
                    format!("lambda: {body}")
                } else {
                    format!("lambda {}: {body}", params.join(", "))
                };
                (text, Prec::Lambda)
            }
            Expr::Walrus(name, value) => (format!("{name} := {}", self.expr(value, Prec::Lambda)), Prec::Walrus),
            Expr::Await(value) => (format!("await {}", self.expr(value, Prec::Primary)), Prec::Await),
            Expr::Eval(inner) => {
                let mut nested = Self::default();
                let code = nested.expr(inner, Prec::Lambda);
                self.modules.extend(nested.modules);
                (format!("eval({})", str_literal(&code, false)), Prec::Primary)
            }
            Expr::Exec(stmts) => {
                let mut nested = Self::default();
                for stmt in stmts {
                    nested.stmt(stmt);
                }
                self.modules.extend(nested.modules);
                (format!("exec({})", str_literal(&nested.out, false)), Prec::Primary)
            }
        }
    }

    /// Comma-separated literal elements.
    fn elts(&mut self, elts: &[Elt]) -> String {
        elts.iter()
            .map(|elt| match elt {
                Elt::Item(e) => self.expr(e, Prec::IfExp),
                Elt::Star(e) => format!("*{}", self.expr(e, Prec::BitOr)),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Comma-separated call arguments.
    fn args(&mut self, args: &[Arg]) -> String {
        args.iter()
            .map(|arg| match arg {
                Arg::Pos(e) => self.expr(e, Prec::IfExp),
                Arg::Star(e) => format!("*{}", self.expr(e, Prec::BitOr)),
                Arg::Kw(name, e) => format!("{name}={}", self.expr(e, Prec::IfExp)),
                Arg::DoubleStar(e) => format!("**{}", self.expr(e, Prec::BitOr)),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The text between the brackets of a subscript.
    fn index(&mut self, index: &Index) -> String {
        match index {
            Index::Expr(e) => self.expr(e, Prec::IfExp),
            Index::Slice(lower, upper, step) => {
                let mut text = String::new();
                if let Some(lower) = lower {
                    text.push_str(&self.expr(lower, Prec::Or));
                }
                text.push(':');
                if let Some(upper) = upper {
                    text.push_str(&self.expr(upper, Prec::Or));
                }
                if let Some(step) = step {
                    text.push(':');
                    text.push_str(&self.expr(step, Prec::Or));
                }
                text
            }
        }
    }

    /// An f-string; every field expression is parenthesized, which sidesteps the
    /// special meaning of `:`, `!`, `=` and `{` inside replacement fields.
    fn fstring(&mut self, parts: &[FPart]) -> String {
        let mut text = String::from("f'");
        for part in parts {
            match part {
                FPart::Lit(lit) => escape_into(&mut text, lit, true),
                FPart::Field {
                    expr,
                    debug,
                    conv,
                    spec,
                } => {
                    let _ = write!(text, "{{({})", self.expr(expr, Prec::Walrus));
                    if *debug {
                        text.push('=');
                    }
                    if let Some(conv) = conv {
                        text.push('!');
                        text.push(*conv);
                    }
                    if !spec.is_empty() {
                        text.push(':');
                        for piece in spec {
                            match piece {
                                SpecPart::Lit(lit) => escape_into(&mut text, lit, true),
                                SpecPart::Field(e) => {
                                    let _ = write!(text, "{{({})}}", self.expr(e, Prec::Walrus));
                                }
                            }
                        }
                    }
                    text.push('}');
                }
            }
        }
        text.push('\'');
        text
    }
}

/// A `match` pattern.
fn pattern_text(pattern: &Pattern) -> String {
    match pattern {
        Pattern::Value(literal) => pattern_literal(literal),
        Pattern::Capture(name) => (*name).to_owned(),
        Pattern::Wildcard => "_".to_owned(),
        Pattern::Sequence(items, star) => {
            let mut parts: Vec<String> = items.iter().map(pattern_text).collect();
            if let Some((position, name)) = star {
                parts.insert((*position).min(parts.len()), format!("*{}", name.unwrap_or("_")));
            }
            format!("[{}]", parts.join(", "))
        }
        Pattern::Mapping(pairs, rest) => {
            let mut parts: Vec<String> = pairs
                .iter()
                .map(|(key, value)| format!("{}: {}", pattern_literal(key), pattern_text(value)))
                .collect();
            if let Some(rest) = rest {
                parts.push(format!("**{rest}"));
            }
            format!("{{{}}}", parts.join(", "))
        }
        Pattern::Class { cls, args, kwargs } => {
            let mut parts: Vec<String> = args.iter().map(pattern_text).collect();
            for (name, value) in kwargs {
                parts.push(format!("{name}={}", pattern_text(value)));
            }
            format!("{cls}({})", parts.join(", "))
        }
        Pattern::Or(alternatives) => alternatives.iter().map(pattern_text).collect::<Vec<_>>().join(" | "),
        Pattern::As(inner, name) => {
            let inner_text = pattern_text(inner);
            if matches!(**inner, Pattern::As(..)) {
                format!("({inner_text}) as {name}")
            } else {
                format!("{inner_text} as {name}")
            }
        }
    }
}

/// A literal inside a pattern, where signed numbers need no parentheses.
fn pattern_literal(literal: &Expr) -> String {
    match literal {
        Expr::Int(value) => value.to_string(),
        Expr::Str(text) => str_literal(text, false),
        Expr::Bytes(bytes) => bytes_literal(bytes),
        Expr::Bool(true) => "True".to_owned(),
        Expr::Bool(false) => "False".to_owned(),
        _ => "None".to_owned(),
    }
}

/// A float literal; infinities and NaN have no literal syntax, so they become calls.
fn float_literal(value: f64) -> (String, Prec) {
    if value.is_nan() {
        ("float('nan')".to_owned(), Prec::Primary)
    } else if value.is_infinite() {
        let text = if value > 0.0 { "float('inf')" } else { "float('-inf')" };
        (text.to_owned(), Prec::Primary)
    } else {
        // Rust's `{:?}` always prints a `.` or an exponent, so this stays a float literal.
        let text = format!("{value:?}");
        let prec = if text.starts_with('-') { Prec::Unary } else { Prec::Atom };
        (text, prec)
    }
}

/// A single-quoted, pure-ASCII string literal.
fn str_literal(text: &str, fstring: bool) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('\'');
    escape_into(&mut out, text, fstring);
    out.push('\'');
    out
}

/// Appends `text` escaped for a single-quoted literal; in f-strings braces are doubled.
fn escape_into(out: &mut String, text: &str, fstring: bool) {
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '{' if fstring => out.push_str("{{"),
            '}' if fstring => out.push_str("}}"),
            ' '..='~' => out.push(c),
            c if u32::from(c) < 0x100 => {
                let _ = write!(out, "\\x{:02x}", u32::from(c));
            }
            c if u32::from(c) < 0x1_0000 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => {
                let _ = write!(out, "\\U{:08x}", u32::from(c));
            }
        }
    }
}

/// A single-quoted bytes literal; everything outside printable ASCII becomes `\xNN`.
fn bytes_literal(bytes: &[u8]) -> String {
    let mut out = String::from("b'");
    for &b in bytes {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'\'' => out.push_str("\\'"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            b' '..=b'~' => out.push(char::from(b)),
            _ => {
                let _ = write!(out, "\\x{b:02x}");
            }
        }
    }
    out.push('\'');
    out
}
