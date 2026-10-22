//! The built-in scalar functions, registered by
//! [`FunctionRegistry::default`](crate::registry::FunctionRegistry).
//!
//! Every function here takes one argument. Except for the predicates
//! (`isnull`, `isfloat`, `isfloatnan`), a `null` argument yields `null`.

use crate::{
    eval::{
        as_f64, as_int, float_result, int_result, json_to_num, num_to_json, type_name, EvalError,
    },
    parser::ast::Num,
    registry::{Arity, Function, FunctionRegistry},
};
use serde_json::Value;

type MathFn = (&'static str, fn(f64) -> f64);
type StringFn = (&'static str, fn(&str) -> String);

/// Number in, float out.
const FLOAT_MATH: &[MathFn] = &[
    ("sqrt", f64::sqrt),
    ("cos", f64::cos),
    ("sin", f64::sin),
    ("tan", f64::tan),
    ("acos", f64::acos),
    ("asin", f64::asin),
    ("atan", f64::atan),
    ("cosh", f64::cosh),
    ("sinh", f64::sinh),
    ("tanh", f64::tanh),
    ("acosh", f64::acosh),
    ("asinh", f64::asinh),
    ("atanh", f64::atanh),
    ("log", f64::ln),
    ("log10", f64::log10),
    ("exp", f64::exp),
];

/// Rounding an integer is the identity; rounding a float yields an integer
/// when it fits, so results can be used as grouping keys.
const ROUNDING: &[MathFn] = &[
    ("floor", f64::floor),
    ("ceil", f64::ceil),
    ("trunc", f64::trunc),
];

const STRING_CASE: &[StringFn] = &[
    ("toupper", str::to_uppercase),
    ("tolower", str::to_lowercase),
    ("capitalize", capitalize),
];

pub(crate) fn register(registry: &mut FunctionRegistry) {
    let one = Arity::exactly(1);

    for &(name, f) in FLOAT_MATH {
        registry.register(Function::scalar(name, one, move |args| {
            let x = as_f64(number(name, &args[0])?);
            float_result(name, f(x))
        }));
    }
    for &(name, f) in ROUNDING {
        registry.register(Function::scalar(name, one, move |args| {
            match number(name, &args[0])? {
                Num::Float(x) => float_to_int_if_exact(name, f(x.0)),
                n => num_to_json(n).ok_or(EvalError::NonFinite { op: name }),
            }
        }));
    }
    for &(name, f) in STRING_CASE {
        registry.register(Function::scalar(name, one, move |args| match &args[0] {
            Value::String(s) => Ok(Value::String(f(s))),
            other => Err(wrong_type(name, other)),
        }));
    }

    registry.register(Function::scalar("abs", one, |args| {
        match number("abs", &args[0])? {
            Num::Float(f) => float_result("abs", f.0.abs()),
            n => int_result("abs", as_int(n).expect("integer").abs()),
        }
    }));
    registry.register(Function::scalar("tostring", one, |args| {
        Ok(match &args[0] {
            Value::String(s) => Value::String(s.clone()),
            other => Value::String(other.to_string()),
        })
    }));
    registry.register(Function::scalar("tofloat", one, |args| {
        match parse_float(&args[0]) {
            Some(f) => float_result("tofloat", f),
            None => Err(conversion_error("tofloat", &args[0])),
        }
    }));
    registry.register(Function::scalar("toint", one, |args| to_int(&args[0])));

    registry.register(
        Function::scalar("isnull", one, |args| Ok(Value::Bool(args[0].is_null()))).sees_null(),
    );
    registry.register(
        Function::scalar("isfloat", one, |args| {
            Ok(Value::Bool(parse_float(&args[0]).is_some()))
        })
        .sees_null(),
    );
    registry.register(
        Function::scalar("isfloatnan", one, |args| {
            Ok(Value::Bool(
                parse_float(&args[0]).map(f64::is_nan).unwrap_or(false),
            ))
        })
        .sees_null(),
    );
}

fn wrong_type(func: &'static str, v: &Value) -> EvalError {
    EvalError::Type {
        op: func,
        left: type_name(v),
        right: None,
    }
}

fn number(func: &'static str, v: &Value) -> Result<Num, EvalError> {
    match v {
        Value::Number(n) => Ok(json_to_num(n)),
        other => Err(wrong_type(func, other)),
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
        other => wrong_type(func, other),
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
