//! Evaluate a parsed [`Expr`] against one JSON record.
//!
//! Semantics (see `plans/02-select-and-expressions.md`):
//!
//! - A selector whose path is missing, or that walks through a value of the
//!   wrong type, evaluates to `null`.
//! - `null` propagates: arithmetic and functions with a `null` argument
//!   return `null` (except `isnull`).
//! - Integer `+ - *` stay integers when the result fits in `i64`/`u64`, and
//!   become floats otherwise. `/` always yields a float. `**` yields an
//!   integer when both sides are integers, the exponent is non-negative, and
//!   the result fits.
//! - `+` on two strings concatenates.
//! - Anything else (e.g. `"abc" * 2`, dividing by zero, `sqrt(-1)`) is an
//!   [`EvalError`]. Callers decide whether that becomes `null` or aborts.
//!   JSON has no NaN or infinity, so results that aren't finite are errors.

use crate::parser::ast::{BinaryOp, Expr, Num, Term, UnaryFunction, Value as Literal};
use serde_json::{Number, Value};
use std::fmt;
use yajlish_ndjson::Selector;

#[derive(Debug, Clone, PartialEq)]
pub enum EvalError {
    /// An operator or function was applied to values of the wrong type.
    Type {
        op: &'static str,
        left: &'static str,
        right: Option<&'static str>,
    },
    DivisionByZero,
    /// The result is NaN or infinite, which JSON cannot represent.
    NonFinite {
        op: &'static str,
    },
    /// A string could not be converted (`tofloat("abc")`).
    Parse {
        func: &'static str,
        input: String,
    },
    /// A number is outside the range of the target type (`toint(1e300)`).
    OutOfRange {
        func: &'static str,
    },
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EvalError::Type {
                op,
                left,
                right: Some(right),
            } => write!(f, "cannot apply '{}' to {} and {}", op, left, right),
            EvalError::Type {
                op,
                left,
                right: None,
            } => write!(f, "cannot apply '{}' to {}", op, left),
            EvalError::DivisionByZero => write!(f, "division by zero"),
            EvalError::NonFinite { op } => write!(f, "'{}' produced a non-finite number", op),
            EvalError::Parse { func, input } => {
                write!(f, "{}: cannot convert {:?}", func, input)
            }
            EvalError::OutOfRange { func } => write!(f, "{}: number out of range", func),
        }
    }
}

impl std::error::Error for EvalError {}

impl Expr {
    /// Evaluate this expression against `record` (the `d` in selectors).
    pub fn eval(&self, record: &Value) -> Result<Value, EvalError> {
        match self {
            Expr::Term(Term::Value(lit)) => Ok(literal_to_json(lit)),
            Expr::Term(Term::Selector(path)) => {
                Ok(select(record, path).cloned().unwrap_or(Value::Null))
            }
            Expr::Negate(inner) => negate(inner.eval(record)?),
            Expr::Op(left, op, right) => binary(*op, left.eval(record)?, right.eval(record)?),
            Expr::UnaryFunction(func, arg) => unary_function(func, arg.eval(record)?),
        }
    }
}

/// Walk `path` into `record`. `None` when a key or index is missing or the
/// path goes through a scalar.
pub fn select<'a>(record: &'a Value, path: &[Selector]) -> Option<&'a Value> {
    path.iter()
        .try_fold(record, |value, step| match (step, value) {
            (Selector::Identifier(key), Value::Object(map)) => map.get(key),
            (Selector::Index(i), Value::Array(items)) => items.get(*i),
            _ => None,
        })
}

fn literal_to_json(lit: &Literal) -> Value {
    match lit {
        Literal::Bool(b) => Value::Bool(*b),
        Literal::String(s) => Value::String(s.clone()),
        Literal::Null => Value::Null,
        Literal::Num(n) => num_to_json(*n).unwrap_or(Value::Null),
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn json_to_num(n: &Number) -> Num {
    if let Some(u) = n.as_u64() {
        Num::PosInt(u)
    } else if let Some(i) = n.as_i64() {
        Num::Int(i)
    } else {
        Num::Float(n.as_f64().unwrap_or(f64::NAN).into())
    }
}

fn num_to_json(n: Num) -> Option<Value> {
    match n {
        Num::PosInt(u) => Some(Value::from(u)),
        Num::Int(i) => Some(Value::from(i)),
        Num::Float(f) => Number::from_f64(f.0).map(Value::Number),
    }
}

fn float_result(op: &'static str, f: f64) -> Result<Value, EvalError> {
    Number::from_f64(f)
        .map(Value::Number)
        .ok_or(EvalError::NonFinite { op })
}

/// An integer result if it fits in `u64`/`i64`, else a float.
fn int_result(op: &'static str, i: i128) -> Result<Value, EvalError> {
    if let Ok(u) = u64::try_from(i) {
        Ok(Value::from(u))
    } else if let Ok(s) = i64::try_from(i) {
        Ok(Value::from(s))
    } else {
        float_result(op, i as f64)
    }
}

fn as_int(n: Num) -> Option<i128> {
    match n {
        Num::PosInt(u) => Some(i128::from(u)),
        Num::Int(i) => Some(i128::from(i)),
        Num::Float(_) => None,
    }
}

fn as_f64(n: Num) -> f64 {
    match n {
        Num::PosInt(u) => u as f64,
        Num::Int(i) => i as f64,
        Num::Float(f) => f.0,
    }
}

fn negate(v: Value) -> Result<Value, EvalError> {
    match &v {
        Value::Null => Ok(Value::Null),
        Value::Number(n) => match json_to_num(n) {
            Num::Float(f) => float_result("-", -f.0),
            n => int_result("-", -as_int(n).expect("integer")),
        },
        other => Err(EvalError::Type {
            op: "-",
            left: type_name(other),
            right: None,
        }),
    }
}

fn binary_op_name(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Pow => "**",
    }
}

fn binary(op: BinaryOp, left: Value, right: Value) -> Result<Value, EvalError> {
    let name = binary_op_name(op);
    match (&left, &right) {
        (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
        (Value::String(a), Value::String(b)) if op == BinaryOp::Add => {
            Ok(Value::String(format!("{}{}", a, b)))
        }
        (Value::Number(a), Value::Number(b)) => numeric(op, json_to_num(a), json_to_num(b)),
        (l, r) => Err(EvalError::Type {
            op: name,
            left: type_name(l),
            right: Some(type_name(r)),
        }),
    }
}

fn numeric(op: BinaryOp, a: Num, b: Num) -> Result<Value, EvalError> {
    let name = binary_op_name(op);
    let ints = as_int(a).zip(as_int(b));
    match op {
        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul => {
            if let Some((x, y)) = ints {
                // i128 cannot overflow for operands in the i64 ∪ u64 range
                // under + and -; * can, so it is checked.
                let r = match op {
                    BinaryOp::Add => Some(x + y),
                    BinaryOp::Sub => Some(x - y),
                    _ => x.checked_mul(y),
                };
                match r {
                    Some(r) => int_result(name, r),
                    None => float_result(name, as_f64(a) * as_f64(b)),
                }
            } else {
                let (x, y) = (as_f64(a), as_f64(b));
                let r = match op {
                    BinaryOp::Add => x + y,
                    BinaryOp::Sub => x - y,
                    _ => x * y,
                };
                float_result(name, r)
            }
        }
        BinaryOp::Div => {
            let y = as_f64(b);
            if y == 0.0 {
                Err(EvalError::DivisionByZero)
            } else {
                float_result(name, as_f64(a) / y)
            }
        }
        BinaryOp::Pow => {
            if let Some((x, y)) = ints {
                if let Ok(exp) = u32::try_from(y) {
                    if let Some(r) = x.checked_pow(exp) {
                        return int_result(name, r);
                    }
                }
            }
            float_result(name, as_f64(a).powf(as_f64(b)))
        }
    }
}

fn function_name(func: &UnaryFunction) -> &'static str {
    use UnaryFunction::*;
    match func {
        Abs => "abs",
        Sqrt => "sqrt",
        Floor => "floor",
        Ceil => "ceil",
        Trunc => "trunc",
        Cos => "cos",
        Sin => "sin",
        Tan => "tan",
        ACos => "acos",
        ASin => "asin",
        ATan => "atan",
        Cosh => "cosh",
        Sinh => "sinh",
        Tanh => "tanh",
        ACosh => "acosh",
        ASinh => "asinh",
        ATanh => "atanh",
        Log => "log",
        Log10 => "log10",
        Exp => "exp",
        ToUpper => "toupper",
        ToLower => "tolower",
        Capitalize => "capitalize",
        ToString => "tostring",
        IsFloat => "isfloat",
        IsFloatNan => "isfloatnan",
        IsNull => "isnull",
        ToFloat => "tofloat",
        ToInt => "toint",
    }
}

fn unary_function(func: &UnaryFunction, arg: Value) -> Result<Value, EvalError> {
    use UnaryFunction::*;
    let name = function_name(func);

    // Predicates see null; everything else propagates it.
    match func {
        IsNull => return Ok(Value::Bool(arg.is_null())),
        IsFloat => return Ok(Value::Bool(parse_float(&arg).is_some())),
        IsFloatNan => {
            return Ok(Value::Bool(
                parse_float(&arg).map(f64::is_nan).unwrap_or(false),
            ))
        }
        _ => {}
    }
    if arg.is_null() {
        return Ok(Value::Null);
    }

    let wrong_type = |v: &Value| EvalError::Type {
        op: name,
        left: type_name(v),
        right: None,
    };

    match func {
        ToUpper | ToLower | Capitalize => match &arg {
            Value::String(s) => Ok(Value::String(match func {
                ToUpper => s.to_uppercase(),
                ToLower => s.to_lowercase(),
                _ => capitalize(s),
            })),
            other => Err(wrong_type(other)),
        },
        ToString => Ok(match arg {
            Value::String(s) => Value::String(s),
            other => Value::String(other.to_string()),
        }),
        ToFloat => match parse_float(&arg) {
            Some(f) => float_result(name, f),
            None => Err(conversion_error(name, &arg)),
        },
        ToInt => to_int(&arg),
        _ => {
            let n = match &arg {
                Value::Number(n) => json_to_num(n),
                other => return Err(wrong_type(other)),
            };
            math_function(func, name, n)
        }
    }
}

fn math_function(func: &UnaryFunction, name: &'static str, n: Num) -> Result<Value, EvalError> {
    use UnaryFunction::*;
    match func {
        Abs => match n {
            Num::Float(f) => float_result(name, f.0.abs()),
            n => int_result(name, as_int(n).expect("integer").abs()),
        },
        // Rounding an integer is the identity; rounding a float yields an
        // integer when it fits, so results can be used as grouping keys.
        Floor | Ceil | Trunc => match n {
            Num::Float(f) => {
                let r = match func {
                    Floor => f.0.floor(),
                    Ceil => f.0.ceil(),
                    _ => f.0.trunc(),
                };
                float_to_int_if_exact(name, r)
            }
            n => num_to_json(n).ok_or(EvalError::NonFinite { op: name }),
        },
        _ => {
            let x = as_f64(n);
            let r = match func {
                Sqrt => x.sqrt(),
                Cos => x.cos(),
                Sin => x.sin(),
                Tan => x.tan(),
                ACos => x.acos(),
                ASin => x.asin(),
                ATan => x.atan(),
                Cosh => x.cosh(),
                Sinh => x.sinh(),
                Tanh => x.tanh(),
                ACosh => x.acosh(),
                ASinh => x.asinh(),
                ATanh => x.atanh(),
                Log => x.ln(),
                Log10 => x.log10(),
                Exp => x.exp(),
                _ => unreachable!("non-math functions are handled by the caller"),
            };
            float_result(name, r)
        }
    }
}

/// Whether an integer-valued float lies in the i64 ∪ u64 range
/// (`[-2^63, 2^64)`), so `as i128` is exact and the result fits.
fn in_int_range(f: f64) -> bool {
    (-9_223_372_036_854_775_808.0..18_446_744_073_709_551_616.0).contains(&f)
}

/// `r` must already be integer-valued.
fn float_to_int_if_exact(op: &'static str, r: f64) -> Result<Value, EvalError> {
    if in_int_range(r) {
        int_result(op, r as i128)
    } else {
        float_result(op, r)
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Numbers, and strings that parse as numbers (surrounding whitespace
/// allowed; `"NaN"`/`"inf"` parse to their float values).
fn parse_float(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => Some(as_f64(json_to_num(n))),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn conversion_error(func: &'static str, v: &Value) -> EvalError {
    match v {
        Value::String(s) => EvalError::Parse {
            func,
            input: s.clone(),
        },
        other => EvalError::Type {
            op: func,
            left: type_name(other),
            right: None,
        },
    }
}

/// Integers pass through; floats and numeric strings truncate toward zero.
fn to_int(v: &Value) -> Result<Value, EvalError> {
    const NAME: &str = "toint";
    match v {
        Value::Number(n) => match json_to_num(n) {
            Num::Float(f) => truncate_to_int(f.0),
            n => num_to_json(n).ok_or(EvalError::OutOfRange { func: NAME }),
        },
        Value::String(s) => {
            let t = s.trim();
            if let Ok(u) = t.parse::<u64>() {
                Ok(Value::from(u))
            } else if let Ok(i) = t.parse::<i64>() {
                Ok(Value::from(i))
            } else if let Ok(f) = t.parse::<f64>() {
                truncate_to_int(f)
            } else {
                Err(conversion_error(NAME, v))
            }
        }
        other => Err(conversion_error(NAME, other)),
    }
}

fn truncate_to_int(f: f64) -> Result<Value, EvalError> {
    let t = f.trunc();
    if in_int_range(t) {
        int_result("toint", t as i128)
    } else {
        Err(EvalError::OutOfRange { func: "toint" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::ExprParser;
    use serde_json::json;

    fn eval(expr: &str, record: Value) -> Result<Value, EvalError> {
        ExprParser::new()
            .parse(expr)
            .unwrap_or_else(|e| panic!("failed to parse {:?}: {:?}", expr, e))
            .eval(&record)
    }

    fn ok(expr: &str, record: Value) -> Value {
        eval(expr, record).unwrap_or_else(|e| panic!("{:?} failed: {}", expr, e))
    }

    #[test]
    fn test_selectors() {
        let r = json!({"a": {"b": [10, {"c": "x"}]}, "key with space": 1});
        assert_eq!(ok("d.a.b[0]", r.clone()), json!(10));
        assert_eq!(ok("d.a.b[1].c", r.clone()), json!("x"));
        assert_eq!(ok(r#"d."key with space""#, r.clone()), json!(1));
        assert_eq!(ok("d", r.clone()), r.clone());
        // missing keys, out-of-range indexes, and walking through scalars
        assert_eq!(ok("d.missing", r.clone()), Value::Null);
        assert_eq!(ok("d.a.b[5]", r.clone()), Value::Null);
        assert_eq!(ok("d.a.b[0].c", r.clone()), Value::Null);
        assert_eq!(ok("d.a[0]", r), Value::Null);
    }

    #[test]
    fn test_integer_arithmetic_stays_integer() {
        let r = json!({"price": 3, "qty": 4});
        assert_eq!(ok("d.price * d.qty", r.clone()), json!(12));
        assert_eq!(ok("d.price - d.qty", r.clone()), json!(-1));
        assert_eq!(ok("3-5", r.clone()), json!(-2));
        assert_eq!(ok("2 ** 10", r.clone()), json!(1024));
        assert_eq!(ok("-2 ** 2", r.clone()), json!(-4));
        assert_eq!(ok("(-2) ** 3", r), json!(-8));
    }

    #[test]
    fn test_integer_overflow_promotes_to_float() {
        let r = json!({"big": u64::MAX});
        assert_eq!(ok("d.big + 1", r.clone()), json!(18446744073709551616.0));
        assert_eq!(
            ok("d.big * d.big", r.clone()),
            json!((u64::MAX as f64) * (u64::MAX as f64))
        );
        assert_eq!(ok("2 ** 100", r), json!(2f64.powi(100)));
    }

    #[test]
    fn test_float_arithmetic() {
        let r = json!({"a": 1.5, "b": 2});
        assert_eq!(ok("d.a + d.b", r.clone()), json!(3.5));
        assert_eq!(ok("d.b / 4", r.clone()), json!(0.5));
        assert_eq!(ok("4 / 2", r.clone()), json!(2.0));
        assert_eq!(ok("2 ** (-1)", r.clone()), json!(0.5));
        assert_eq!(ok("-d.a", r), json!(-1.5));
    }

    #[test]
    fn test_null_propagates() {
        let r = json!({"a": null, "b": 2});
        assert_eq!(ok("d.a + d.b", r.clone()), Value::Null);
        assert_eq!(ok("d.missing * 2", r.clone()), Value::Null);
        assert_eq!(ok("-d.a", r.clone()), Value::Null);
        assert_eq!(ok("sqrt(d.a)", r.clone()), Value::Null);
        assert_eq!(ok("toupper(d.a)", r.clone()), Value::Null);
        assert_eq!(ok("isnull(d.a)", r.clone()), json!(true));
        assert_eq!(ok("isnull(d.missing)", r.clone()), json!(true));
        assert_eq!(ok("isnull(d.b)", r), json!(false));
    }

    #[test]
    fn test_strings() {
        let r = json!({"first": "ada", "last": "LOVELACE", "n": 42});
        assert_eq!(
            ok(r#"d.first + " " + d.last"#, r.clone()),
            json!("ada LOVELACE")
        );
        assert_eq!(ok("toupper(d.first)", r.clone()), json!("ADA"));
        assert_eq!(ok("tolower(d.last)", r.clone()), json!("lovelace"));
        assert_eq!(ok("capitalize(d.first)", r.clone()), json!("Ada"));
        assert_eq!(ok("tostring(d.n)", r.clone()), json!("42"));
        assert_eq!(ok("tostring(d.first)", r), json!("ada"));
    }

    #[test]
    fn test_conversions() {
        let r = json!({"s": " 3.75 ", "i": "-12", "f": 2.9, "neg": -2.9, "word": "abc"});
        assert_eq!(ok("tofloat(d.s)", r.clone()), json!(3.75));
        assert_eq!(ok("toint(d.s)", r.clone()), json!(3));
        assert_eq!(ok("toint(d.i)", r.clone()), json!(-12));
        assert_eq!(ok("toint(d.f)", r.clone()), json!(2));
        assert_eq!(ok("toint(d.neg)", r.clone()), json!(-2));
        assert_eq!(ok("isfloat(d.s)", r.clone()), json!(true));
        assert_eq!(ok("isfloat(d.word)", r.clone()), json!(false));
        assert_eq!(ok(r#"isfloatnan("NaN")"#, r.clone()), json!(true));
        assert_eq!(
            eval("tofloat(d.word)", r.clone()),
            Err(EvalError::Parse {
                func: "tofloat",
                input: "abc".into()
            })
        );
        assert_eq!(
            eval("toint(1e300)", r),
            Err(EvalError::OutOfRange { func: "toint" })
        );
    }

    #[test]
    fn test_math_functions() {
        let r = json!({"x": -2.5, "i": -3});
        assert_eq!(ok("abs(d.x)", r.clone()), json!(2.5));
        assert_eq!(ok("abs(d.i)", r.clone()), json!(3));
        assert_eq!(ok("floor(d.x)", r.clone()), json!(-3));
        assert_eq!(ok("ceil(d.x)", r.clone()), json!(-2));
        assert_eq!(ok("trunc(d.x)", r.clone()), json!(-2));
        assert_eq!(ok("floor(d.i)", r.clone()), json!(-3));
        assert_eq!(ok("sqrt(16)", r.clone()), json!(4.0));
        assert_eq!(ok("exp(0)", r.clone()), json!(1.0));
        assert_eq!(ok("log10(1000)", r.clone()), json!(3.0));
        assert_eq!(ok("cosh(0)", r.clone()), json!(1.0));
        assert_eq!(ok("acosh(1)", r), json!(0.0));
    }

    #[test]
    fn test_errors() {
        let r = json!({"s": "abc", "b": true, "zero": 0});
        assert_eq!(
            eval("d.s * 2", r.clone()),
            Err(EvalError::Type {
                op: "*",
                left: "string",
                right: Some("number")
            })
        );
        assert_eq!(
            eval("-d.b", r.clone()),
            Err(EvalError::Type {
                op: "-",
                left: "bool",
                right: None
            })
        );
        assert_eq!(
            eval("1 / d.zero", r.clone()),
            Err(EvalError::DivisionByZero)
        );
        assert_eq!(
            eval("sqrt(-1)", r.clone()),
            Err(EvalError::NonFinite { op: "sqrt" })
        );
        assert_eq!(
            eval("toupper(d.zero)", r),
            Err(EvalError::Type {
                op: "toupper",
                left: "number",
                right: None
            })
        );
    }
}
