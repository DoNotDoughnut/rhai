//! Module defining Rhai data types.

pub mod dynamic;
pub mod error;
pub mod fn_ptr;
pub mod immutable_string;
pub mod parse_error;
pub mod scope;

pub use dynamic::Dynamic;
pub use error::EvalAltResult;
pub use fn_ptr::FnPtr;
pub use immutable_string::ImmutableString;
pub use parse_error::{LexError, ParseError, ParseErrorType};
pub use scope::Scope;
