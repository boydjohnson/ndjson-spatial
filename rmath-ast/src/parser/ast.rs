use ordered_float::OrderedFloat;
use yajlish::ndjson_handler::Selector;

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
    UnaryFunction(UnaryFunction, Box<Expr>),
}

#[derive(Debug, PartialEq)]
pub enum UnaryFunction {
    Abs,
    Sqrt,
    Ceiling,
    Floor,
    Ceil,
    Trunc,
    Cos,
    Sin,
    Tan,
    ACos,
    ASin,
    ATan,
    Cosh,
    Sinh,
    Tanh,
    ACosh,
    ASinh,
    ATanh,
    Log,
    Log10,
    Exp,
    ToUpper,
    ToLower,
    Capitalize,
    ToString,
    IsFloat,
    IsFloatNan,
    IsNull,
    ToFloat,
    ToInt,
}

#[derive(Debug, PartialEq)]
pub enum BinaryOp {
    Add,
    Sub,
    Div,
    Mul,
    Pow,
}
