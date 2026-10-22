//! Aggregate functions and the lists of expressions that use them.
//!
//! An aggregate function (`count`, `sum`, `max`, ...) folds every record of
//! a group into an [`Accumulator`] and yields one value when the group ends.
//! Accumulators keep constant state, except the ones marked
//! [unbounded](crate::registry::Function::unbounded) (`count_distinct`,
//! `collect`).
//!
//! [`AggregateList::compile`] takes the expressions of a select list such as
//! `d.state, count(), round(avg(d.salary), 2)` and splits each one into
//! aggregate *slots* (`count()`, `avg(d.salary)`) and a finishing expression
//! evaluated on the slot results (`round(<slot 1>, 2)`).
//!
//! Semantics:
//!
//! - Aggregates skip `null` arguments, so a missing field doesn't count.
//!   `count()` counts records and `count(x)` counts non-null `x`.
//! - An empty group gives `null` for every aggregate except `count` and
//!   `count_distinct`, which give 0.
//! - `sum`, `avg`, `var` and `stddev` take numbers. `sum` stays an integer
//!   while every input is one. `min`, `max`, `argmin` and `argmax` order
//!   numbers, strings, or bools, and keep the first record on a tie.
//! - A value an aggregate can't take (`sum` of a string) is an
//!   [`EvalError`]; the record is skipped for that aggregate only, and the
//!   caller decides how to report it.
//! - A field outside any aggregate (`max(d.amount), d.name`) is taken from
//!   the record that held the list's only `min()` or `max()`, or must be a
//!   grouping key. Anything else is [`ResolveError::Bare`].
//!
//! ```
//! use rmath_ast::{AggregateList, FunctionRegistry};
//! use rmath_ast::parser::ExprParser;
//! use serde_json::json;
//!
//! let registry = FunctionRegistry::default();
//! let items = ["count()", "max(d.amount)", "d.name"]
//!     .map(|src| *ExprParser::new().parse(src).unwrap());
//! let list = AggregateList::compile(&registry, &items, &[]).unwrap();
//!
//! let mut state = list.start();
//! for record in [json!({"name": "Ada", "amount": 3}), json!({"name": "Grace", "amount": 9})] {
//!     list.update(&mut state, &record, |item, err| panic!("item {}: {}", item, err));
//! }
//! let row: Vec<_> = list.finish(&state).into_iter().map(Result::unwrap).collect();
//! assert_eq!(row, vec![json!(2), json!(9), json!("Grace")]);
//! ```

use crate::{
    eval::{
        as_f64, as_int, float_result, int_result, json_order, json_to_num, type_name, EvalError,
    },
    parser::ast::{Expr, Num, Term},
    registry::{visit, Arity, CompiledExpr, Extreme, Function, FunctionRegistry, ResolveError},
};
use serde_json::Value;
use std::{any::Any, cmp::Ordering, collections::HashSet, fmt, ops::Range, sync::Arc};

/// The running state of one aggregate over one group.
pub trait Accumulator: Send {
    /// Fold in one record's evaluated arguments. On error the state is
    /// unchanged.
    fn update(&mut self, args: &[Value]) -> Result<(), EvalError>;

    /// Fold in another accumulator of the same function, as if its records
    /// came after this one's.
    ///
    /// # Panics
    ///
    /// If `other` belongs to a different function.
    fn merge(&mut self, other: &dyn Accumulator);

    /// The result for the records seen so far.
    fn finish(&self) -> Result<Value, EvalError>;

    fn as_any(&self) -> &dyn Any;
}

fn same<T: 'static>(other: &dyn Accumulator) -> &T {
    other
        .as_any()
        .downcast_ref()
        .expect("merging accumulators of different functions")
}

/// One aggregate call in an [`AggregateList`].
#[derive(Debug, Clone)]
struct Slot {
    function: Arc<Function>,
    args: Vec<CompiledExpr>,
}

#[derive(Debug, Clone)]
struct Item {
    expr: CompiledExpr,
    /// The slots this item's aggregates were compiled into.
    slots: Range<usize>,
}

/// A list of expressions evaluated over a group of records: a select list
/// in aggregate mode.
#[derive(Debug, Clone)]
pub struct AggregateList {
    slots: Vec<Slot>,
    /// The item each slot belongs to.
    slot_items: Vec<usize>,
    items: Vec<Item>,
}

/// The accumulators of an [`AggregateList`] for one group.
pub struct AggregateState {
    accumulators: Vec<Box<dyn Accumulator>>,
}

impl AggregateState {
    /// Fold in the state of another group of the same list (e.g. another
    /// partition of the input), as if its records came after this one's.
    pub fn merge(&mut self, other: &AggregateState) {
        for (mine, theirs) in self.accumulators.iter_mut().zip(&other.accumulators) {
            mine.merge(theirs.as_ref());
        }
    }
}

impl fmt::Debug for AggregateState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AggregateState")
            .field("slots", &self.accumulators.len())
            .finish()
    }
}

impl AggregateList {
    /// Compile `items` for evaluation over groups of records grouped by the
    /// `by` expressions (empty for the whole stream).
    ///
    /// A part of an item that is textually equal to a `by` expression is
    /// constant within a group and may be used outside aggregates. So may
    /// any field when the items hold exactly one `min()` or `max()` call:
    /// it is evaluated on the record that held that extreme.
    pub fn compile(
        registry: &FunctionRegistry,
        items: &[Expr],
        by: &[Expr],
    ) -> Result<Self, ResolveError> {
        let row = extreme_row(registry, items)?;
        let mut list = AggregateList {
            slots: Vec::new(),
            slot_items: Vec::new(),
            items: Vec::with_capacity(items.len()),
        };

        for (n, item) in items.iter().enumerate() {
            let first_slot = list.slots.len();
            let slots = &mut list.slots;
            let mut push = |function: Function, args| {
                slots.push(Slot {
                    function: Arc::new(function),
                    args,
                });
                CompiledExpr::Slot(slots.len() - 1)
            };
            let expr = registry.resolve_with(item, &mut |e| {
                if by.contains(e) && !registry.contains_aggregate(e) {
                    return Some(registry.resolve(e).map(|key| push(first(), vec![key])));
                }
                match e {
                    Expr::Call(name, args) => {
                        let function = registry.get(name).filter(|f| f.is_aggregate())?;
                        Some(
                            aggregate_args(registry, function, args)
                                .map(|args| push(Function::clone(function), args)),
                        )
                    }
                    Expr::Term(Term::Selector(_)) => Some(match &row {
                        Some((extreme, key)) => registry
                            .resolve(e)
                            .map(|field| push(arg(*extreme), vec![key.clone(), field])),
                        None => Err(ResolveError::Bare { item: n + 1 }),
                    }),
                    _ => None,
                }
            })?;
            let slots = first_slot..list.slots.len();
            list.slot_items.extend(slots.clone().map(|_| n));
            list.items.push(Item { expr, slots });
        }
        Ok(list)
    }

    /// The number of items.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The names of the aggregates used whose state grows with the input.
    pub fn unbounded_functions(&self) -> Vec<&'static str> {
        let mut names: Vec<_> = self
            .slots
            .iter()
            .filter(|s| s.function.is_unbounded())
            .map(|s| s.function.name())
            .collect();
        names.dedup();
        names
    }

    /// Fresh state for a new group.
    pub fn start(&self) -> AggregateState {
        AggregateState {
            accumulators: self
                .slots
                .iter()
                .map(|s| s.function.accumulator().expect("slots are aggregates"))
                .collect(),
        }
    }

    /// Fold `record` into `state`. When an aggregate's arguments fail to
    /// evaluate or it rejects them, `on_error` gets the index of the item it
    /// belongs to and that aggregate skips the record.
    pub fn update(
        &self,
        state: &mut AggregateState,
        record: &Value,
        mut on_error: impl FnMut(usize, EvalError),
    ) {
        let mut args = Vec::new();
        for (n, (slot, acc)) in self.slots.iter().zip(&mut state.accumulators).enumerate() {
            args.clear();
            let result = slot
                .args
                .iter()
                .try_for_each(|a| a.eval(record).map(|v| args.push(v)))
                .and_then(|()| acc.update(&args));
            if let Err(e) = result {
                on_error(self.slot_items[n], e);
            }
        }
    }

    /// One value per item for the group in `state`.
    pub fn finish(&self, state: &AggregateState) -> Vec<Result<Value, EvalError>> {
        let finished: Vec<_> = state.accumulators.iter().map(|a| a.finish()).collect();
        let values: Vec<_> = finished
            .iter()
            .map(|r| r.clone().unwrap_or(Value::Null))
            .collect();
        self.items
            .iter()
            .map(|item| {
                if let Some(Err(e)) = finished[item.slots.clone()].iter().find(|r| r.is_err()) {
                    return Err(e.clone());
                }
                item.expr.eval_with(&Value::Null, &values)
            })
            .collect()
    }
}

/// The only `min()`/`max()` call in `items`, as its direction and compiled
/// first argument. `None` when there are none or several.
fn extreme_row(
    registry: &FunctionRegistry,
    items: &[Expr],
) -> Result<Option<(Extreme, CompiledExpr)>, ResolveError> {
    let mut calls = Vec::new();
    for item in items {
        visit(item, &mut |e| {
            if let Expr::Call(name, args) = e {
                if let Some(f) = registry.get(name) {
                    if let (Some(extreme), Some(key)) = (f.extreme_kind(), args.first()) {
                        calls.push((f, extreme, key));
                    }
                }
            }
        });
    }
    match calls[..] {
        [(function, extreme, key)] => Ok(Some((
            extreme,
            registry
                .resolve(key)
                .map_err(|e| nested(function.name(), e))?,
        ))),
        _ => Ok(None),
    }
}

/// Check the arity of a call to `function` and resolve its (scalar)
/// arguments.
fn aggregate_args(
    registry: &FunctionRegistry,
    function: &Function,
    args: &[Expr],
) -> Result<Vec<CompiledExpr>, ResolveError> {
    if !function.arity().accepts(args.len()) {
        return Err(ResolveError::Arity {
            name: function.name(),
            expected: function.arity(),
            got: args.len(),
        });
    }
    args.iter()
        .map(|a| registry.resolve(a).map_err(|e| nested(function.name(), e)))
        .collect()
}

fn nested(outer: &'static str, e: ResolveError) -> ResolveError {
    match e {
        ResolveError::Aggregate { name } => ResolveError::NestedAggregate { outer, inner: name },
        e => e,
    }
}

pub(crate) fn register(registry: &mut FunctionRegistry) {
    let one = Arity::exactly(1);

    registry.register(Function::aggregate("count", Arity::range(0, 1), || {
        Box::new(Count::default())
    }));
    registry.register(Function::aggregate("sum", one, || Box::new(Sum::default())));
    registry.register(Function::aggregate("avg", one, || Box::new(Avg::default())));
    registry.register(
        Function::aggregate("min", one, || Box::new(Best::new(Extreme::Min))).extreme(Extreme::Min),
    );
    registry.register(
        Function::aggregate("max", one, || Box::new(Best::new(Extreme::Max))).extreme(Extreme::Max),
    );
    registry.register(arg(Extreme::Min));
    registry.register(arg(Extreme::Max));
    registry.register(first());
    registry.register(Function::aggregate("last", one, || {
        Box::new(Pick {
            last: true,
            value: None,
        })
    }));
    for (name, sample, root) in [
        ("var", true, false),
        ("var_pop", false, false),
        ("stddev", true, true),
        ("stddev_pop", false, true),
    ] {
        registry.register(Function::aggregate(name, one, move || {
            Box::new(Variance {
                name,
                sample,
                root,
                ..Variance::default()
            })
        }));
    }
    registry.register(
        Function::aggregate("count_distinct", one, || Box::new(CountDistinct::default()))
            .unbounded(),
    );
    registry
        .register(Function::aggregate("collect", one, || Box::new(Collect::default())).unbounded());
}

/// `first(x)`: the first non-null `x`.
fn first() -> Function {
    Function::aggregate("first", Arity::exactly(1), || {
        Box::new(Pick {
            last: false,
            value: None,
        })
    })
}

/// `argmin(key, value)` / `argmax(key, value)`: `value` from the record
/// with the smallest / largest `key`.
fn arg(extreme: Extreme) -> Function {
    let name = match extreme {
        Extreme::Min => "argmin",
        Extreme::Max => "argmax",
    };
    Function::aggregate(name, Arity::exactly(2), move || {
        Box::new(ArgBest {
            name,
            extreme,
            best: None,
        })
    })
}

fn wrong_type(op: &'static str, v: &Value) -> EvalError {
    EvalError::Type {
        op,
        left: type_name(v),
        right: None,
    }
}

/// A number, `None` for `null`, or a type error.
fn number(op: &'static str, v: &Value) -> Result<Option<Num>, EvalError> {
    match v {
        Value::Null => Ok(None),
        Value::Number(n) => Ok(Some(json_to_num(n))),
        other => Err(wrong_type(op, other)),
    }
}

#[derive(Default)]
struct Count {
    n: u64,
}

impl Accumulator for Count {
    fn update(&mut self, args: &[Value]) -> Result<(), EvalError> {
        if args.first().is_none_or(|v| !v.is_null()) {
            self.n += 1;
        }
        Ok(())
    }

    fn merge(&mut self, other: &dyn Accumulator) {
        self.n += same::<Self>(other).n;
    }

    fn finish(&self) -> Result<Value, EvalError> {
        Ok(Value::from(self.n))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Integers are summed exactly; once a float (or an integer overflow) is
/// seen the result is a float.
#[derive(Default)]
struct Sum {
    int: i128,
    float: f64,
    is_float: bool,
    n: u64,
}

impl Sum {
    fn add_int(&mut self, i: i128) {
        match self.int.checked_add(i) {
            Some(sum) => self.int = sum,
            None => {
                self.float += i as f64;
                self.is_float = true;
            }
        }
    }

    fn add(&mut self, n: Num) {
        match as_int(n) {
            Some(i) => self.add_int(i),
            None => {
                self.float += as_f64(n);
                self.is_float = true;
            }
        }
        self.n += 1;
    }

    fn merge_sum(&mut self, other: &Sum) {
        self.add_int(other.int);
        self.float += other.float;
        self.is_float |= other.is_float;
        self.n += other.n;
    }

    fn total(&self) -> f64 {
        self.int as f64 + self.float
    }
}

impl Accumulator for Sum {
    fn update(&mut self, args: &[Value]) -> Result<(), EvalError> {
        if let Some(n) = number("sum", &args[0])? {
            self.add(n);
        }
        Ok(())
    }

    fn merge(&mut self, other: &dyn Accumulator) {
        self.merge_sum(same(other));
    }

    fn finish(&self) -> Result<Value, EvalError> {
        if self.n == 0 {
            Ok(Value::Null)
        } else if self.is_float {
            float_result("sum", self.total())
        } else {
            int_result("sum", self.int)
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Default)]
struct Avg(Sum);

impl Accumulator for Avg {
    fn update(&mut self, args: &[Value]) -> Result<(), EvalError> {
        if let Some(n) = number("avg", &args[0])? {
            self.0.add(n);
        }
        Ok(())
    }

    fn merge(&mut self, other: &dyn Accumulator) {
        self.0.merge_sum(&same::<Self>(other).0);
    }

    fn finish(&self) -> Result<Value, EvalError> {
        if self.0.n == 0 {
            Ok(Value::Null)
        } else {
            float_result("avg", self.0.total() / self.0.n as f64)
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Welford's online variance.
#[derive(Default)]
struct Variance {
    name: &'static str,
    /// Divide by `n - 1` instead of `n`.
    sample: bool,
    /// Standard deviation instead of variance.
    root: bool,
    n: u64,
    mean: f64,
    m2: f64,
}

impl Accumulator for Variance {
    fn update(&mut self, args: &[Value]) -> Result<(), EvalError> {
        if let Some(x) = number(self.name, &args[0])? {
            let x = as_f64(x);
            self.n += 1;
            let delta = x - self.mean;
            self.mean += delta / self.n as f64;
            self.m2 += delta * (x - self.mean);
        }
        Ok(())
    }

    fn merge(&mut self, other: &dyn Accumulator) {
        let other: &Self = same(other);
        if other.n == 0 {
            return;
        }
        let (na, nb) = (self.n as f64, other.n as f64);
        let n = na + nb;
        let delta = other.mean - self.mean;
        self.mean += delta * nb / n;
        self.m2 += other.m2 + delta * delta * na * nb / n;
        self.n += other.n;
    }

    fn finish(&self) -> Result<Value, EvalError> {
        let divisor = if self.sample {
            self.n.saturating_sub(1)
        } else {
            self.n
        };
        if divisor == 0 {
            return Ok(Value::Null);
        }
        let var = self.m2 / divisor as f64;
        float_result(self.name, if self.root { var.sqrt() } else { var })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Whether `candidate` should replace `best` as the `extreme`. Ties keep
/// `best`.
fn beats(
    op: &'static str,
    extreme: Extreme,
    candidate: &Value,
    best: Option<&Value>,
) -> Result<bool, EvalError> {
    if !matches!(
        candidate,
        Value::Number(_) | Value::String(_) | Value::Bool(_)
    ) {
        return Err(wrong_type(op, candidate));
    }
    let Some(best) = best else {
        return Ok(true);
    };
    match json_order(candidate, best) {
        Some(ord) => Ok(ord
            == match extreme {
                Extreme::Min => Ordering::Less,
                Extreme::Max => Ordering::Greater,
            }),
        None => Err(EvalError::Type {
            op,
            left: type_name(candidate),
            right: Some(type_name(best)),
        }),
    }
}

/// `min(x)` / `max(x)`.
struct Best {
    extreme: Extreme,
    best: Option<Value>,
}

impl Best {
    fn new(extreme: Extreme) -> Self {
        Best {
            extreme,
            best: None,
        }
    }

    fn name(&self) -> &'static str {
        match self.extreme {
            Extreme::Min => "min",
            Extreme::Max => "max",
        }
    }
}

impl Accumulator for Best {
    fn update(&mut self, args: &[Value]) -> Result<(), EvalError> {
        let v = &args[0];
        if !v.is_null() && beats(self.name(), self.extreme, v, self.best.as_ref())? {
            self.best = Some(v.clone());
        }
        Ok(())
    }

    fn merge(&mut self, other: &dyn Accumulator) {
        if let Some(theirs) = &same::<Self>(other).best {
            if beats(self.name(), self.extreme, theirs, self.best.as_ref()).unwrap_or(false) {
                self.best = Some(theirs.clone());
            }
        }
    }

    fn finish(&self) -> Result<Value, EvalError> {
        Ok(self.best.clone().unwrap_or(Value::Null))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// `argmin(key, value)` / `argmax(key, value)`. Records with a `null` key
/// are skipped; the value is kept as is, `null` included.
struct ArgBest {
    name: &'static str,
    extreme: Extreme,
    best: Option<(Value, Value)>,
}

impl Accumulator for ArgBest {
    fn update(&mut self, args: &[Value]) -> Result<(), EvalError> {
        let key = &args[0];
        if !key.is_null()
            && beats(
                self.name,
                self.extreme,
                key,
                self.best.as_ref().map(|(k, _)| k),
            )?
        {
            self.best = Some((key.clone(), args[1].clone()));
        }
        Ok(())
    }

    fn merge(&mut self, other: &dyn Accumulator) {
        if let Some((key, value)) = &same::<Self>(other).best {
            let mine = self.best.as_ref().map(|(k, _)| k);
            if beats(self.name, self.extreme, key, mine).unwrap_or(false) {
                self.best = Some((key.clone(), value.clone()));
            }
        }
    }

    fn finish(&self) -> Result<Value, EvalError> {
        Ok(self
            .best
            .as_ref()
            .map_or(Value::Null, |(_, value)| value.clone()))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// `first(x)` / `last(x)`: the first or last non-null `x`.
struct Pick {
    last: bool,
    value: Option<Value>,
}

impl Accumulator for Pick {
    fn update(&mut self, args: &[Value]) -> Result<(), EvalError> {
        let v = &args[0];
        if !v.is_null() && (self.last || self.value.is_none()) {
            self.value = Some(v.clone());
        }
        Ok(())
    }

    fn merge(&mut self, other: &dyn Accumulator) {
        if let Some(theirs) = &same::<Self>(other).value {
            if self.last || self.value.is_none() {
                self.value = Some(theirs.clone());
            }
        }
    }

    fn finish(&self) -> Result<Value, EvalError> {
        Ok(self.value.clone().unwrap_or(Value::Null))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Exact distinct count; memory grows with the number of distinct values.
#[derive(Default)]
struct CountDistinct {
    seen: HashSet<String>,
}

impl Accumulator for CountDistinct {
    fn update(&mut self, args: &[Value]) -> Result<(), EvalError> {
        if !args[0].is_null() {
            let mut key = String::new();
            canonical(&args[0], &mut key);
            self.seen.insert(key);
        }
        Ok(())
    }

    fn merge(&mut self, other: &dyn Accumulator) {
        self.seen.extend(same::<Self>(other).seen.iter().cloned());
    }

    fn finish(&self) -> Result<Value, EvalError> {
        Ok(Value::from(self.seen.len()))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A string that is equal for values that compare equal: numbers by value
/// (`1 == 1.0`), objects regardless of key order.
fn canonical(v: &Value, out: &mut String) {
    match v {
        Value::Number(n) => {
            let n = json_to_num(n);
            match as_int(n) {
                Some(i) => out.push_str(&i.to_string()),
                None => {
                    let f = as_f64(n);
                    if f.fract() == 0.0 && f.abs() < 1e19 {
                        out.push_str(&(f as i128).to_string());
                    } else {
                        out.push_str(&format!("{:?}", f));
                    }
                }
            }
        }
        Value::Array(items) => {
            out.push('[');
            for item in items {
                canonical(item, out);
                out.push(',');
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_unstable_by_key(|(k, _)| *k);
            out.push('{');
            for (k, item) in entries {
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                canonical(item, out);
                out.push(',');
            }
            out.push('}');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// `collect(x)`: the non-null `x`s as an array, in input order.
#[derive(Default)]
struct Collect {
    items: Vec<Value>,
}

impl Accumulator for Collect {
    fn update(&mut self, args: &[Value]) -> Result<(), EvalError> {
        if !args[0].is_null() {
            self.items.push(args[0].clone());
        }
        Ok(())
    }

    fn merge(&mut self, other: &dyn Accumulator) {
        self.items.extend(same::<Self>(other).items.iter().cloned());
    }

    fn finish(&self) -> Result<Value, EvalError> {
        Ok(if self.items.is_empty() {
            Value::Null
        } else {
            Value::Array(self.items.clone())
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::ExprParser;
    use serde_json::json;

    fn parse(src: &str) -> Expr {
        *ExprParser::new().parse(src).unwrap()
    }

    fn compile(items: &[&str], by: &[&str]) -> Result<AggregateList, ResolveError> {
        let items: Vec<_> = items.iter().map(|s| parse(s)).collect();
        let by: Vec<_> = by.iter().map(|s| parse(s)).collect();
        AggregateList::compile(&FunctionRegistry::default(), &items, &by)
    }

    /// Run `items` over `records` as one group; errors become `"error"`.
    fn run(items: &[&str], records: &[Value]) -> Vec<Value> {
        let list = compile(items, &[]).unwrap();
        let mut state = list.start();
        for r in records {
            list.update(&mut state, r, |_, _| {});
        }
        list.finish(&state)
            .into_iter()
            .map(|r| r.unwrap_or_else(|_| json!("error")))
            .collect()
    }

    fn one(item: &str, records: &[Value]) -> Value {
        run(&[item], records).remove(0)
    }

    fn xs(values: &[Value]) -> Vec<Value> {
        values.iter().map(|x| json!({ "x": x })).collect()
    }

    #[test]
    fn test_count() {
        let records = xs(&[json!(1), Value::Null, json!("a")]);
        assert_eq!(one("count()", &records), json!(3));
        // a missing field and null are both skipped
        assert_eq!(
            one("count(d.x)", &[records.clone(), vec![json!({})]].concat()),
            json!(2)
        );
        assert_eq!(one("count()", &[]), json!(0));
    }

    #[test]
    fn test_sum_and_avg() {
        let ints = xs(&[json!(1), json!(2), Value::Null, json!(4)]);
        assert_eq!(one("sum(d.x)", &ints), json!(7));
        assert_eq!(one("avg(d.x)", &ints), json!(7.0 / 3.0));
        let mixed = xs(&[json!(1), json!(2.5)]);
        assert_eq!(one("sum(d.x)", &mixed), json!(3.5));
        assert_eq!(one("sum(d.x)", &[]), Value::Null);
        assert_eq!(one("avg(d.x)", &[]), Value::Null);
        // integers beyond i64 stay exact while they fit in u64
        let big = xs(&[json!(u64::MAX - 1), json!(1)]);
        assert_eq!(one("sum(d.x)", &big), json!(u64::MAX));
        let negative = xs(&[json!(-5), json!(2)]);
        assert_eq!(one("sum(d.x)", &negative), json!(-3));
    }

    #[test]
    fn test_type_errors_skip_the_record() {
        let list = compile(&["sum(d.x)", "count()"], &[]).unwrap();
        let mut state = list.start();
        let mut errors = vec![];
        for r in xs(&[json!(1), json!("two"), json!(3)]) {
            list.update(&mut state, &r, |item, e| errors.push((item, e)));
        }
        assert_eq!(
            errors,
            vec![(
                0,
                EvalError::Type {
                    op: "sum",
                    left: "string",
                    right: None
                }
            )]
        );
        assert_eq!(list.finish(&state), vec![Ok(json!(4)), Ok(json!(3))]);
    }

    #[test]
    fn test_min_max() {
        let records = xs(&[json!(3), json!(-1.5), Value::Null, json!(10)]);
        assert_eq!(one("min(d.x)", &records), json!(-1.5));
        assert_eq!(one("max(d.x)", &records), json!(10));
        let strings = xs(&[json!("b"), json!("a"), json!("c")]);
        assert_eq!(
            run(&["min(d.x)", "max(d.x)"], &strings),
            vec![json!("a"), json!("c")]
        );
        assert_eq!(one("max(d.x)", &[]), Value::Null);
        // mixed types can't be ordered
        let mixed = xs(&[json!(1), json!("a"), json!(2)]);
        assert_eq!(one("max(d.x)", &mixed), json!(2));
    }

    #[test]
    fn test_variance() {
        let records = xs(&[
            json!(2),
            json!(4),
            json!(4),
            json!(4),
            json!(5),
            json!(5),
            json!(7),
            json!(9),
        ]);
        assert_eq!(one("var_pop(d.x)", &records), json!(4.0));
        assert_eq!(one("stddev_pop(d.x)", &records), json!(2.0));
        assert_eq!(one("var(d.x)", &records), json!(32.0 / 7.0));
        assert_eq!(one("stddev(d.x)", &records), json!((32.0f64 / 7.0).sqrt()));
        assert_eq!(one("var(d.x)", &xs(&[json!(1)])), Value::Null);
        assert_eq!(one("var_pop(d.x)", &xs(&[json!(1)])), json!(0.0));
    }

    #[test]
    fn test_first_last_arg() {
        let records = vec![
            json!({"k": 2, "v": "a"}),
            json!({"k": 5, "v": "b"}),
            json!({"k": null, "v": "c"}),
            json!({"k": 5, "v": "d"}),
            json!({"k": 1}),
        ];
        assert_eq!(
            run(
                &[
                    "first(d.v)",
                    "last(d.v)",
                    "argmax(d.k, d.v)",
                    "argmin(d.k, d.v)"
                ],
                &records
            ),
            // ties keep the first; argmin keeps the missing value as null
            vec![json!("a"), json!("d"), json!("b"), Value::Null]
        );
    }

    #[test]
    fn test_unbounded() {
        let records = xs(&[
            json!(1),
            json!(1.0),
            json!("1"),
            Value::Null,
            json!({"a": 1, "b": 2}),
        ]);
        assert_eq!(one("count_distinct(d.x)", &records), json!(3));
        assert_eq!(one("collect(d.x)", &records[..3]), json!([1, 1.0, "1"]));
        assert_eq!(one("collect(d.x)", &[]), Value::Null);
        let list = compile(&["collect(d.x)", "count_distinct(d.y)", "sum(d.x)"], &[]).unwrap();
        assert_eq!(
            list.unbounded_functions(),
            vec!["collect", "count_distinct"]
        );
    }

    #[test]
    fn test_scalar_around_and_inside_aggregates() {
        let records = vec![
            json!({"price": 2, "qty": 3}),
            json!({"price": 1.5, "qty": 2}),
        ];
        assert_eq!(
            run(
                &[
                    "sum(d.price * d.qty)",
                    "floor(avg(d.qty))",
                    "count() * 10 + 1",
                    "sum(d.qty) / count()",
                    "sum(d.qty) / 0",
                ],
                &records
            ),
            vec![json!(9.0), json!(2), json!(21), json!(2.5), json!("error")]
        );
    }

    #[test]
    fn test_bare_fields_use_the_extreme_row() {
        let records = vec![
            json!({"name": "Ada", "amount": 3}),
            json!({"name": "Grace", "amount": 9}),
            json!({"name": "Alan", "amount": 9}),
            json!({"name": "Edsger"}),
        ];
        assert_eq!(
            run(
                &["max(d.amount)", "d.name", "toupper(d.name) + \"!\""],
                &records
            ),
            vec![json!(9), json!("Grace"), json!("GRACE!")]
        );
        assert_eq!(
            run(&["d.name", "min(d.amount)", "count()"], &records),
            vec![json!("Ada"), json!(3), json!(4)]
        );
    }

    #[test]
    fn test_bare_fields_without_one_extreme_are_errors() {
        assert_eq!(
            compile(&["sum(d.amount)", "d.name"], &[]).unwrap_err(),
            ResolveError::Bare { item: 2 }
        );
        assert_eq!(
            compile(&["max(d.a)", "max(d.b)", "d.name"], &[]).unwrap_err(),
            ResolveError::Bare { item: 3 }
        );
        assert_eq!(
            compile(&["count() + d.x"], &[]).unwrap_err(),
            ResolveError::Bare { item: 1 }
        );
    }

    #[test]
    fn test_grouping_keys_may_be_bare() {
        assert_eq!(
            compile(&["d.state", "nope(d.state)", "count()"], &["d.state"]).unwrap_err(),
            ResolveError::UnknownFunction {
                name: "nope".into()
            }
        );
        let list = compile(
            &["d.state", "toupper(d.state)", "floor(d.x / 10)", "count()"],
            &["d.state", "floor(d.x / 10)"],
        )
        .unwrap();
        let mut state = list.start();
        for r in [
            json!({"state": "mn", "x": 31}),
            json!({"state": "mn", "x": 35}),
        ] {
            list.update(&mut state, &r, |_, e| panic!("{}", e));
        }
        assert_eq!(
            list.finish(&state),
            vec![Ok(json!("mn")), Ok(json!("MN")), Ok(json!(3)), Ok(json!(2))]
        );
        // a field that isn't a key is still bare
        assert_eq!(
            compile(&["d.county", "count()"], &["d.state"]).unwrap_err(),
            ResolveError::Bare { item: 1 }
        );
    }

    #[test]
    fn test_nested_aggregates_and_arity() {
        assert_eq!(
            compile(&["sum(max(d.x))"], &[]).unwrap_err(),
            ResolveError::NestedAggregate {
                outer: "sum",
                inner: "max"
            }
        );
        assert_eq!(
            compile(&["max(sum(d.x))", "d.y"], &[])
                .unwrap_err()
                .to_string(),
            "sum() can't be used inside the aggregate max()"
        );
        assert_eq!(
            compile(&["sum(d.x, d.y)"], &[]).unwrap_err().to_string(),
            "sum() takes 1 argument, got 2"
        );
        assert_eq!(
            compile(&["count(d.x, d.y)"], &[]).unwrap_err().to_string(),
            "count() takes 0 to 1 arguments, got 2"
        );
    }

    #[test]
    fn test_scalar_resolve_rejects_aggregates() {
        let registry = FunctionRegistry::default();
        let err = registry.compile("sum(d.x) > 1").unwrap_err();
        assert_eq!(
            err.to_string(),
            "sum() is an aggregate function and can't be used here"
        );
        assert!(registry.contains_aggregate(&parse("1 + round_ish(count())")));
        assert!(!registry.contains_aggregate(&parse("abs(d.x) + nope(1)")));
        let count = registry.get("count").unwrap();
        assert_eq!(count.call(&[]), Err(EvalError::NotScalar { func: "count" }));
    }

    #[test]
    fn test_merge_matches_one_pass() {
        let items = [
            "count()",
            "sum(d.x)",
            "avg(d.x)",
            "min(d.x)",
            "max(d.x)",
            "var(d.x)",
            "stddev_pop(d.x)",
            "first(d.x)",
            "last(d.x)",
            "argmax(d.x, d.i)",
            "count_distinct(d.x)",
            "collect(d.x)",
        ];
        let records: Vec<_> = [4, 8, 15, 16, 23, 42, 8, 4]
            .iter()
            .enumerate()
            .map(|(i, x)| json!({"i": i, "x": x}))
            .collect();
        let list = compile(&items, &[]).unwrap();

        let mut whole = list.start();
        for r in &records {
            list.update(&mut whole, r, |_, e| panic!("{}", e));
        }
        for split in 0..=records.len() {
            let (mut left, mut right) = (list.start(), list.start());
            for r in &records[..split] {
                list.update(&mut left, r, |_, e| panic!("{}", e));
            }
            for r in &records[split..] {
                list.update(&mut right, r, |_, e| panic!("{}", e));
            }
            left.merge(&right);
            let close = |a: &Value, b: &Value| match (a.as_f64(), b.as_f64()) {
                (Some(x), Some(y)) => (x - y).abs() < 1e-9,
                _ => a == b,
            };
            for (a, b) in list.finish(&left).iter().zip(list.finish(&whole)) {
                assert!(
                    close(a.as_ref().unwrap(), b.as_ref().unwrap()),
                    "{:?} {:?}",
                    a,
                    b
                );
            }
        }
    }
}
