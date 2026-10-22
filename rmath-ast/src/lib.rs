pub mod aggregate;
pub mod builtins;
pub mod eval;
#[allow(
    clippy::just_underscores_and_digits,
    clippy::clone_on_copy,
    clippy::needless_lifetimes,
    clippy::type_complexity,
    clippy::new_without_default,
    clippy::vec_box
)]
pub mod parser;
pub mod registry;

pub use aggregate::{Accumulator, AggregateList, AggregateState};
pub use eval::EvalError;
pub use registry::{
    Arity, CompileError, CompiledExpr, Extreme, Function, FunctionKind, FunctionRegistry,
    ResolveError,
};
