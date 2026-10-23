//! The function registry: every function an expression can call.
//!
//! The parser turns `name(arg, ...)` into [`Expr::Call`] without knowing what
//! `name` is. [`FunctionRegistry::resolve`] then looks each name up, checks
//! its arity, and produces a [`CompiledExpr`] that holds the function itself,
//! so evaluation does no name lookups.
//!
//! Functions are either scalar (evaluated per record) or aggregate (an
//! [`Accumulator`] fed every record, see [`crate::aggregate`]). `resolve`
//! only accepts scalar calls; [`crate::aggregate::AggregateList::compile`]
//! handles lists that contain aggregates.
//!
//! [`FunctionRegistry::default`] holds the built-in functions (see
//! [`crate::builtins`] and [`crate::aggregate`]). Other crates add their own with
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
    aggregate::{self, Accumulator},
    builtins,
    eval::{
        binary, compare, json_eq, literal_to_json, logic_operand, logic_result, negate, select,
        EvalError,
    },
    parser::{
        ast::{BinaryOp, CompareOp, Expr, Term},
        ExprParser,
    },
};
use serde_json::Value;
use std::{collections::HashMap, fmt, sync::Arc};
use yajlish_ndjson::Selector;

/// The implementation of a scalar function: evaluated arguments in, one
/// value out.
pub type ScalarFn = Arc<dyn Fn(&[Value]) -> Result<Value, EvalError> + Send + Sync>;

/// Makes a fresh accumulator for an aggregate function from the values of
/// its constant parameters (see [`Function::aggregate_with_params`]), or
/// says why they are wrong.
pub type AggregateFn = Arc<dyn Fn(&[Value]) -> Result<Box<dyn Accumulator>, String> + Send + Sync>;

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

/// What kind of function this is.
#[derive(Clone)]
#[non_exhaustive]
pub enum FunctionKind {
    /// Evaluated once per record from that record's values.
    Scalar(ScalarFn),
    /// Folds every record of a group into an [`Accumulator`] and yields one
    /// value at the end.
    Aggregate(AggregateFn),
}

/// Which row an aggregate picks, for aggregates like `min` and `max` whose
/// result comes from one record. A select list with exactly one of these
/// evaluates its bare fields on that record (see [`AggregateList`]).
///
/// [`AggregateList`]: crate::aggregate::AggregateList
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extreme {
    Min,
    Max,
}

/// A registered function.
#[derive(Clone)]
pub struct Function {
    name: &'static str,
    arity: Arity,
    null_propagates: bool,
    unbounded: bool,
    extreme: Option<Extreme>,
    params_from: Option<usize>,
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
            unbounded: false,
            extreme: None,
            params_from: None,
            kind: FunctionKind::Scalar(Arc::new(f)),
        }
    }

    /// An aggregate function. `new` makes the accumulator for one group; it
    /// is fed every record's evaluated arguments, `null`s included.
    pub fn aggregate<F>(name: &'static str, arity: Arity, new: F) -> Self
    where
        F: Fn() -> Box<dyn Accumulator> + Send + Sync + 'static,
    {
        Function {
            name,
            arity,
            null_propagates: false,
            unbounded: false,
            extreme: None,
            params_from: None,
            kind: FunctionKind::Aggregate(Arc::new(move |_: &[Value]| Ok(new()))),
        }
    }

    /// An aggregate function whose arguments from index `params_from` on are
    /// constant parameters (the `0.9` of `approx_quantile(d.x, 0.9)`) rather
    /// than values per record. They are evaluated once when the list is
    /// compiled, and `new` gets the ones the call gave to make each
    /// accumulator, or returns why they are wrong.
    pub fn aggregate_with_params<F>(
        name: &'static str,
        arity: Arity,
        params_from: usize,
        new: F,
    ) -> Self
    where
        F: Fn(&[Value]) -> Result<Box<dyn Accumulator>, String> + Send + Sync + 'static,
    {
        Function {
            name,
            arity,
            null_propagates: false,
            unbounded: false,
            extreme: None,
            params_from: Some(params_from),
            kind: FunctionKind::Aggregate(Arc::new(new)),
        }
    }

    /// Mark an aggregate whose state grows with its input (`collect`,
    /// `count_distinct`), so commands can refuse it without an explicit
    /// opt-in.
    pub fn unbounded(mut self) -> Self {
        self.unbounded = true;
        self
    }

    /// Mark an aggregate whose result is the first argument of the record
    /// holding the minimum or maximum of that argument.
    pub fn extreme(mut self, extreme: Extreme) -> Self {
        self.extreme = Some(extreme);
        self
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

    pub fn is_aggregate(&self) -> bool {
        matches!(self.kind, FunctionKind::Aggregate(_))
    }

    pub fn is_unbounded(&self) -> bool {
        self.unbounded
    }

    pub fn extreme_kind(&self) -> Option<Extreme> {
        self.extreme
    }

    /// For an aggregate with constant parameters, the index of the first.
    pub fn params_from(&self) -> Option<usize> {
        self.params_from
    }

    /// A fresh accumulator given the values of the call's constant
    /// parameters (none for most aggregates), or why they are wrong. `None`
    /// for a scalar function.
    pub fn accumulator(&self, params: &[Value]) -> Option<Result<Box<dyn Accumulator>, String>> {
        match &self.kind {
            FunctionKind::Aggregate(new) => Some(new(params)),
            FunctionKind::Scalar(_) => None,
        }
    }

    /// Call a scalar function on already-evaluated arguments. Aggregates
    /// can't be called this way and return [`EvalError::NotScalar`].
    pub fn call(&self, args: &[Value]) -> Result<Value, EvalError> {
        match &self.kind {
            FunctionKind::Scalar(f) => {
                if self.null_propagates && args.iter().any(Value::is_null) {
                    return Ok(Value::Null);
                }
                f(args)
            }
            FunctionKind::Aggregate(_) => Err(EvalError::NotScalar { func: self.name }),
        }
    }
}

impl fmt::Debug for Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Function")
            .field("name", &self.name)
            .field("arity", &self.arity)
            .field("null_propagates", &self.null_propagates)
            .field("aggregate", &self.is_aggregate())
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
        aggregate::register(&mut registry);
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

    /// Whether `expr` calls an aggregate function anywhere. Unknown
    /// functions count as scalar (resolving reports them).
    pub fn contains_aggregate(&self, expr: &Expr) -> bool {
        let mut found = false;
        visit(expr, &mut |e| {
            if let Expr::Call(name, _) = e {
                found |= self.get(name).is_some_and(|f| f.is_aggregate());
            }
        });
        found
    }

    /// Bind every call in `expr` to a registered scalar function, checking
    /// arity. Aggregate calls are [`ResolveError::Aggregate`].
    pub fn resolve(&self, expr: &Expr) -> Result<CompiledExpr, ResolveError> {
        self.resolve_with(expr, &mut |_| None)
    }

    /// [`resolve`](Self::resolve), except that `hook` sees every node first
    /// and can compile it itself by returning `Some`.
    pub(crate) fn resolve_with(
        &self,
        expr: &Expr,
        hook: &mut dyn FnMut(&Expr) -> Option<Result<CompiledExpr, ResolveError>>,
    ) -> Result<CompiledExpr, ResolveError> {
        if let Some(compiled) = hook(expr) {
            return compiled;
        }
        let mut go = |e: &Expr| self.resolve_with(e, &mut *hook);
        Ok(match expr {
            Expr::Term(Term::Value(lit)) => CompiledExpr::Literal(literal_to_json(lit)),
            Expr::Term(Term::Selector(path)) => CompiledExpr::Select(path.clone()),
            Expr::Negate(inner) => CompiledExpr::Negate(Box::new(go(inner)?)),
            Expr::Op(left, op, right) => {
                CompiledExpr::Op(Box::new(go(left)?), *op, Box::new(go(right)?))
            }
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
                if function.is_aggregate() {
                    return Err(ResolveError::Aggregate {
                        name: function.name,
                    });
                }
                let args = args.iter().map(&mut go).collect::<Result<_, _>>()?;
                CompiledExpr::Call(Arc::clone(function), args)
            }
            Expr::Compare(left, op, right) => {
                CompiledExpr::Compare(Box::new(go(left)?), *op, Box::new(go(right)?))
            }
            Expr::And(left, right) => CompiledExpr::And(Box::new(go(left)?), Box::new(go(right)?)),
            Expr::Or(left, right) => CompiledExpr::Or(Box::new(go(left)?), Box::new(go(right)?)),
            Expr::Not(inner) => CompiledExpr::Not(Box::new(go(inner)?)),
            Expr::IsNull { expr, negated } => CompiledExpr::IsNull {
                expr: Box::new(go(expr)?),
                negated: *negated,
            },
            Expr::In {
                expr,
                list,
                negated,
            } => CompiledExpr::In {
                expr: Box::new(go(expr)?),
                list: list.iter().map(literal_to_json).collect(),
                negated: *negated,
            },
        })
    }
}

/// Calls `f` on `expr` and every expression inside it.
pub(crate) fn visit<'a>(expr: &'a Expr, f: &mut dyn FnMut(&'a Expr)) {
    f(expr);
    match expr {
        Expr::Term(_) => {}
        Expr::Negate(e) | Expr::Not(e) => visit(e, f),
        Expr::IsNull { expr, .. } | Expr::In { expr, .. } => visit(expr, f),
        Expr::Op(l, _, r) | Expr::Compare(l, _, r) | Expr::And(l, r) | Expr::Or(l, r) => {
            visit(l, f);
            visit(r, f);
        }
        Expr::Call(_, args) => args.iter().for_each(|a| visit(a, f)),
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
    /// An aggregate where only scalar expressions are allowed (`filter
    /// 'sum(d.x) > 1'`).
    Aggregate {
        name: &'static str,
    },
    /// An aggregate inside another aggregate's arguments (`sum(max(d.x))`).
    NestedAggregate {
        outer: &'static str,
        inner: &'static str,
    },
    /// A field used outside any aggregate in an aggregate list, where there
    /// is no single row to take it from. `item` counts from 1.
    Bare {
        item: usize,
    },
    /// A bad constant parameter of an aggregate (`approx_quantile(d.x, 2)`).
    Parameter {
        name: &'static str,
        message: String,
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
                // "1 argument", "at least 1 argument", "0 to 1 arguments"
                if expected.min == 1 && expected.max.is_none_or(|max| max == 1) {
                    ""
                } else {
                    "s"
                },
                got
            ),
            ResolveError::Aggregate { name } => write!(
                f,
                "{}() is an aggregate function and can't be used here",
                name
            ),
            ResolveError::NestedAggregate { outer, inner } => write!(
                f,
                "{}() can't be used inside the aggregate {}()",
                inner, outer
            ),
            ResolveError::Bare { item } => write!(
                f,
                "item {}: a field outside an aggregate needs exactly one min() or max() \
                 in the list (its row is used) or must be a grouping key; \
                 use first(), last(), argmax() or argmin() instead",
                item
            ),
            ResolveError::Parameter { name, message } => write!(f, "{}(): {}", name, message),
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
    Compare(Box<CompiledExpr>, CompareOp, Box<CompiledExpr>),
    And(Box<CompiledExpr>, Box<CompiledExpr>),
    Or(Box<CompiledExpr>, Box<CompiledExpr>),
    Not(Box<CompiledExpr>),
    IsNull {
        expr: Box<CompiledExpr>,
        negated: bool,
    },
    In {
        expr: Box<CompiledExpr>,
        list: Vec<Value>,
        negated: bool,
    },
    /// The result of the n-th aggregate of an
    /// [`AggregateList`](crate::aggregate::AggregateList). Outside one it is
    /// `null`.
    Slot(usize),
}

impl CompiledExpr {
    /// Evaluate against `record` (the `d` in selectors).
    pub fn eval(&self, record: &Value) -> Result<Value, EvalError> {
        self.eval_with(record, &[])
    }

    /// Evaluate with `slots` holding the aggregate results.
    pub(crate) fn eval_with(&self, record: &Value, slots: &[Value]) -> Result<Value, EvalError> {
        let eval = |e: &CompiledExpr| e.eval_with(record, slots);
        match self {
            CompiledExpr::Literal(v) => Ok(v.clone()),
            CompiledExpr::Select(path) => Ok(select(record, path).cloned().unwrap_or(Value::Null)),
            CompiledExpr::Negate(inner) => negate(eval(inner)?),
            CompiledExpr::Op(left, op, right) => binary(*op, eval(left)?, eval(right)?),
            CompiledExpr::Call(function, args) => {
                let args = args.iter().map(eval).collect::<Result<Vec<_>, _>>()?;
                function.call(&args)
            }
            CompiledExpr::Compare(left, op, right) => {
                Ok(Value::Bool(compare(*op, &eval(left)?, &eval(right)?)))
            }
            CompiledExpr::And(left, right) => {
                let l = logic_operand("and", &eval(left)?)?;
                if l == Some(false) {
                    return Ok(Value::Bool(false));
                }
                let r = logic_operand("and", &eval(right)?)?;
                Ok(logic_result(match (l, r) {
                    (_, Some(false)) => Some(false),
                    (Some(true), r) => r,
                    _ => None,
                }))
            }
            CompiledExpr::Or(left, right) => {
                let l = logic_operand("or", &eval(left)?)?;
                if l == Some(true) {
                    return Ok(Value::Bool(true));
                }
                let r = logic_operand("or", &eval(right)?)?;
                Ok(logic_result(match (l, r) {
                    (_, Some(true)) => Some(true),
                    (Some(false), r) => r,
                    _ => None,
                }))
            }
            CompiledExpr::Not(inner) => Ok(logic_result(
                logic_operand("not", &eval(inner)?)?.map(|b| !b),
            )),
            CompiledExpr::IsNull { expr, negated } => {
                Ok(Value::Bool(eval(expr)?.is_null() != *negated))
            }
            CompiledExpr::Slot(i) => Ok(slots.get(*i).cloned().unwrap_or(Value::Null)),
            CompiledExpr::In {
                expr,
                list,
                negated,
            } => {
                let v = eval(expr)?;
                Ok(Value::Bool(
                    list.iter().any(|item| json_eq(&v, item)) != *negated,
                ))
            }
        }
    }

    /// Evaluate as a predicate: `Ok(true)` only when the result is `true`.
    /// `false` and `null` are `Ok(false)`; any other value is a type error.
    pub fn eval_bool(&self, record: &Value) -> Result<bool, EvalError> {
        match self.eval(record)? {
            Value::Bool(b) => Ok(b),
            Value::Null => Ok(false),
            other => Err(EvalError::NotBool {
                got: crate::eval::type_name(&other),
            }),
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
                "approx_count_distinct",
                "approx_quantile",
                "argmax",
                "argmin",
                "asin",
                "asinh",
                "atan",
                "atanh",
                "avg",
                "capitalize",
                "ceil",
                "collect",
                "cos",
                "cosh",
                "count",
                "count_distinct",
                "exp",
                "first",
                "floor",
                "isfloat",
                "isfloatnan",
                "isnull",
                "last",
                "log",
                "log10",
                "max",
                "median",
                "min",
                "sin",
                "sinh",
                "sqrt",
                "stddev",
                "stddev_pop",
                "sum",
                "tan",
                "tanh",
                "tofloat",
                "toint",
                "tolower",
                "tostring",
                "toupper",
                "trunc",
                "var",
                "var_pop",
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
