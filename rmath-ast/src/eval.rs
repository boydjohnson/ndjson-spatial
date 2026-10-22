//! Evaluate a [`CompiledExpr`](crate::registry::CompiledExpr) against one JSON record.
//!
//! Semantics (see `plans/02-select-and-expressions.md`):
//!
//! - A selector whose path is missing, or that walks through a value of the
//!   wrong type, evaluates to `null`.
//! - `null` propagates: arithmetic and functions with a `null` argument
//!   return `null` (except functions registered with
//!   [`Function::sees_null`](crate::registry::Function::sees_null), e.g. `isnull`).
//! - Integer `+ - *` stay integers when the result fits in `i64`/`u64`, and
//!   become floats otherwise. `/` always yields a float. `**` yields an
//!   integer when both sides are integers, the exponent is non-negative, and
//!   the result fits.
//! - `+` on two strings concatenates.
//! - Comparisons always yield a bool. `==` and `!=` compare numbers by value
//!   (`1 == 1.0`) and everything else structurally, so `d.a == null` is true
//!   for a missing `a`. `< <= > >=` order two numbers, two strings, or two
//!   bools, and are false for any other pair (including `null`).
//! - `x in [a, b]` is `x == a or x == b`; `is null` / `is not null` test for
//!   `null` (or a missing path).
//! - `and`, `or`, and `not` take bools and use three-valued logic for
//!   `null`: `null and false` is `false`, `null or true` is `true`, anything
//!   else involving `null` is `null`. They short-circuit, so the right side
//!   of `false and ...` is not evaluated.
//! - Anything else (e.g. `"abc" * 2`, dividing by zero, `sqrt(-1)`) is an
//!   [`EvalError`]. Callers decide whether that becomes `null` or aborts.
//!   JSON has no NaN or infinity, so results that aren't finite are errors.

use crate::parser::ast::{BinaryOp, CompareOp, Num, Value as Literal};
use serde_json::{Number, Value};
use std::{cmp::Ordering, fmt};
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
    /// A predicate (e.g. a `filter` expression) produced something other than
    /// a bool or `null`.
    NotBool {
        got: &'static str,
    },
    /// An aggregate function was called on one record's values.
    NotScalar {
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
            EvalError::NotBool { got } => write!(f, "expected a bool, got {}", got),
            EvalError::NotScalar { func } => {
                write!(f, "{}() is an aggregate and needs a group of records", func)
            }
        }
    }
}

impl std::error::Error for EvalError {}

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

pub(crate) fn literal_to_json(lit: &Literal) -> Value {
    match lit {
        Literal::Bool(b) => Value::Bool(*b),
        Literal::String(s) => Value::String(s.clone()),
        Literal::Null => Value::Null,
        Literal::Num(n) => num_to_json(*n).unwrap_or(Value::Null),
    }
}

pub(crate) fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

pub(crate) fn json_to_num(n: &Number) -> Num {
    if let Some(u) = n.as_u64() {
        Num::PosInt(u)
    } else if let Some(i) = n.as_i64() {
        Num::Int(i)
    } else {
        Num::Float(n.as_f64().unwrap_or(f64::NAN).into())
    }
}

pub(crate) fn num_to_json(n: Num) -> Option<Value> {
    match n {
        Num::PosInt(u) => Some(Value::from(u)),
        Num::Int(i) => Some(Value::from(i)),
        Num::Float(f) => Number::from_f64(f.0).map(Value::Number),
    }
}

pub(crate) fn float_result(op: &'static str, f: f64) -> Result<Value, EvalError> {
    Number::from_f64(f)
        .map(Value::Number)
        .ok_or(EvalError::NonFinite { op })
}

/// An integer result if it fits in `u64`/`i64`, else a float.
pub(crate) fn int_result(op: &'static str, i: i128) -> Result<Value, EvalError> {
    if let Ok(u) = u64::try_from(i) {
        Ok(Value::from(u))
    } else if let Ok(s) = i64::try_from(i) {
        Ok(Value::from(s))
    } else {
        float_result(op, i as f64)
    }
}

pub(crate) fn as_int(n: Num) -> Option<i128> {
    match n {
        Num::PosInt(u) => Some(i128::from(u)),
        Num::Int(i) => Some(i128::from(i)),
        Num::Float(_) => None,
    }
}

pub(crate) fn as_f64(n: Num) -> f64 {
    match n {
        Num::PosInt(u) => u as f64,
        Num::Int(i) => i as f64,
        Num::Float(f) => f.0,
    }
}

pub(crate) fn negate(v: Value) -> Result<Value, EvalError> {
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

pub(crate) fn binary(op: BinaryOp, left: Value, right: Value) -> Result<Value, EvalError> {
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

/// `left <op> right`; see the module docs.
pub(crate) fn compare(op: CompareOp, left: &Value, right: &Value) -> bool {
    match op {
        CompareOp::Eq => json_eq(left, right),
        CompareOp::Ne => !json_eq(left, right),
        CompareOp::Lt => json_order(left, right) == Some(Ordering::Less),
        CompareOp::Le => matches!(
            json_order(left, right),
            Some(Ordering::Less | Ordering::Equal)
        ),
        CompareOp::Gt => json_order(left, right) == Some(Ordering::Greater),
        CompareOp::Ge => matches!(
            json_order(left, right),
            Some(Ordering::Greater | Ordering::Equal)
        ),
    }
}

/// Structural equality, except that numbers compare by value.
pub(crate) fn json_eq(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => {
            num_order(json_to_num(a), json_to_num(b)) == Some(Ordering::Equal)
        }
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| json_eq(x, y))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(k, x)| b.get(k).is_some_and(|y| json_eq(x, y)))
        }
        (a, b) => a == b,
    }
}

/// The order of two numbers, two strings, or two bools; `None` otherwise.
pub(crate) fn json_order(left: &Value, right: &Value) -> Option<Ordering> {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => num_order(json_to_num(a), json_to_num(b)),
        (Value::String(a), Value::String(b)) => Some(a.cmp(b)),
        (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)),
        _ => None,
    }
}

/// Integers compare exactly; a float on either side compares as `f64`.
fn num_order(a: Num, b: Num) -> Option<Ordering> {
    match as_int(a).zip(as_int(b)) {
        Some((x, y)) => Some(x.cmp(&y)),
        None => as_f64(a).partial_cmp(&as_f64(b)),
    }
}

/// An operand of `and`, `or`, or `not`: a bool, or `None` for `null`.
pub(crate) fn logic_operand(op: &'static str, v: &Value) -> Result<Option<bool>, EvalError> {
    match v {
        Value::Bool(b) => Ok(Some(*b)),
        Value::Null => Ok(None),
        other => Err(EvalError::Type {
            op,
            left: type_name(other),
            right: None,
        }),
    }
}

pub(crate) fn logic_result(b: Option<bool>) -> Value {
    b.map_or(Value::Null, Value::Bool)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::FunctionRegistry;
    use serde_json::json;

    fn eval(expr: &str, record: Value) -> Result<Value, EvalError> {
        FunctionRegistry::default()
            .compile(expr)
            .unwrap_or_else(|e| panic!("failed to compile {:?}: {}", expr, e))
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
    #[test]
    fn test_comparisons() {
        let r = json!({"i": 3, "f": 3.0, "s": "b", "t": true, "n": null, "arr": [1, 2.0]});
        assert_eq!(ok("d.i == 3", r.clone()), json!(true));
        assert_eq!(ok("d.i == d.f", r.clone()), json!(true));
        assert_eq!(ok("d.i != 3.5", r.clone()), json!(true));
        assert_eq!(ok("d.i < 3.5", r.clone()), json!(true));
        assert_eq!(ok("d.i <= 3", r.clone()), json!(true));
        assert_eq!(ok("d.i > -1", r.clone()), json!(true));
        assert_eq!(ok("d.i >= 4", r.clone()), json!(false));
        assert_eq!(ok("d.i * 2 > d.f + 2", r.clone()), json!(true));
        assert_eq!(ok(r#"d.s > "a""#, r.clone()), json!(true));
        assert_eq!(ok(r#"d.s == "b""#, r.clone()), json!(true));
        assert_eq!(ok("d.t > false", r.clone()), json!(true));
        assert_eq!(ok("d.t == true", r.clone()), json!(true));
        assert_eq!(ok("d.arr == d.arr", r.clone()), json!(true));
        // big integers compare exactly
        let big = json!({"a": u64::MAX, "b": i64::MIN});
        assert_eq!(ok("d.a > d.a - 1", big.clone()), json!(true));
        assert_eq!(ok("d.b < 0", big), json!(true));
    }

    #[test]
    fn test_comparing_mismatched_types() {
        let r = json!({"i": 3, "s": "3", "n": null});
        // ordering across types is false either way
        assert_eq!(ok("d.s < d.i", r.clone()), json!(false));
        assert_eq!(ok("d.s >= d.i", r.clone()), json!(false));
        assert_eq!(ok("d.n < 1", r.clone()), json!(false));
        assert_eq!(ok("d.n >= 1", r.clone()), json!(false));
        // and equality is false
        assert_eq!(ok("d.s == d.i", r.clone()), json!(false));
        assert_eq!(ok("d.s != d.i", r.clone()), json!(true));
        // null is equal only to null (or a missing path)
        assert_eq!(ok("d.n == null", r.clone()), json!(true));
        assert_eq!(ok("d.missing == null", r.clone()), json!(true));
        assert_eq!(ok("d.i == null", r.clone()), json!(false));
        assert_eq!(ok("d.i != null", r), json!(true));
    }

    #[test]
    fn test_is_null_and_in() {
        let r = json!({"a": null, "s": "WI", "i": 2});
        assert_eq!(ok("d.a is null", r.clone()), json!(true));
        assert_eq!(ok("d.missing is null", r.clone()), json!(true));
        assert_eq!(ok("d.s is not null", r.clone()), json!(true));
        assert_eq!(ok(r#"d.s in ["MN", "WI"]"#, r.clone()), json!(true));
        assert_eq!(ok(r#"d.s not in ["MN", "WI"]"#, r.clone()), json!(false));
        assert_eq!(ok("d.i in [1, 2.0]", r.clone()), json!(true));
        assert_eq!(ok("d.i + 1 in [3]", r.clone()), json!(true));
        assert_eq!(ok("d.a in [null]", r.clone()), json!(true));
        assert_eq!(ok(r#"d.a in ["x"]"#, r.clone()), json!(false));
        assert_eq!(ok("d.i in []", r), json!(false));
    }

    #[test]
    fn test_and_or_not() {
        let r = json!({"t": true, "f": false, "n": null, "i": 1});
        assert_eq!(ok("d.t and d.t", r.clone()), json!(true));
        assert_eq!(ok("d.t and d.f", r.clone()), json!(false));
        assert_eq!(ok("d.f or d.t", r.clone()), json!(true));
        assert_eq!(ok("d.f or d.f", r.clone()), json!(false));
        assert_eq!(ok("not d.f", r.clone()), json!(true));
        assert_eq!(ok("d.i == 1 and not d.f", r.clone()), json!(true));
        // three-valued logic
        assert_eq!(ok("d.n and d.f", r.clone()), json!(false));
        assert_eq!(ok("d.f and d.n", r.clone()), json!(false));
        assert_eq!(ok("d.n and d.t", r.clone()), Value::Null);
        assert_eq!(ok("d.n or d.t", r.clone()), json!(true));
        assert_eq!(ok("d.t or d.n", r.clone()), json!(true));
        assert_eq!(ok("d.n or d.f", r.clone()), Value::Null);
        assert_eq!(ok("not d.n", r), Value::Null);
    }

    #[test]
    fn test_and_or_short_circuit() {
        let r = json!({"zero": 0, "s": "x"});
        // the right side would be an error
        assert_eq!(
            ok("d.zero != 0 and 1 / d.zero > 1", r.clone()),
            json!(false)
        );
        assert_eq!(ok("d.zero == 0 or d.s * 2", r.clone()), json!(true));
        assert_eq!(
            eval("d.zero == 0 and 1 / d.zero > 1", r),
            Err(EvalError::DivisionByZero)
        );
    }

    #[test]
    fn test_logic_needs_bools() {
        let r = json!({"i": 1, "s": "x"});
        assert_eq!(
            eval("d.i and true", r.clone()),
            Err(EvalError::Type {
                op: "and",
                left: "number",
                right: None
            })
        );
        assert_eq!(
            eval("false or d.s", r.clone()),
            Err(EvalError::Type {
                op: "or",
                left: "string",
                right: None
            })
        );
        assert_eq!(
            eval("not d.s", r),
            Err(EvalError::Type {
                op: "not",
                left: "string",
                right: None
            })
        );
    }

    #[test]
    fn test_eval_bool() {
        let registry = FunctionRegistry::default();
        let r = json!({"a": 1, "n": null});
        let pred = |src: &str| registry.compile(src).unwrap().eval_bool(&r);
        assert_eq!(pred("d.a == 1"), Ok(true));
        assert_eq!(pred("d.a == 2"), Ok(false));
        assert_eq!(pred("d.n and true"), Ok(false));
        assert_eq!(pred("d.a + 1"), Err(EvalError::NotBool { got: "number" }));
        assert_eq!(
            EvalError::NotBool { got: "number" }.to_string(),
            "expected a bool, got number"
        );
    }
}
