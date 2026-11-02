//! What the generator may reference: names Monty implements and interesting literal values.
//!
//! The lists were extracted from Monty's source, not guessed:
//! - builtins: `limitations/builtins.md` and `crates/monty-types/src/builtins.rs`
//! - methods: the `StaticStrings` arms of each type's `py_call_attr` in `crates/monty/src/types/`
//! - module functions: the `*Functions` enums and `create_module` in `crates/monty/src/modules/`
//!
//! Anything that suspends to the host is left out (`open`, `os.listdir`, `Path.exists`,
//! `os.path.realpath`, ...): `MontyRun::run` cannot serve it and ends the whole program.
//! Arity ranges are the usual ones, not hard limits; the generator sometimes ignores them.

use std::f64::consts::PI;

use crate::ast::Name;

/// A callable: usual positional arity, accepted keywords, and which positional takes a callback.
#[derive(Clone, Copy, Debug)]
pub struct Func {
    pub name: &'static str,
    pub min: u8,
    pub max: u8,
    pub kws: &'static [&'static str],
    pub callback: Option<u8>,
}

/// A plain callable.
const fn f(name: &'static str, min: u8, max: u8) -> Func {
    Func {
        name,
        min,
        max,
        kws: &[],
        callback: None,
    }
}

/// A callable that accepts keyword arguments.
const fn fk(name: &'static str, min: u8, max: u8, kws: &'static [&'static str]) -> Func {
    Func {
        name,
        min,
        max,
        kws,
        callback: None,
    }
}

/// A callable whose positional argument `callback` should be a function.
const fn fc(name: &'static str, min: u8, max: u8, callback: u8, kws: &'static [&'static str]) -> Func {
    Func {
        name,
        min,
        max,
        kws,
        callback: Some(callback),
    }
}

// === Name pools ===

/// Global variables, all defined (with different types) by the prologue.
pub const VARS: &[Name] = &["v0", "v1", "v2", "v3", "v4", "v5", "v6"];
/// Functions; the prologue defines placeholders that accept any arguments.
pub const FUNCS: &[Name] = &["f0", "f1", "f2"];
/// Classes; the prologue defines placeholders.
pub const CLASSES: &[Name] = &["C0", "C1"];
/// Callable user-level names (functions and classes).
pub const USER_CALLABLES: &[Name] = &["f0", "f1", "f2", "C0", "C1"];
/// Instance attribute / dataclass field names.
pub const ATTRS: &[Name] = &["a0", "a1"];
/// Parameter names of top-level functions and methods (`pa`/`pk` for `*args`/`**kwargs`).
pub const PARAMS: &[Name] = &["p0", "p1", "p2"];
/// Parameter names of functions nested in a function, distinct so `nonlocal p0` stays legal.
pub const NESTED_PARAMS: &[Name] = &["q0", "q1", "q2"];
/// Lambda parameter names.
pub const LAMBDA_PARAMS: &[Name] = &["l0", "l1"];
/// Comprehension variable names.
pub const COMP_VARS: &[Name] = &["c0", "c1"];
/// Keywords used when calling user functions and classes.
pub const USER_KWS: &[Name] = &["p0", "p1", "p2", "a0", "a1", "q0"];
/// Builtins that make good callbacks (`map(f, ...)`, `key=f`).
pub const CALLBACK_NAMES: &[Name] = &[
    "len",
    "str",
    "repr",
    "int",
    "float",
    "abs",
    "bool",
    "sorted",
    "list",
    "tuple",
    "set",
    "hash",
    "ord",
    "chr",
    "type",
    "sum",
    "min",
    "max",
    "iter",
    "id",
    "bin",
    "hex",
    "reversed",
    "frozenset",
    "bytes",
    "dict",
];
/// Keyword arguments whose value should be a callable.
pub const CALLBACK_KWS: &[Name] = &[
    "key",
    "default_factory",
    "object_hook",
    "default",
    "parse_float",
    "parse_int",
];
/// Builtin type names, for `isinstance` and class patterns.
pub const TYPE_NAMES: &[Name] = &[
    "int",
    "float",
    "str",
    "bytes",
    "bool",
    "list",
    "tuple",
    "dict",
    "set",
    "frozenset",
    "complex",
    "range",
    "slice",
    "type",
];
/// Builtin types usable as class patterns with one positional sub-pattern.
pub const PATTERN_CLASSES: &[Name] = &["int", "float", "str", "bytes", "bool", "list", "tuple", "dict", "set"];
/// Annotations for dataclass fields (Monty stringizes them).
pub const ANNOTATIONS: &[Name] = &["int", "str", "float", "list", "dict", "tuple", "bool", "C0"];
/// Attribute names passed as strings to `getattr` / `hasattr` / `setattr`.
pub const GETATTR_NAMES: &[&str] = &[
    "a0",
    "a1",
    "m0",
    "real",
    "imag",
    "year",
    "append",
    "upper",
    "__class__",
    "__name__",
    "__dict__",
    "x",
];

/// Exceptions the program may catch (all builtin exception names).
pub const CATCHABLE: &[Name] = &[
    "Exception",
    "BaseException",
    "ArithmeticError",
    "OverflowError",
    "ZeroDivisionError",
    "LookupError",
    "IndexError",
    "KeyError",
    "RuntimeError",
    "NotImplementedError",
    "RecursionError",
    "AttributeError",
    "NameError",
    "UnboundLocalError",
    "ValueError",
    "UnicodeDecodeError",
    "UnicodeEncodeError",
    "ImportError",
    "ModuleNotFoundError",
    "OSError",
    "FileNotFoundError",
    "TimeoutError",
    "AssertionError",
    "MemoryError",
    "StopIteration",
    "SyntaxError",
    "TypeError",
    "KeyboardInterrupt",
    "SystemExit",
];
/// Exceptions the program may raise: everything `except Exception` catches.
pub const RAISABLE: &[Name] = &[
    "Exception",
    "ArithmeticError",
    "OverflowError",
    "ZeroDivisionError",
    "LookupError",
    "IndexError",
    "KeyError",
    "RuntimeError",
    "NotImplementedError",
    "RecursionError",
    "AttributeError",
    "NameError",
    "ValueError",
    "UnicodeDecodeError",
    "ImportError",
    "OSError",
    "TimeoutError",
    "AssertionError",
    "StopIteration",
    "TypeError",
];

// === Builtin functions ===

/// Builtin functions and type constructors (`open` left out: it suspends to the host).
/// `eval`/`exec` are generated as their own expression kinds.
pub const BUILTINS: &[Func] = &[
    f("abs", 1, 1),
    f("all", 1, 1),
    f("any", 1, 1),
    f("bin", 1, 1),
    f("chr", 1, 1),
    f("divmod", 2, 2),
    fk("enumerate", 1, 2, &["start"]),
    fc("filter", 2, 2, 0, &[]),
    f("format", 1, 2),
    f("getattr", 2, 3),
    f("hasattr", 2, 2),
    f("hash", 1, 1),
    f("hex", 1, 1),
    f("id", 1, 1),
    f("isinstance", 2, 2),
    f("iter", 1, 1),
    f("len", 1, 1),
    f("locals", 0, 0),
    fc("map", 2, 3, 0, &[]),
    fk("max", 1, 3, &["key", "default"]),
    fk("min", 1, 3, &["key", "default"]),
    f("next", 1, 2),
    f("oct", 1, 1),
    f("ord", 1, 1),
    fk("pow", 2, 3, &["mod"]),
    fk("print", 0, 3, &["sep", "end"]),
    f("repr", 1, 1),
    f("reversed", 1, 1),
    fk("round", 1, 2, &["ndigits"]),
    f("setattr", 3, 3),
    fk("sorted", 1, 1, &["key", "reverse"]),
    fk("sum", 1, 2, &["start"]),
    f("type", 1, 1),
    fk("zip", 0, 3, &["strict"]),
    f("bool", 0, 1),
    fk("bytes", 0, 3, &["encoding", "errors"]),
    f("complex", 0, 2),
    fk("dict", 0, 1, &["a", "b"]),
    f("float", 0, 1),
    f("frozenset", 0, 1),
    fk("int", 0, 2, &["base"]),
    f("list", 0, 1),
    f("range", 1, 3),
    f("set", 0, 1),
    f("slice", 1, 3),
    fk("str", 0, 3, &["encoding", "errors"]),
    f("tuple", 0, 1),
];

// === Methods, grouped by the type of their receiver ===

/// Receiver types the generator knows how to build and call methods on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Str,
    Bytes,
    List,
    Dict,
    Set,
    Tuple,
    Deque,
    Counter,
    RePattern,
    ReMatch,
    Date,
    DateTime,
    TimeDelta,
    Decimal,
    Hash,
    Path,
    NamedTuple,
    Random,
    Instance,
    Complex,
    Range,
    DictView,
}

/// Every family, parallel to [`FAMILY_WEIGHTS`].
pub const FAMILIES: [Family; 22] = [
    Family::Str,
    Family::Bytes,
    Family::List,
    Family::Dict,
    Family::Set,
    Family::Tuple,
    Family::Deque,
    Family::Counter,
    Family::RePattern,
    Family::ReMatch,
    Family::Date,
    Family::DateTime,
    Family::TimeDelta,
    Family::Decimal,
    Family::Hash,
    Family::Path,
    Family::NamedTuple,
    Family::Random,
    Family::Instance,
    Family::Complex,
    Family::Range,
    Family::DictView,
];
/// How often each family is picked; core types dominate.
pub const FAMILY_WEIGHTS: [u32; 22] = [24, 10, 14, 12, 7, 3, 4, 3, 3, 3, 2, 3, 1, 4, 2, 2, 2, 2, 4, 1, 1, 1];

/// Methods and plain attributes of one family.
pub struct Members {
    pub methods: &'static [Func],
    pub attrs: &'static [Name],
}

impl Family {
    /// The family's methods and readable attributes.
    #[must_use]
    pub fn members(self) -> Members {
        let (methods, attrs): (&'static [Func], &'static [Name]) = match self {
            Self::Str => (STR_METHODS, &[]),
            Self::Bytes => (BYTES_METHODS, &[]),
            Self::List => (LIST_METHODS, &[]),
            Self::Dict => (DICT_METHODS, &[]),
            Self::Set => (SET_METHODS, &[]),
            Self::Tuple => (TUPLE_METHODS, &[]),
            Self::Deque => (DEQUE_METHODS, &["maxlen"]),
            Self::Counter => (COUNTER_METHODS, &[]),
            Self::RePattern => (RE_PATTERN_METHODS, &["flags", "pattern"]),
            Self::ReMatch => (RE_MATCH_METHODS, &["string"]),
            Self::Date => (DATE_METHODS, &["year", "month", "day"]),
            Self::DateTime => (
                DATETIME_METHODS,
                &[
                    "year",
                    "month",
                    "day",
                    "hour",
                    "minute",
                    "second",
                    "microsecond",
                    "tzinfo",
                ],
            ),
            Self::TimeDelta => (TIMEDELTA_METHODS, &["days", "seconds", "microseconds"]),
            Self::Decimal => (DECIMAL_METHODS, &["real", "imag"]),
            Self::Hash => (HASH_METHODS, &["block_size", "digest_size", "name"]),
            Self::Path => (PATH_METHODS, &["name", "parent", "parts", "stem", "suffix", "suffixes"]),
            Self::NamedTuple => (NAMEDTUPLE_METHODS, &["_fields", "a0", "a1"]),
            Self::Random => (RANDOM_METHODS, &[]),
            Self::Instance => (INSTANCE_METHODS, &["a0", "a1"]),
            Self::Complex => (COMPLEX_METHODS, &["real", "imag"]),
            Self::Range => (RANGE_METHODS, &["start", "stop", "step"]),
            Self::DictView => (DICT_VIEW_METHODS, &[]),
        };
        Members { methods, attrs }
    }

    /// The prologue variable that starts out with this type, if any.
    #[must_use]
    pub fn home(self) -> Option<Name> {
        match self {
            Self::Str => Some("v1"),
            Self::List => Some("v2"),
            Self::Dict => Some("v3"),
            Self::Tuple => Some("v4"),
            Self::Bytes => Some("v5"),
            Self::Instance => Some("v6"),
            _ => None,
        }
    }
}

/// `datetime.timedelta` methods.
pub const TIMEDELTA_METHODS: &[Func] = &[f("total_seconds", 0, 0)];

/// Methods the generator gives its own classes.
pub const INSTANCE_METHODS: &[Func] = &[fk("m0", 0, 2, &["p0", "p1"]), fk("m1", 0, 2, &["p0", "p1"])];

/// `complex` methods.
pub const COMPLEX_METHODS: &[Func] = &[f("conjugate", 0, 0)];

/// `range` methods.
pub const RANGE_METHODS: &[Func] = &[f("count", 1, 1), f("index", 1, 1)];

/// Dict view (`keys()`, `items()`, `values()`) methods.
pub const DICT_VIEW_METHODS: &[Func] = &[f("isdisjoint", 1, 1)];

/// `str` methods.
pub const STR_METHODS: &[Func] = &[
    f("capitalize", 0, 0),
    f("casefold", 0, 0),
    f("center", 1, 2),
    f("count", 1, 3),
    fk("encode", 0, 2, &["encoding", "errors"]),
    f("endswith", 1, 3),
    fk("expandtabs", 0, 1, &["tabsize"]),
    f("find", 1, 3),
    fk("format", 0, 3, &["a", "b"]),
    f("index", 1, 3),
    f("isalnum", 0, 0),
    f("isalpha", 0, 0),
    f("isascii", 0, 0),
    f("isdecimal", 0, 0),
    f("isdigit", 0, 0),
    f("isidentifier", 0, 0),
    f("islower", 0, 0),
    f("isnumeric", 0, 0),
    f("isprintable", 0, 0),
    f("isspace", 0, 0),
    f("istitle", 0, 0),
    f("isupper", 0, 0),
    f("join", 1, 1),
    f("ljust", 1, 2),
    f("lower", 0, 0),
    f("lstrip", 0, 1),
    f("partition", 1, 1),
    f("removeprefix", 1, 1),
    f("removesuffix", 1, 1),
    fk("replace", 2, 3, &["count"]),
    f("rfind", 1, 3),
    f("rindex", 1, 3),
    f("rjust", 1, 2),
    f("rpartition", 1, 1),
    fk("rsplit", 0, 2, &["sep", "maxsplit"]),
    f("rstrip", 0, 1),
    fk("split", 0, 2, &["sep", "maxsplit"]),
    fk("splitlines", 0, 1, &["keepends"]),
    f("startswith", 1, 3),
    f("strip", 0, 1),
    f("swapcase", 0, 0),
    f("title", 0, 0),
    f("upper", 0, 0),
    f("zfill", 1, 1),
];

/// `bytes` methods.
pub const BYTES_METHODS: &[Func] = &[
    f("capitalize", 0, 0),
    f("center", 1, 2),
    f("count", 1, 3),
    fk("decode", 0, 2, &["encoding", "errors"]),
    f("endswith", 1, 3),
    f("find", 1, 3),
    f("fromhex", 1, 1),
    fk("hex", 0, 2, &["sep", "bytes_per_sep"]),
    f("index", 1, 3),
    f("isalnum", 0, 0),
    f("isalpha", 0, 0),
    f("isascii", 0, 0),
    f("isdigit", 0, 0),
    f("islower", 0, 0),
    f("isspace", 0, 0),
    f("istitle", 0, 0),
    f("isupper", 0, 0),
    f("join", 1, 1),
    f("ljust", 1, 2),
    f("lower", 0, 0),
    f("lstrip", 0, 1),
    f("partition", 1, 1),
    f("removeprefix", 1, 1),
    f("removesuffix", 1, 1),
    fk("replace", 2, 3, &["count"]),
    f("rfind", 1, 3),
    f("rindex", 1, 3),
    f("rjust", 1, 2),
    f("rpartition", 1, 1),
    fk("rsplit", 0, 2, &["sep", "maxsplit"]),
    f("rstrip", 0, 1),
    fk("split", 0, 2, &["sep", "maxsplit"]),
    fk("splitlines", 0, 1, &["keepends"]),
    f("startswith", 1, 3),
    f("strip", 0, 1),
    f("swapcase", 0, 0),
    f("title", 0, 0),
    f("upper", 0, 0),
    f("zfill", 1, 1),
];

/// `list` methods.
pub const LIST_METHODS: &[Func] = &[
    f("append", 1, 1),
    f("clear", 0, 0),
    f("copy", 0, 0),
    f("count", 1, 1),
    f("extend", 1, 1),
    f("index", 1, 3),
    f("insert", 2, 2),
    f("pop", 0, 1),
    f("remove", 1, 1),
    f("reverse", 0, 0),
    fk("sort", 0, 0, &["key", "reverse"]),
];

/// `dict` methods.
pub const DICT_METHODS: &[Func] = &[
    f("clear", 0, 0),
    f("copy", 0, 0),
    f("fromkeys", 1, 2),
    f("get", 1, 2),
    f("items", 0, 0),
    f("keys", 0, 0),
    f("pop", 1, 2),
    f("popitem", 0, 0),
    f("setdefault", 1, 2),
    fk("update", 0, 1, &["a", "b"]),
    f("values", 0, 0),
];

/// `collections.Counter` methods (its own plus common dict ones).
pub const COUNTER_METHODS: &[Func] = &[
    f("most_common", 0, 1),
    f("elements", 0, 0),
    f("total", 0, 0),
    f("subtract", 0, 1),
    f("update", 0, 1),
    f("get", 1, 2),
    f("items", 0, 0),
    f("pop", 1, 2),
    f("copy", 0, 0),
    f("setdefault", 1, 2),
];

/// `set` methods (the `*_update` forms are not implemented).
pub const SET_METHODS: &[Func] = &[
    f("add", 1, 1),
    f("clear", 0, 0),
    f("copy", 0, 0),
    f("difference", 0, 2),
    f("discard", 1, 1),
    f("intersection", 0, 2),
    f("isdisjoint", 1, 1),
    f("issubset", 1, 1),
    f("issuperset", 1, 1),
    f("pop", 0, 0),
    f("remove", 1, 1),
    f("symmetric_difference", 1, 1),
    f("union", 0, 2),
    f("update", 0, 2),
];

/// `tuple` methods.
pub const TUPLE_METHODS: &[Func] = &[f("count", 1, 1), f("index", 1, 3)];

/// `collections.deque` methods.
pub const DEQUE_METHODS: &[Func] = &[
    f("append", 1, 1),
    f("appendleft", 1, 1),
    f("clear", 0, 0),
    f("copy", 0, 0),
    f("count", 1, 1),
    f("extend", 1, 1),
    f("extendleft", 1, 1),
    f("index", 1, 3),
    f("insert", 2, 2),
    f("pop", 0, 0),
    f("popleft", 0, 0),
    f("remove", 1, 1),
    f("reverse", 0, 0),
    f("rotate", 0, 1),
];

/// `re.Pattern` methods.
pub const RE_PATTERN_METHODS: &[Func] = &[
    f("findall", 1, 3),
    f("finditer", 1, 3),
    f("fullmatch", 1, 3),
    f("match", 1, 3),
    f("search", 1, 3),
    fk("split", 1, 2, &["maxsplit"]),
    fk("sub", 2, 3, &["count"]),
];

/// `re.Match` methods.
pub const RE_MATCH_METHODS: &[Func] = &[
    f("end", 0, 1),
    f("group", 0, 2),
    fk("groupdict", 0, 1, &["default"]),
    fk("groups", 0, 1, &["default"]),
    f("span", 0, 1),
    f("start", 0, 1),
];

/// `datetime.date` methods.
pub const DATE_METHODS: &[Func] = &[
    f("isoformat", 0, 0),
    f("isoweekday", 0, 0),
    fk("replace", 0, 3, &["year", "month", "day"]),
    f("strftime", 1, 1),
    f("weekday", 0, 0),
];

/// `datetime.datetime` methods.
pub const DATETIME_METHODS: &[Func] = &[
    f("astimezone", 0, 1),
    f("date", 0, 0),
    f("dst", 0, 0),
    fk("isoformat", 0, 2, &["sep", "timespec"]),
    f("isoweekday", 0, 0),
    fk(
        "replace",
        0,
        3,
        &[
            "year",
            "month",
            "day",
            "hour",
            "minute",
            "second",
            "microsecond",
            "tzinfo",
        ],
    ),
    f("strftime", 1, 1),
    f("time", 0, 0),
    f("timestamp", 0, 0),
    f("timetz", 0, 0),
    f("tzname", 0, 0),
    f("utcoffset", 0, 0),
    f("weekday", 0, 0),
];

/// `decimal.Decimal` methods (from the `DecimalMethod` enum).
pub const DECIMAL_METHODS: &[Func] = &[
    f("adjusted", 0, 0),
    f("as_integer_ratio", 0, 0),
    f("as_tuple", 0, 0),
    f("canonical", 0, 0),
    f("compare", 1, 2),
    f("compare_signal", 1, 2),
    f("compare_total", 1, 2),
    f("compare_total_mag", 1, 2),
    f("conjugate", 0, 0),
    f("copy_abs", 0, 0),
    f("copy_negate", 0, 0),
    f("copy_sign", 1, 2),
    f("exp", 0, 1),
    f("fma", 2, 3),
    f("is_canonical", 0, 0),
    f("is_finite", 0, 0),
    f("is_infinite", 0, 0),
    f("is_nan", 0, 0),
    f("is_normal", 0, 1),
    f("is_qnan", 0, 0),
    f("is_signed", 0, 0),
    f("is_snan", 0, 0),
    f("is_subnormal", 0, 1),
    f("is_zero", 0, 0),
    f("ln", 0, 1),
    f("log10", 0, 1),
    f("logb", 0, 1),
    f("logical_and", 1, 2),
    f("logical_invert", 0, 1),
    f("logical_or", 1, 2),
    f("logical_xor", 1, 2),
    f("max", 1, 2),
    f("max_mag", 1, 2),
    f("min", 1, 2),
    f("min_mag", 1, 2),
    f("next_minus", 0, 1),
    f("next_plus", 0, 1),
    f("next_toward", 1, 2),
    f("normalize", 0, 1),
    f("number_class", 0, 1),
    fk("quantize", 1, 3, &["rounding", "context"]),
    f("radix", 0, 0),
    f("remainder_near", 1, 2),
    f("rotate", 1, 2),
    f("same_quantum", 1, 2),
    f("scaleb", 1, 2),
    f("shift", 1, 2),
    f("sqrt", 0, 1),
    f("to_eng_string", 0, 1),
    f("to_integral", 0, 2),
    f("to_integral_exact", 0, 2),
    f("to_integral_value", 0, 2),
];

/// `hashlib` hash-object methods.
pub const HASH_METHODS: &[Func] = &[
    f("copy", 0, 0),
    f("digest", 0, 1),
    f("hexdigest", 0, 1),
    f("update", 1, 1),
];

/// `pathlib.Path` methods that never touch the filesystem.
pub const PATH_METHODS: &[Func] = &[
    f("as_posix", 0, 0),
    f("is_absolute", 0, 0),
    f("joinpath", 0, 3),
    f("with_name", 1, 1),
    f("with_stem", 1, 1),
    f("with_suffix", 1, 1),
];

/// Methods of `collections.namedtuple` instances.
pub const NAMEDTUPLE_METHODS: &[Func] = &[
    f("_asdict", 0, 0),
    fk("_replace", 0, 0, &["a0", "a1"]),
    f("count", 1, 1),
    f("index", 1, 3),
];

/// `random.Random` methods (the same set as the module functions).
pub const RANDOM_METHODS: &[Func] = &[
    f("random", 0, 0),
    f("seed", 0, 1),
    f("getrandbits", 1, 1),
    f("randbytes", 1, 1),
    f("randrange", 1, 3),
    f("randint", 2, 2),
    f("choice", 1, 1),
    fk("choices", 1, 2, &["weights", "k"]),
    f("shuffle", 1, 1),
    fk("sample", 2, 2, &["counts"]),
    f("uniform", 2, 2),
    f("triangular", 0, 3),
    f("gauss", 0, 2),
    f("expovariate", 0, 1),
    f("betavariate", 2, 2),
    f("binomialvariate", 0, 2),
    f("getstate", 0, 0),
    f("setstate", 1, 1),
];

/// Method signatures generated in classes: name and fixed parameter count after `self`
/// (`None` means a flexible parameter list). Includes dunders Monty does not dispatch
/// (`__len__`, `__lt__`, `__bool__`) since defining them must not break anything either.
pub const CLASS_METHODS: &[(Name, Option<u8>)] = &[
    ("__init__", None),
    ("__repr__", Some(0)),
    ("__str__", Some(0)),
    ("__eq__", Some(1)),
    ("__hash__", Some(0)),
    ("__iter__", Some(0)),
    ("__next__", Some(0)),
    ("__contains__", Some(1)),
    ("__index__", Some(0)),
    ("__int__", Some(0)),
    ("__float__", Some(0)),
    ("__abs__", Some(0)),
    ("__round__", None),
    ("__floor__", Some(0)),
    ("__ceil__", Some(0)),
    ("__trunc__", Some(0)),
    ("__call__", None),
    ("__getitem__", Some(1)),
    ("__setitem__", Some(2)),
    ("__pos__", Some(0)),
    ("__neg__", Some(0)),
    ("__invert__", Some(0)),
    ("__add__", Some(1)),
    ("__radd__", Some(1)),
    ("__sub__", Some(1)),
    ("__rsub__", Some(1)),
    ("__mul__", Some(1)),
    ("__rmul__", Some(1)),
    ("__truediv__", Some(1)),
    ("__floordiv__", Some(1)),
    ("__mod__", Some(1)),
    ("__pow__", Some(1)),
    ("__lshift__", Some(1)),
    ("__and__", Some(1)),
    ("__or__", Some(1)),
    ("__xor__", Some(1)),
    ("__matmul__", Some(1)),
    ("__enter__", Some(0)),
    ("__exit__", Some(3)),
    ("__len__", Some(0)),
    ("__lt__", Some(1)),
    ("__bool__", Some(0)),
    ("m0", None),
    ("m1", None),
];

// === Modules ===

/// A stdlib module: its callables (names may be dotted, e.g. `date.fromisoformat`) and constants.
pub struct Module {
    pub name: Name,
    pub funcs: &'static [Func],
    pub consts: &'static [Name],
}

/// Every module the generator imports. `os.path` is used under its other name, `posixpath`.
pub const MODULES: &[Module] = &[
    Module {
        name: "math",
        funcs: &[
            f("floor", 1, 1),
            f("ceil", 1, 1),
            f("trunc", 1, 1),
            f("sqrt", 1, 1),
            f("isqrt", 1, 1),
            f("cbrt", 1, 1),
            f("pow", 2, 2),
            f("exp", 1, 1),
            f("exp2", 1, 1),
            f("expm1", 1, 1),
            f("log", 1, 2),
            f("log1p", 1, 1),
            f("log2", 1, 1),
            f("log10", 1, 1),
            f("fabs", 1, 1),
            f("isnan", 1, 1),
            f("isinf", 1, 1),
            f("isfinite", 1, 1),
            f("copysign", 2, 2),
            fk("isclose", 2, 2, &["rel_tol", "abs_tol"]),
            fk("nextafter", 2, 2, &["steps"]),
            f("ulp", 1, 1),
            f("sin", 1, 1),
            f("cos", 1, 1),
            f("tan", 1, 1),
            f("asin", 1, 1),
            f("acos", 1, 1),
            f("atan", 1, 1),
            f("atan2", 2, 2),
            f("sinh", 1, 1),
            f("cosh", 1, 1),
            f("tanh", 1, 1),
            f("asinh", 1, 1),
            f("acosh", 1, 1),
            f("atanh", 1, 1),
            f("degrees", 1, 1),
            f("radians", 1, 1),
            f("factorial", 1, 1),
            f("gcd", 0, 3),
            f("lcm", 0, 3),
            f("comb", 2, 2),
            f("perm", 1, 2),
            f("fmod", 2, 2),
            f("remainder", 2, 2),
            f("modf", 1, 1),
            f("frexp", 1, 1),
            f("ldexp", 2, 2),
            f("gamma", 1, 1),
            f("lgamma", 1, 1),
            f("erf", 1, 1),
            f("erfc", 1, 1),
            f("hypot", 0, 3),
            f("dist", 2, 2),
            f("fsum", 1, 1),
            fk("prod", 1, 1, &["start"]),
            f("sumprod", 2, 2),
            f("fma", 3, 3),
        ],
        consts: &["pi", "e", "tau", "inf", "nan"],
    },
    Module {
        name: "json",
        funcs: &[
            fk("loads", 1, 1, &["parse_float", "parse_int", "object_hook", "strict"]),
            fk(
                "dumps",
                1,
                1,
                &[
                    "indent",
                    "sort_keys",
                    "separators",
                    "ensure_ascii",
                    "default",
                    "skipkeys",
                    "allow_nan",
                ],
            ),
        ],
        consts: &[],
    },
    Module {
        name: "re",
        funcs: &[
            fk("compile", 1, 2, &["flags"]),
            fk("search", 2, 3, &["flags"]),
            fk("match", 2, 3, &["flags"]),
            fk("fullmatch", 2, 3, &["flags"]),
            fk("findall", 2, 3, &["flags"]),
            fk("finditer", 2, 3, &["flags"]),
            fk("sub", 3, 4, &["count", "flags"]),
            fk("split", 2, 3, &["maxsplit", "flags"]),
            f("escape", 1, 1),
        ],
        consts: &[
            "I",
            "M",
            "S",
            "A",
            "IGNORECASE",
            "MULTILINE",
            "DOTALL",
            "ASCII",
            "NOFLAG",
            "error",
        ],
    },
    Module {
        name: "itertools",
        funcs: &[
            fk("count", 0, 2, &["start", "step"]),
            f("cycle", 1, 1),
            fk("repeat", 1, 2, &["times"]),
            fc("accumulate", 1, 2, 1, &["initial"]),
            fk("batched", 2, 2, &["strict"]),
            f("chain", 0, 3),
            f("chain.from_iterable", 1, 1),
            f("compress", 2, 2),
            fc("dropwhile", 2, 2, 0, &[]),
            fc("filterfalse", 2, 2, 0, &[]),
            fc("groupby", 1, 2, 1, &["key"]),
            f("islice", 2, 4),
            f("pairwise", 1, 1),
            fc("starmap", 2, 2, 0, &[]),
            fc("takewhile", 2, 2, 0, &[]),
            f("tee", 1, 2),
            fk("zip_longest", 0, 3, &["fillvalue"]),
            fk("product", 0, 3, &["repeat"]),
            f("permutations", 1, 2),
            f("combinations", 2, 2),
            f("combinations_with_replacement", 2, 2),
        ],
        consts: &[],
    },
    Module {
        name: "functools",
        funcs: &[
            fc("reduce", 2, 3, 0, &[]),
            fc("partial", 1, 3, 0, &[]),
            fk("lru_cache", 0, 1, &["maxsize", "typed"]),
            fc("cache", 1, 1, 0, &[]),
        ],
        consts: &[],
    },
    Module {
        name: "collections",
        funcs: &[
            fk("deque", 0, 2, &["maxlen"]),
            f("Counter", 0, 1),
            fc("defaultdict", 0, 2, 0, &[]),
            fk("namedtuple", 2, 2, &["defaults", "rename"]),
        ],
        consts: &[],
    },
    Module {
        name: "copy",
        funcs: &[f("copy", 1, 1), f("deepcopy", 1, 1)],
        consts: &[],
    },
    Module {
        name: "bisect",
        funcs: &[
            fk("bisect_left", 2, 4, &["lo", "hi", "key"]),
            fk("bisect_right", 2, 4, &["lo", "hi", "key"]),
            fk("insort_left", 2, 4, &["lo", "hi", "key"]),
            fk("insort_right", 2, 4, &["lo", "hi", "key"]),
            fk("bisect", 2, 4, &["lo", "hi", "key"]),
            fk("insort", 2, 4, &["lo", "hi", "key"]),
        ],
        consts: &[],
    },
    Module {
        name: "base64",
        funcs: &[
            fk("b64encode", 1, 2, &["altchars"]),
            fk("b64decode", 1, 3, &["altchars", "validate"]),
            f("standard_b64encode", 1, 1),
            f("standard_b64decode", 1, 1),
            f("urlsafe_b64encode", 1, 1),
            f("urlsafe_b64decode", 1, 1),
            f("b32encode", 1, 1),
            fk("b32decode", 1, 3, &["casefold", "map01"]),
            f("b32hexencode", 1, 1),
            fk("b32hexdecode", 1, 2, &["casefold"]),
            f("b16encode", 1, 1),
            fk("b16decode", 1, 2, &["casefold"]),
            f("encodebytes", 1, 1),
            f("decodebytes", 1, 1),
            fk("b85encode", 1, 2, &["pad"]),
            f("b85decode", 1, 1),
            f("z85encode", 1, 1),
            f("z85decode", 1, 1),
            fk("a85encode", 1, 1, &["foldspaces", "wrapcol", "pad", "adobe"]),
            fk("a85decode", 1, 1, &["foldspaces", "adobe", "ignorechars"]),
        ],
        consts: &[],
    },
    Module {
        name: "binascii",
        funcs: &[
            fk("hexlify", 1, 3, &["sep", "bytes_per_sep"]),
            f("unhexlify", 1, 1),
            fk("b2a_hex", 1, 3, &["sep", "bytes_per_sep"]),
            f("a2b_hex", 1, 1),
            fk("b2a_base64", 1, 1, &["newline"]),
            fk("a2b_base64", 1, 1, &["strict_mode"]),
            f("crc32", 1, 2),
            f("crc_hqx", 2, 2),
            fk("b2a_uu", 1, 1, &["backtick"]),
            f("a2b_uu", 1, 1),
            fk("b2a_qp", 1, 4, &["quotetabs", "istext", "header"]),
            fk("a2b_qp", 1, 2, &["header"]),
        ],
        consts: &[],
    },
    Module {
        name: "hashlib",
        funcs: &[
            f("md5", 0, 1),
            f("sha1", 0, 1),
            f("sha224", 0, 1),
            f("sha256", 0, 1),
            f("sha384", 0, 1),
            f("sha512", 0, 1),
            f("sha3_224", 0, 1),
            f("sha3_256", 0, 1),
            f("sha3_384", 0, 1),
            f("sha3_512", 0, 1),
            f("shake_128", 0, 1),
            f("shake_256", 0, 1),
            fk("new", 1, 2, &["data"]),
            fk("pbkdf2_hmac", 4, 5, &["dklen"]),
        ],
        consts: &[],
    },
    Module {
        name: "datetime",
        funcs: &[
            fk("date", 1, 3, &["year", "month", "day"]),
            fk("datetime", 3, 7, &["tzinfo", "fold", "microsecond"]),
            fk("time", 0, 4, &["hour", "minute", "second", "microsecond", "tzinfo"]),
            fk(
                "timedelta",
                0,
                3,
                &[
                    "days",
                    "seconds",
                    "microseconds",
                    "milliseconds",
                    "minutes",
                    "hours",
                    "weeks",
                ],
            ),
            f("timezone", 1, 2),
            f("date.today", 0, 0),
            f("date.fromisoformat", 1, 1),
            f("date.fromtimestamp", 1, 1),
            fk("datetime.now", 0, 1, &["tz"]),
            f("datetime.fromisoformat", 1, 1),
            fk("datetime.fromtimestamp", 1, 2, &["tz"]),
            f("datetime.strptime", 2, 2),
            f("datetime.combine", 2, 3),
        ],
        consts: &[
            "date.min",
            "date.max",
            "datetime.min",
            "datetime.max",
            "timedelta.min",
            "timedelta.max",
            "timedelta.resolution",
            "timezone.utc",
        ],
    },
    Module {
        name: "decimal",
        funcs: &[
            fk("Decimal", 0, 2, &["context"]),
            f("getcontext", 0, 0),
            f("setcontext", 1, 1),
            f("localcontext", 0, 1),
            f("IEEEContext", 1, 1),
        ],
        consts: &[
            "MAX_PREC",
            "MAX_EMAX",
            "MIN_EMIN",
            "MIN_ETINY",
            "BasicContext",
            "ExtendedContext",
            "DefaultContext",
        ],
    },
    Module {
        name: "fnmatch",
        funcs: &[
            f("fnmatch", 2, 2),
            f("fnmatchcase", 2, 2),
            f("filter", 2, 2),
            f("translate", 1, 1),
        ],
        consts: &[],
    },
    Module {
        name: "random",
        funcs: &[
            f("random", 0, 0),
            fk("seed", 0, 2, &["version"]),
            f("getstate", 0, 0),
            f("setstate", 1, 1),
            f("getrandbits", 1, 1),
            f("randbytes", 1, 1),
            fk("randrange", 1, 3, &["step"]),
            f("randint", 2, 2),
            f("choice", 1, 1),
            fk("choices", 1, 2, &["weights", "cum_weights", "k"]),
            f("shuffle", 1, 1),
            fk("sample", 2, 2, &["counts"]),
            f("uniform", 2, 2),
            f("triangular", 0, 3),
            f("normalvariate", 0, 2),
            f("gauss", 0, 2),
            f("lognormvariate", 2, 2),
            f("expovariate", 0, 1),
            f("vonmisesvariate", 2, 2),
            f("gammavariate", 2, 2),
            f("betavariate", 2, 2),
            f("paretovariate", 1, 1),
            f("weibullvariate", 2, 2),
            fk("binomialvariate", 0, 2, &["n", "p"]),
            f("Random", 0, 1),
        ],
        consts: &[],
    },
    Module {
        name: "time",
        funcs: &[
            f("time", 0, 0),
            f("sleep", 1, 1),
            f("time_ns", 0, 0),
            f("monotonic", 0, 0),
            f("monotonic_ns", 0, 0),
            f("perf_counter", 0, 0),
            f("perf_counter_ns", 0, 0),
            f("process_time", 0, 0),
            f("process_time_ns", 0, 0),
            f("thread_time", 0, 0),
            f("thread_time_ns", 0, 0),
            f("gmtime", 0, 1),
            f("localtime", 0, 1),
            f("mktime", 1, 1),
            f("asctime", 0, 1),
            f("ctime", 0, 1),
            f("strftime", 1, 2),
            f("strptime", 1, 2),
        ],
        consts: &["timezone", "altzone", "daylight", "tzname"],
    },
    Module {
        name: "tomllib",
        funcs: &[fk("loads", 1, 1, &["parse_float"])],
        consts: &[],
    },
    Module {
        name: "unicodedata",
        funcs: &[
            f("category", 1, 1),
            f("name", 1, 2),
            f("lookup", 1, 1),
            f("combining", 1, 1),
            f("normalize", 2, 2),
            f("is_normalized", 2, 2),
        ],
        consts: &["unidata_version"],
    },
    Module {
        name: "csv",
        funcs: &[
            fk(
                "reader",
                1,
                1,
                &[
                    "delimiter",
                    "quotechar",
                    "quoting",
                    "skipinitialspace",
                    "strict",
                    "escapechar",
                ],
            ),
            f("get_dialect", 1, 1),
            f("list_dialects", 0, 0),
            f("field_size_limit", 0, 1),
        ],
        consts: &["QUOTE_MINIMAL", "QUOTE_ALL", "QUOTE_NONNUMERIC", "QUOTE_NONE"],
    },
    Module {
        name: "dataclasses",
        funcs: &[
            fk("dataclass", 0, 1, &["eq", "frozen"]),
            f("is_dataclass", 1, 1),
            fk("field", 0, 0, &["default", "default_factory"]),
        ],
        consts: &[],
    },
    Module {
        name: "asyncio",
        funcs: &[f("run", 1, 1), f("gather", 0, 3), f("sleep", 1, 2)],
        consts: &[],
    },
    Module {
        name: "posixpath",
        funcs: &[
            f("join", 1, 3),
            f("split", 1, 1),
            f("splitext", 1, 1),
            f("splitdrive", 1, 1),
            f("splitroot", 1, 1),
            f("basename", 1, 1),
            f("dirname", 1, 1),
            f("isabs", 1, 1),
            f("normcase", 1, 1),
            f("normpath", 1, 1),
            f("abspath", 1, 1),
            f("relpath", 1, 2),
            f("commonpath", 1, 1),
            f("commonprefix", 1, 1),
            f("isjunction", 1, 1),
            f("isdevdrive", 1, 1),
        ],
        consts: &["sep", "curdir", "pardir"],
    },
    Module {
        name: "pathlib",
        funcs: &[f("Path", 0, 3)],
        consts: &[],
    },
    Module {
        name: "typing",
        funcs: &[f("get_type_hints", 1, 1)],
        consts: &["Any", "Optional", "List"],
    },
    Module {
        name: "os",
        funcs: &[f("fspath", 1, 1)],
        consts: &["sep", "name"],
    },
    Module {
        name: "sys",
        funcs: &[],
        consts: &["maxsize", "version", "platform", "byteorder", "version_info"],
    },
];

// === Literal pools ===

/// Small integers, the most common values in real code.
pub const SMALL_INTS: &[i128] = &[0, 1, 2, 3, -1, 4, 5, 7, 8, 10, -2, 16, 100, 6, 9, -5];

/// Integer boundary values: word sizes, float precision, Unicode and year limits.
pub const INTS: &[i128] = &[
    0,
    -1,
    31,
    32,
    63,
    64,
    127,
    128,
    255,
    256,
    1000,
    1024,
    4096,
    9999,
    10000,
    65535,
    65536,
    1_114_111,
    1_114_112,
    1_000_000,
    2_147_483_647,
    2_147_483_648,
    -2_147_483_648,
    4_294_967_295,
    4_294_967_296,
    9_007_199_254_740_993,
    9_223_372_036_854_775_807,
    -9_223_372_036_854_775_808,
    9_223_372_036_854_775_808,
    -9_223_372_036_854_775_809,
    18_446_744_073_709_551_615,
    18_446_744_073_709_551_616,
    100_000_000_000_000_000_000,
    -100_000_000_000_000_000_000,
    170_141_183_460_469_231_731_687_303_715_884_105_727,
];

/// Float boundary values (infinities and NaN print as `float('...')`).
pub const FLOATS: &[f64] = &[
    0.0,
    -0.0,
    0.5,
    1.0,
    -1.5,
    2.5,
    0.1,
    1e-7,
    1e16,
    1e22,
    1e300,
    -1e300,
    f64::MAX,
    f64::MIN_POSITIVE,
    5e-324,
    f64::INFINITY,
    f64::NEG_INFINITY,
    f64::NAN,
    PI,
    9_007_199_254_740_993.0,
    4_503_599_627_370_496.5,
    123_456_789.987_654_33,
    1e100,
];

/// Literals printed verbatim: other number syntaxes, imaginary numbers, special string forms.
pub const RAW_LITERALS: &[&str] = &[
    "0xff",
    "0o777",
    "0b1011",
    "1_000_000",
    "0x7fffffffffffffff",
    "0xffffffffffffffffffffffff",
    "340282366920938463463374607431768211456",
    "1000000000000000000000000000000000000000000000000000000000000",
    "2j",
    "1.5j",
    "0j",
    "1e300j",
    ".5",
    "5.",
    "1_0.0_1",
    "1e-5",
    "r'\\d+\\s*'",
    "'\\N{BULLET}'",
    "rb'\\x00'",
    "'''multi\\nline'''",
];

/// String values: edge cases for case mapping, number parsing, formatting, regex, JSON, TOML, dates.
pub const STRS: &[&str] = &[
    "",
    "a",
    "ab",
    "abc",
    "hello world",
    "  padded  ",
    "ABC",
    "aBc DeF",
    "123",
    "-42",
    "+7",
    "3.14",
    "1e5",
    "1_000",
    "0x1f",
    "inf",
    "-nan",
    "True",
    "None",
    "a,b,,c",
    "a b\tc\nd",
    "line1\nline2\r\nline3",
    "\0",
    "\x7f",
    "é",
    "ß",
    "ﬁ",
    "İ",
    "Σ",
    "ǅ",
    "ŉ",
    "😀",
    "\u{10ffff}",
    "a\u{301}",
    "\u{200b}",
    "\u{ffff}",
    "%s",
    "%d",
    "%5.2f",
    "%(a)s",
    "{}",
    "{0}",
    "{0!r:>10}",
    "{:,}",
    "{a}",
    "{{}}",
    "[1, 2, 3]",
    "{\"a\": [1, null, true, 1.5e300]}",
    "\"str\"",
    "a = 1\nb = 'x'",
    "[t]\nk = [1, {x = 2}]",
    "(a|b)*c",
    "^\\w+$",
    "[a-z]+",
    "(?P<n>x)(?P=n)",
    "\\d{2,3}",
    "a{3,}?",
    "(?i)AbC",
    "(",
    "*",
    "\\",
    "x(?=y)",
    "(?<!a)b",
    "\\bword\\b",
    ".*",
    "2024-01-31",
    "2024-01-31T12:34:56",
    "2024-02-29 23:59:59.999999+01:00",
    "%Y-%m-%d",
    "%H:%M:%S",
    "%a %b %d %j",
    "utf-8",
    "ascii",
    "utf-16",
    "utf-32",
    "latin-1",
    "strict",
    "ignore",
    "replace",
    "backslashreplace",
    "md5",
    "sha256",
    "NFC",
    "NFKD",
    "LATIN SMALL LETTER A",
    "a/b/../c",
    "/usr/lib/../bin/",
    "*.py",
    "[!a]*",
    "1.10",
    "-0",
    "NaN",
    "sNaN",
    "Infinity",
    "1E+2",
    "0.000001",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
];

/// Characters for strings built from fuzzer bytes.
pub const STR_CHARS: &[char] = &[
    'a',
    'b',
    'A',
    ' ',
    '1',
    '0',
    '.',
    ',',
    '-',
    '_',
    '\n',
    '\t',
    '\0',
    'é',
    'İ',
    'ß',
    'Σ',
    '😀',
    '\u{301}',
    '%',
    '{',
    '}',
    '\\',
    '\'',
    '"',
    'x',
    'z',
    '\u{10ffff}',
    '\u{fffd}',
    '\u{7f}',
    '/',
    ':',
];

/// Regular expressions (also part of [`STRS`]), used for `re` pattern arguments.
pub const REGEXES: &[&str] = &[
    "a",
    "(a|b)*c",
    "^\\w+$",
    "[a-z]+",
    "(?P<n>x)(?P=n)",
    "\\d{2,3}",
    "a{3,}?",
    "(?i)AbC",
    "(",
    "x(?=y)",
    "(?<!a)b",
    "\\bword\\b",
    ".*",
    "(a)(b)?(c)",
    "",
    "\\s+",
];

/// Bytes values.
pub const BYTES: &[&[u8]] = &[
    b"",
    b"a",
    b"abc",
    b"hello world",
    b"  x  ",
    b"\x00\xff",
    b"\x80abc",
    b"%s %d",
    b"{}",
    b"a,b,c",
    b"ABC",
    b"aBc",
    b"123",
    b"\n\r\t",
    b"\xe2\x82\xac",
    b"\xff\xfe",
    b"YWJj",
    b"6162",
    b"<~9jqo^~>",
    b"=3D",
];

/// Text for f-string literal parts.
pub const FSTRING_TEXT: &[&str] = &["", "a", " ", "x=", ", ", "{", "}", "\\n", "é"];

/// Format specs, for f-strings and `format()`.
pub const FORMAT_SPECS: &[&str] = &[
    "", "d", "x", "#x", "08.3f", ">10", "<5", "^9", "=+8", "+,d", "_b", ",.2%", ".0e", "g", "G", ".100f", "1000", "c",
    "s", ".3s", "z.1f", "#o", "n", " d", "010", ".5", "e", "%", "X", "#_X", "*^20", "0=5",
];

/// Characters that end a format spec (the presentation type).
pub const FORMAT_TYPES: &[char] = &[
    'b', 'c', 'd', 'e', 'E', 'f', 'F', 'g', 'G', 'n', 'o', 's', 'x', 'X', '%',
];

/// Fill characters for format specs (never braces or quotes).
pub const FILL_CHARS: &[char] = &['*', '0', ' ', 'x', '-', '#', '_', '~', 'é'];

/// `%`-format strings.
pub const PERCENT_FORMATS: &[&str] = &[
    "%s",
    "%d",
    "%5.2f",
    "%-10s|",
    "%x %o %e",
    "%(a)s",
    "%r %%",
    "%c",
    "%*d",
    "%.*f",
    "%s %s",
    "%+08.3g",
    "%i%",
    "%(a)d %(b)s",
];

/// Strings with `{}` replacement fields, for `str.format`.
pub const BRACE_FORMATS: &[&str] = &[
    "{}",
    "{0}",
    "{0!r:>10}",
    "{:,}",
    "{a}",
    "{0}{1}{0}",
    "{:%Y}",
    "{{}}",
    "{:^{}}",
    "{0[0]}",
    "{0.real}",
    "{!s:*<8}",
    "{:.{}f}",
    "}",
];

/// JSON documents for `json.loads`.
pub const JSON_TEXTS: &[&str] = &[
    "[1, 2, 3]",
    "{\"a\": [1, null, true, 1.5e300]}",
    "\"\\ud800\"",
    "1e400",
    "[[[[]]]]",
    "{\"a\": 1, \"a\": 2}",
    "-0",
    "NaN",
    "[1,]",
    "\"\\u00e9\"",
];

/// TOML documents for `tomllib.loads`.
pub const TOML_TEXTS: &[&str] = &[
    "a = 1\nb = 'x'",
    "[t]\nk = [1, {x = 2}]",
    "d = 1979-05-27T07:32:00Z",
    "f = inf\ng = nan",
    "x = 0x_1",
    "a.b.c = 1",
    "s = \"\"\"\nmulti\"\"\"",
];

/// Values for date/time constructor arguments.
pub const DATE_INTS: &[i128] = &[1, 2, 12, 28, 29, 30, 31, 0, 13, 9999, 10000, 2024, 1970, 59, 60, 23, 24];

/// Mapping-pattern keys; all distinct so no pattern repeats a key.
pub const PATTERN_KEYS: &[PatternKey] = &[
    PatternKey::Int(0),
    PatternKey::Int(1),
    PatternKey::Int(2),
    PatternKey::Str("a"),
    PatternKey::Str("b"),
    PatternKey::Str("c"),
];

/// A literal key in a mapping pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatternKey {
    Int(i128),
    Str(&'static str),
}
