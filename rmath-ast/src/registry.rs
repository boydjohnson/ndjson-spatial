//! The function registry: every function an expression can call.
//!
//! The parser turns `name(arg, ...)` into [`Expr::Call`] without knowing what
//! `name` is. [`FunctionRegistry::resolve`] then looks each name up, checks
//! its arity, and produces a [`CompiledExpr`] that holds the function itself,
//! so evaluation does no name lookups.
//!
//! [`FunctionRegistry::default`] holds the built-in functions (see
//! [`crate::builtins`]). Other crates add their own with
//! [`FunctionRegistry::register`], e.g. `ndjson-spatial`'s geometry
//! functions.
//!
//! ```
//! use rmath_ast::{Arity, EvalError, Function, FunctionRegistry};
//! use serde_json::{json, Value};
//!
//! let mut registry = FunctionRegistry::default();
//! registry.register(Function::scalar("len", Arity::exactly(1), |args| {
//!     match &args[0] {
//!         Value::String(s) => Ok(json!(s.chars().count())),
//!         _ => Err(EvalError::Type { op: "len", left: "non-string", right: None }),
//!     }
//! }));
//!
//! let expr = registry.compile("len(toupper(d.name)) + 1").unwrap();
//! assert_eq!(expr.eval(&json!({"name": "Ada"})), Ok(json!(4)));
//! ```

use crate::{
    builtins,
    eval::{binary, literal_to_json, negate, select, EvalError},
    parser::{
        ast::{BinaryOp, Expr, Term},
        ExprParser,
    },
};
use serde_json::Value;
use std::{collections::HashMap, fmt, sync::Arc};
use yajlish_ndjson::Selector;

/// The implementation of a scalar function: evaluated arguments in, one
/// value out.
pub type ScalarFn = Arc<dyn Fn(&[Value]) -> Result<Value, EvalError> + Send + Sync>;

/// How many arguments a function takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arity {
    pub min: usize,
    /// `None` means any number of arguments from `min` up.
    pub max: Option<usize>,
}

impl Arity {
    pub const fn exactly(n: usize) -> Self {
        Arity {
            min: n,
            max: Some(n),
        }
    }

    pub const fn range(min: usize, max: usize) -> Self {
        Arity {
            min,
            max: Some(max),
        }
    }

    pub const fn at_least(min: usize) -> Self {
        Arity { min, max: None }
    }

    pub fn accepts(&self, n: usize) -> bool {
        n >= self.min && self.max.is_none_or(|max| n <= max)
    }
}

impl fmt::Display for Arity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.max {
            Some(max) if max == self.min => write!(f, "{}", self.min),
            Some(max) => write!(f, "{} to {}", self.min, max),
            None => write!(f, "at least {}", self.min),
        }
    }
}

/// What kind of function this is. Aggregates (`sum`, `count`, ...) will be a
/// second variant with an accumulator factory.
#[derive(Clone)]
#[non_exhaustive]
pub enum FunctionKind {
    /// Evaluated once per record from that record's values.
    Scalar(ScalarFn),
}

/// A registered function.
#[derive(Clone)]
pub struct Function {
    name: &'static str,
    arity: Arity,
    null_propagates: bool,
    kind: FunctionKind,
}

impl Function {
    /// A scalar function. By default a `null` argument makes the result
    /// `null` without calling `f`; see [`Function::sees_null`].
    pub fn scalar<F>(name: &'static str, arity: Arity, f: F) -> Self
    where
        F: Fn(&[Value]) -> Result<Value, EvalError> + Send + Sync + 'static,
    {
        Function {
            name,
            arity,
            null_propagates: true,
            kind: FunctionKind::Scalar(Arc::new(f)),
        }
    }

    /// Pass `null` arguments to the implementation instead of short-circuiting
    /// to `null` (for predicates like `isnull`, and `coalesce`).
    pub fn sees_null(mut self) -> Self {
        self.null_propagates = false;
        self
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn arity(&self) -> Arity {
        self.arity
    }

    pub fn kind(&self) -> &FunctionKind {
        &self.kind
    }

    /// Call the function on already-evaluated arguments.
    pub fn call(&self, args: &[Value]) -> Result<Value, EvalError> {
        if self.null_propagates && args.iter().any(Value::is_null) {
            return Ok(Value::Null);
        }
        match &self.kind {
            FunctionKind::Scalar(f) => f(args),
        }
    }
}

impl fmt::Debug for Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Function")
            .field("name", &self.name)
            .field("arity", &self.arity)
            .field("null_propagates", &self.null_propagates)
            .finish_non_exhaustive()
    }
}

/// Name → function.
#[derive(Debug, Clone)]
pub struct FunctionRegistry {
    functions: HashMap<&'static str, Arc<Function>>,
}

impl Default for FunctionRegistry {
    /// A registry holding the built-in functions.
    fn default() -> Self {
        let mut registry = FunctionRegistry::empty();
        builtins::register(&mut registry);
        registry
    }
}

impl FunctionRegistry {
    /// A registry with no functions.
    pub fn empty() -> Self {
        FunctionRegistry {
            functions: HashMap::new(),
        }
    }

    /// Add `function`, replacing and returning any function already
    /// registered under its name.
    pub fn register(&mut self, function: Function) -> Option<Arc<Function>> {
        self.functions.insert(function.name, Arc::new(function))
    }

    pub fn get(&self, name: &str) -> Option<&Arc<Function>> {
        self.functions.get(name)
    }

    /// Registered function names, sorted.
    pub fn names(&self) -> Vec<&'static str> {
        let mut names: Vec<_> = self.functions.keys().copied().collect();
        names.sort_unstable();
        names
    }

    /// Parse `src` and [resolve](Self::resolve) it.
    pub fn compile(&self, src: &str) -> Result<CompiledExpr, CompileError> {
        let expr = ExprParser::new()
            .parse(src)
            .map_err(|e| CompileError::Parse(e.to_string()))?;
        Ok(self.resolve(&expr)?)
    }

    /// Bind every call in `expr` to a registered function, checking arity.
    pub fn resolve(&self, expr: &Expr) -> Result<CompiledExpr, ResolveError> {
        Ok(match expr {
            Expr::Term(Term::Value(lit)) => CompiledExpr::Literal(literal_to_json(lit)),
            Expr::Term(Term::Selector(path)) => CompiledExpr::Select(path.clone()),
            Expr::Negate(inner) => CompiledExpr::Negate(Box::new(self.resolve(inner)?)),
            Expr::Op(left, op, right) => CompiledExpr::Op(
                Box::new(self.resolve(left)?),
                *op,
                Box::new(self.resolve(right)?),
            ),
            Expr::Call(name, args) => {
                let function = self
                    .get(name)
                    .ok_or_else(|| ResolveError::UnknownFunction { name: name.clone() })?;
                if !function.arity.accepts(args.len()) {
                    return Err(ResolveError::Arity {
                        name: function.name,
                        expected: function.arity,
                        got: args.len(),
                    });
                }
                let args = args
                    .iter()
                    .map(|a| self.resolve(a))
                    .collect::<Result<_, _>>()?;
                CompiledExpr::Call(Arc::clone(function), args)
            }
        })
    }
}

/// An error binding a parsed expression to the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    UnknownFunction {
        name: String,
    },
    Arity {
        name: &'static str,
        expected: Arity,
        got: usize,
    },
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveError::UnknownFunction { name } => write!(f, "unknown function '{}'", name),
            ResolveError::Arity {
                name,
                expected,
                got,
            } => write!(
                f,
                "{}() takes {} argument{}, got {}",
                name,
                expected,
                // "1 argument", "at least 1 argument", "1 to 2 arguments"
                if expected.max.unwrap_or(expected.min) == 1 {
                    ""
                } else {
                    "s"
                },
                got
            ),
        }
    }
}

impl std::error::Error for ResolveError {}

/// An error from [`FunctionRegistry::compile`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileError {
    Parse(String),
    Resolve(ResolveError),
}

impl From<ResolveError> for CompileError {
    fn from(e: ResolveError) -> Self {
        CompileError::Resolve(e)
    }
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompileError::Parse(e) => write!(f, "parse error: {}", e),
            CompileError::Resolve(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for CompileError {}

/// An expression with every function call bound, ready to evaluate.
#[derive(Debug, Clone)]
pub enum CompiledExpr {
    Literal(Value),
    Select(Vec<Selector>),
    Negate(Box<CompiledExpr>),
    Op(Box<CompiledExpr>, BinaryOp, Box<CompiledExpr>),
    Call(Arc<Function>, Vec<CompiledExpr>),
}

impl CompiledExpr {
    /// Evaluate against `record` (the `d` in selectors).
    pub fn eval(&self, record: &Value) -> Result<Value, EvalError> {
        match self {
            CompiledExpr::Literal(v) => Ok(v.clone()),
            CompiledExpr::Select(path) => Ok(select(record, path).cloned().unwrap_or(Value::Null)),
            CompiledExpr::Negate(inner) => negate(inner.eval(record)?),
            CompiledExpr::Op(left, op, right) => {
                binary(*op, left.eval(record)?, right.eval(record)?)
            }
            CompiledExpr::Call(function, args) => {
                let args = args
                    .iter()
                    .map(|a| a.eval(record))
                    .collect::<Result<Vec<_>, _>>()?;
                function.call(&args)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// `least(a, b, ...)`: the smallest number, ignoring nulls.
    fn least() -> Function {
        Function::scalar("least", Arity::at_least(1), |args| {
            Ok(args
                .iter()
                .filter_map(Value::as_f64)
                .fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.min(x))))
                .map_or(Value::Null, |m| json!(m)))
        })
        .sees_null()
    }

    #[test]
    fn test_builtins_are_registered() {
        assert_eq!(
            FunctionRegistry::default().names(),
            vec![
                "abs",
                "acos",
                "acosh",
                "asin",
                "asinh",
                "atan",
                "atanh",
                "capitalize",
                "ceil",
                "cos",
                "cosh",
                "exp",
                "floor",
                "isfloat",
                "isfloatnan",
                "isnull",
                "log",
                "log10",
                "sin",
                "sinh",
                "sqrt",
                "tan",
                "tanh",
                "tofloat",
                "toint",
                "tolower",
                "tostring",
                "toupper",
                "trunc",
            ]
        );
        assert!(FunctionRegistry::empty().names().is_empty());
    }

    #[test]
    fn test_unknown_function() {
        let err = FunctionRegistry::default()
            .compile("1 + nope(d.a)")
            .unwrap_err();
        assert_eq!(
            err,
            CompileError::Resolve(ResolveError::UnknownFunction {
                name: "nope".into()
            })
        );
        assert_eq!(err.to_string(), "unknown function 'nope'");
        assert!(matches!(
            FunctionRegistry::empty().compile("abs(1)"),
            Err(CompileError::Resolve(ResolveError::UnknownFunction { .. }))
        ));
    }

    #[test]
    fn test_arity_is_checked_when_resolving() {
        let registry = FunctionRegistry::default();
        let err = registry.compile("abs(1, 2)").unwrap_err();
        assert_eq!(
            err,
            CompileError::Resolve(ResolveError::Arity {
                name: "abs",
                expected: Arity::exactly(1),
                got: 2
            })
        );
        assert_eq!(err.to_string(), "abs() takes 1 argument, got 2");
        assert_eq!(
            registry.compile("sqrt()").unwrap_err().to_string(),
            "sqrt() takes 1 argument, got 0"
        );
        // nested calls are checked too
        assert!(registry.compile("abs(floor())").is_err());
    }

    #[test]
    fn test_arity_display() {
        assert_eq!(Arity::exactly(2).to_string(), "2");
        assert_eq!(Arity::range(1, 2).to_string(), "1 to 2");
        assert_eq!(Arity::at_least(1).to_string(), "at least 1");
        assert!(Arity::range(1, 2).accepts(2));
        assert!(!Arity::range(1, 2).accepts(3));
        assert!(Arity::at_least(0).accepts(100));
    }

    #[test]
    fn test_register_custom_variadic_function() {
        let mut registry = FunctionRegistry::default();
        assert!(registry.register(least()).is_none());
        let r = json!({"a": 3, "b": -1.5, "c": null});
        let eval = |src: &str| registry.compile(src).unwrap().eval(&r);
        assert_eq!(eval("least(d.a)"), Ok(json!(3.0)));
        assert_eq!(eval("least(d.a, d.b, d.c, 10)"), Ok(json!(-1.5)));
        assert_eq!(eval("abs(least(d.a, d.b)) * 2"), Ok(json!(3.0)));
        assert_eq!(eval("least(d.c)"), Ok(Value::Null));
        assert_eq!(
            registry.compile("least()").unwrap_err().to_string(),
            "least() takes at least 1 argument, got 0"
        );
    }

    #[test]
    fn test_null_propagation_is_per_function() {
        let mut registry = FunctionRegistry::empty();
        registry.register(Function::scalar("first", Arity::range(1, 2), |args| {
            Ok(args[0].clone())
        }));
        registry.register(least());
        let r = json!({"a": 1});
        let eval = |src: &str| registry.compile(src).unwrap().eval(&r);
        // `first` propagates null, so it is never called with one
        assert_eq!(eval("first(d.a, null)"), Ok(Value::Null));
        assert_eq!(eval("first(d.a, 2)"), Ok(json!(1)));
        // `least` sees nulls and skips them
        assert_eq!(eval("least(d.a, null)"), Ok(json!(1.0)));
    }

    #[test]
    fn test_register_replaces_existing_function() {
        let mut registry = FunctionRegistry::default();
        let old = registry.register(Function::scalar("abs", Arity::exactly(1), |_| {
            Ok(json!("replaced"))
        }));
        assert_eq!(old.map(|f| f.name()), Some("abs"));
        assert_eq!(
            registry.compile("abs(-1)").unwrap().eval(&json!({})),
            Ok(json!("replaced"))
        );
    }

    #[test]
    fn test_errors_from_arguments_propagate() {
        let registry = FunctionRegistry::default();
        assert_eq!(
            registry.compile("abs(1 / 0)").unwrap().eval(&json!({})),
            Err(EvalError::DivisionByZero)
        );
    }

    #[test]
    fn test_parse_errors() {
        assert!(matches!(
            FunctionRegistry::default().compile("abs(1"),
            Err(CompileError::Parse(_))
        ));
    }
}
