//! The syntax tree of the Python subset that `ast_panic` generates.
//!
//! Every value of these types prints (see [`crate::printer`]) to source that is
//! valid Python *and* inside Monty's supported subset, so fuzzing effort goes
//! into the interpreter instead of into parse errors. Constructs Monty rejects
//! at compile time (`del`, `yield`, class bases, method decorators, `async
//! for`, `@=`, ...) have no representation here at all.
//!
//! The tree is built by the hand-written `Arbitrary` impl in `generate.rs`;
//! only the flat operator enums derive `Arbitrary` directly.

use std::fmt;

use arbitrary::Arbitrary;

use crate::printer;

/// An identifier taken from one of the fixed name pools in [`crate::catalog`].
pub type Name = &'static str;

/// A generated program: top-level statements, each wrapped by the printer in
/// its own `try`/`except Exception` so one error only skips that statement.
#[derive(Clone)]
pub struct Program {
    pub body: Vec<Stmt>,
}

impl Program {
    /// Prints the program, prologue included, as Python source.
    #[must_use]
    pub fn to_source(&self) -> String {
        printer::program_source(self)
    }
}

/// Shows the Python source, which is what `cargo fuzz fmt` prints for a crash input.
impl fmt::Debug for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_source())
    }
}

/// A statement. Context rules (e.g. `break` only inside loops) are enforced by the generator.
#[derive(Clone, Debug)]
pub enum Stmt {
    Expr(Expr),
    Assign {
        target: Target,
        value: Expr,
    },
    AugAssign {
        target: Target,
        op: BinOp,
        value: Expr,
    },
    If {
        test: Expr,
        body: Vec<Self>,
        elifs: Vec<(Expr, Vec<Self>)>,
        orelse: Vec<Self>,
    },
    /// Printed with a hidden counter `_g<guard>` that breaks after a few iterations.
    For {
        target: Target,
        iter: Expr,
        body: Vec<Self>,
        orelse: Vec<Self>,
        guard: u16,
    },
    /// Printed with the same hidden iteration guard as [`Stmt::For`].
    While {
        test: Expr,
        body: Vec<Self>,
        orelse: Vec<Self>,
        guard: u16,
    },
    Try {
        body: Vec<Self>,
        handlers: Vec<Handler>,
        orelse: Vec<Self>,
        finalbody: Vec<Self>,
    },
    Raise {
        exc: Option<Expr>,
        cause: Option<Expr>,
    },
    Assert {
        test: Expr,
        msg: Option<Expr>,
    },
    FunctionDef(Box<FunctionDef>),
    ClassDef(Box<ClassDef>),
    With {
        items: Vec<(Expr, Option<Name>)>,
        body: Vec<Self>,
    },
    Match {
        subject: Expr,
        cases: Vec<MatchCase>,
    },
    Import {
        module: Name,
        alias: Option<Name>,
    },
    ImportFrom {
        module: Name,
        name: Name,
        alias: Option<Name>,
    },
    Return(Option<Expr>),
    Global(Vec<Name>),
    Nonlocal(Vec<Name>),
    Break,
    Continue,
    Pass,
}

/// One `except` clause; empty `types` prints a bare `except:` (generated only as the last clause).
#[derive(Clone, Debug)]
pub struct Handler {
    pub types: Vec<Name>,
    pub name: Option<Name>,
    pub body: Vec<Stmt>,
}

/// A `def`, optionally followed by a call of the new function (`asyncio.run(...)` when async).
#[derive(Clone, Debug)]
pub struct FunctionDef {
    pub name: Name,
    pub is_async: bool,
    pub decorators: Vec<Expr>,
    pub params: Params,
    pub body: Vec<Stmt>,
    pub call: Option<Vec<Arg>>,
}

/// Parameters: positional ones (defaults only on a suffix), then optional `*args` and `**kwargs`.
#[derive(Clone, Debug, Default)]
pub struct Params {
    pub positional: Vec<(Name, Option<Expr>)>,
    pub vararg: Option<Name>,
    pub kwarg: Option<Name>,
}

impl Params {
    /// Every name the parameters bind, in declaration order.
    #[must_use]
    pub fn names(&self) -> Vec<Name> {
        self.positional
            .iter()
            .map(|(name, _)| *name)
            .chain(self.vararg)
            .chain(self.kwarg)
            .collect()
    }
}

/// A simple class (Monty has no inheritance), optionally a dataclass, optionally instantiated afterwards.
#[derive(Clone, Debug)]
pub struct ClassDef {
    pub name: Name,
    pub dataclass: Option<Dataclass>,
    /// Class-level fields: name, optional annotation, optional value.
    pub fields: Vec<(Name, Option<Name>, Option<Expr>)>,
    pub methods: Vec<Method>,
    /// `target = ClassName(args)` printed after the class.
    pub instantiate: Option<(Name, Vec<Arg>)>,
}

/// The two `@dataclass` options Monty supports.
#[derive(Clone, Copy, Debug)]
pub struct Dataclass {
    pub frozen: bool,
    pub eq: Option<bool>,
}

/// A method; `self` is implicit and printed first.
#[derive(Clone, Debug)]
pub struct Method {
    pub name: Name,
    pub params: Params,
    pub body: Vec<Stmt>,
}

/// One `case` of a `match` statement.
#[derive(Clone, Debug)]
pub struct MatchCase {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: Vec<Stmt>,
}

/// A `match` pattern. The generator keeps irrefutable patterns out of every case but the last,
/// keeps capture names unique per pattern, and limits or-patterns to literals.
#[derive(Clone, Debug)]
pub enum Pattern {
    /// A literal: int, str, bytes, `None`, `True` or `False`.
    Value(Expr),
    Capture(Name),
    Wildcard,
    /// Items plus an optional star at `(position, name)`; a `None` name prints `*_`.
    Sequence(Vec<Self>, Option<(usize, Option<Name>)>),
    Mapping(Vec<(Expr, Self)>, Option<Name>),
    Class {
        cls: Name,
        args: Vec<Self>,
        kwargs: Vec<(Name, Self)>,
    },
    Or(Vec<Self>),
    As(Box<Self>, Name),
}

/// An assignment target. Tuple targets hold only names, at most one of them starred.
#[derive(Clone, Debug)]
pub enum Target {
    Name(Name),
    Attr(Expr, Name),
    Subscript(Expr, Index),
    Tuple(Vec<Name>, Option<usize>),
}

/// An expression.
#[derive(Clone, Debug)]
pub enum Expr {
    Name(Name),
    Int(i128),
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
    Bool(bool),
    None,
    Ellipsis,
    /// A literal printed verbatim, e.g. `0xff`, `2j`, `'\ud800'`; always a non-negative atom.
    Raw(&'static str),
    List(Vec<Elt>),
    Tuple(Vec<Elt>),
    Set(Vec<Elt>),
    Dict(Vec<DictItem>),
    BinOp(Box<Self>, BinOp, Box<Self>),
    Unary(UnaryOp, Box<Self>),
    BoolOp(BoolOp, Vec<Self>),
    Compare(Box<Self>, Vec<(CmpOp, Self)>),
    IfExp {
        body: Box<Self>,
        test: Box<Self>,
        orelse: Box<Self>,
    },
    Call {
        func: Box<Self>,
        args: Vec<Arg>,
    },
    MethodCall {
        recv: Box<Self>,
        method: Name,
        args: Vec<Arg>,
    },
    Attr(Box<Self>, Name),
    /// `module.attr`; the printer imports `module` in the prologue. `attr` may be dotted.
    ModAttr(Name, Name),
    Subscript(Box<Self>, Box<Index>),
    FString(Vec<FPart>),
    Comp {
        kind: CompKind,
        elt: Box<Self>,
        /// The value of a dict comprehension (`elt` is then the key).
        value: Option<Box<Self>>,
        gens: Vec<Comprehension>,
    },
    Lambda {
        params: Vec<(Name, Option<Self>)>,
        body: Box<Self>,
    },
    Walrus(Name, Box<Self>),
    Await(Box<Self>),
    /// `eval('<printed expression>')`: exercises Monty's runtime compiler.
    Eval(Box<Self>),
    /// `exec('<printed statements>')`.
    Exec(Vec<Stmt>),
}

/// An element of a list, tuple or set literal.
#[derive(Clone, Debug)]
pub enum Elt {
    Item(Expr),
    Star(Expr),
}

/// An entry of a dict literal.
#[derive(Clone, Debug)]
pub enum DictItem {
    Pair(Expr, Expr),
    Unpack(Expr),
}

/// A call argument. The generator emits positional ones before keyword ones, as Python requires.
#[derive(Clone, Debug)]
pub enum Arg {
    Pos(Expr),
    Star(Expr),
    Kw(Name, Expr),
    DoubleStar(Expr),
}

/// A subscript index.
#[derive(Clone, Debug)]
pub enum Index {
    Expr(Expr),
    Slice(Option<Expr>, Option<Expr>, Option<Expr>),
}

/// A piece of an f-string.
#[derive(Clone, Debug)]
pub enum FPart {
    Lit(String),
    Field {
        expr: Expr,
        /// The `=` self-documenting form.
        debug: bool,
        conv: Option<char>,
        spec: Vec<SpecPart>,
    },
}

/// A piece of an f-string format spec: literal text or a nested `{field}`.
#[derive(Clone, Debug)]
pub enum SpecPart {
    Lit(String),
    Field(Expr),
}

/// One `for ... in ... if ...` clause of a comprehension.
#[derive(Clone, Debug)]
pub struct Comprehension {
    pub target: Target,
    pub iter: Expr,
    pub ifs: Vec<Expr>,
}

/// Which comprehension brackets to print.
#[derive(Clone, Copy, Debug)]
pub enum CompKind {
    List,
    Set,
    Dict,
    Gen,
}

/// Binary operators. `MatMult` is never used in augmented assignment (Monty rejects `@=`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Arbitrary)]
pub enum BinOp {
    Add,
    Sub,
    Mult,
    MatMult,
    Div,
    FloorDiv,
    Mod,
    Pow,
    LShift,
    RShift,
    BitOr,
    BitXor,
    BitAnd,
}

/// Unary operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Arbitrary)]
pub enum UnaryOp {
    Neg,
    Pos,
    Invert,
    Not,
}

/// `and` / `or`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Arbitrary)]
pub enum BoolOp {
    And,
    Or,
}

/// Comparison operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Arbitrary)]
pub enum CmpOp {
    Eq,
    NotEq,
    Lt,
    LtE,
    Gt,
    GtE,
    Is,
    IsNot,
    In,
    NotIn,
}
