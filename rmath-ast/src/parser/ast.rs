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

#[derive(Debug, PartialEq)]
pub enum Term {
    Value(Value),
    Selector(Vec<Selector>),
}

#[derive(Debug, PartialEq)]
pub enum Expr {
    Term(Term),
    Op(Box<Expr>, BinaryOp, Box<Expr>),
    /// A function call. The name is resolved against a
    /// [`FunctionRegistry`](crate::registry::FunctionRegistry) after parsing.
    Call(String, Vec<Expr>),
    Negate(Box<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Div,
    Mul,
    Pow,
}
