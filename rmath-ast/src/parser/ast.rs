use ordered_float::OrderedFloat;
use yajlish_ndjson::Selector;

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Eq, Ord)]
pub enum Num {
    PosInt(u64),
    Int(i64),
    Float(OrderedFloat<f64>),
}

#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord)]
pub enum Value {
    Bool(bool),
    String(String),
    Num(Num),
    Null,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Term {
    Value(Value),
    Selector(Vec<Selector>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Term(Term),
    Op(Box<Expr>, BinaryOp, Box<Expr>),
    /// A function call. The name is resolved against a
    /// [`FunctionRegistry`](crate::registry::FunctionRegistry) after parsing.
    Call(String, Vec<Expr>),
    Negate(Box<Expr>),
    /// `left <op> right`. Comparisons don't chain: `a < b < c` is a parse
    /// error.
    Compare(Box<Expr>, CompareOp, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    /// `e is null` (`negated: false`) or `e is not null`.
    IsNull {
        expr: Box<Expr>,
        negated: bool,
    },
    /// `e in [lit, ...]` or `e not in [...]`.
    In {
        expr: Box<Expr>,
        list: Vec<Value>,
        negated: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Div,
    Mul,
    Pow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// One item of a `select` list.
#[derive(Debug, PartialEq)]
pub enum SelectItem {
    /// `*` or `* except (d.a, d.b.c)`: the whole record, minus the given
    /// paths.
    Star { except: Vec<Vec<Selector>> },
    /// `expr` or `expr as name`. `span` is the expression's byte range in
    /// the source text.
    Expr {
        expr: Expr,
        alias: Option<String>,
        span: (usize, usize),
    },
}
