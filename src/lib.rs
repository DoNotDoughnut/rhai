//! # Rhai - embedded scripting for Rust
//!
//! ![Rhai logo](https://rhai.rs/book/images/logo/rhai-banner-transparent-colour.svg)
//!
//! Rhai is a tiny, simple and fast embedded scripting language for Rust
//! that gives you a safe and easy way to add scripting to your applications.
//!
//! It provides a familiar syntax based on JavaScript+Rust and a simple Rust interface.
//!
//! # A Quick Example
//!
//! ## Contents of `my_script.rhai`
//!
//! ```ignore
//! /// Brute force factorial function
//! fn factorial(x) {
//!     if x == 1 { return 1; }
//!     x * factorial(x - 1)
//! }
//!
//! // Calling an external function 'compute'
//! compute(factorial(10))
//! ```
//!
//! ## The Rust part
//!
//! ```no_run
//! use rhai::{Engine, EvalAltResult};
//!
//! fn main() -> Result<(), Box<EvalAltResult>>
//! {
//!     // Define external function
//!     fn compute_something(x: i64) -> bool {
//!         (x % 40) == 0
//!     }
//!
//!     // Create scripting engine
//!     let mut engine = Engine::new();
//!
//!     // Register external function as 'compute'
//!     engine.register_fn("compute", compute_something);
//!
//! #   #[cfg(not(feature = "no_std"))]
//! #   #[cfg(not(any(target_arch = "wasm32", target_arch = "wasm64")))]
//!     assert_eq!(
//!         // Evaluate the script, expects a 'bool' return
//!         engine.eval_file::<bool>("my_script.rhai".into())?,
//!         true
//!     );
//!
//!     Ok(())
//! }
//! ```
//!
//! # Documentation
//!
//! See [The Rhai Book](https://rhai.rs/book) for details on the Rhai scripting engine and language.

#![cfg_attr(feature = "no_std", no_std)]

#[cfg(feature = "no_std")]
extern crate alloc;

#[cfg(feature = "no_std")]
extern crate no_std_compat as std;

#[cfg(feature = "no_std")]
use std::prelude::v1::*;

// Internal modules

mod api;
mod ast;
mod custom_syntax;
mod engine;
mod func;
mod module;
mod optimizer;
pub mod packages;
mod parser;
mod tests;
mod tokenizer;
mod types;
mod r#unsafe;

type RhaiResult = Result<Dynamic, Box<EvalAltResult>>;

/// The system integer type. It is defined as [`i64`].
///
/// If the `only_i32` feature is enabled, this will be [`i32`] instead.
#[cfg(not(feature = "only_i32"))]
pub type INT = i64;

/// The system integer type.
/// It is defined as [`i32`] since the `only_i32` feature is used.
///
/// If the `only_i32` feature is not used, this will be `i64` instead.
#[cfg(feature = "only_i32")]
pub type INT = i32;

/// The system floating-point type. It is defined as [`f64`].
/// Not available under `no_float`.
///
/// If the `f32_float` feature is enabled, this will be [`f32`] instead.
#[cfg(not(feature = "no_float"))]
#[cfg(not(feature = "f32_float"))]
pub type FLOAT = f64;

/// The system floating-point type.
/// It is defined as [`f32`] since the `f32_float` feature is used.
/// Not available under `no_float`.
///
/// If the `f32_float` feature is not used, this will be `f64` instead.
#[cfg(not(feature = "no_float"))]
#[cfg(feature = "f32_float")]
pub type FLOAT = f32;

pub use ast::{FnAccess, AST};
pub use custom_syntax::Expression;
pub use engine::{Engine, EvalContext, OP_CONTAINS, OP_EQUALS};
pub use func::{NativeCallContext, RegisterNativeFunction};
pub use module::{FnNamespace, Module};
pub use tokenizer::Position;
pub use types::{
    Dynamic, EvalAltResult, FnPtr, ImmutableString, LexError, ParseError, ParseErrorType, Scope,
};

/// An identifier in Rhai. [`SmartString`](https://crates.io/crates/smartstring) is used because most
/// identifiers are ASCII and short, fewer than 23 characters, so they can be stored inline.
#[cfg(not(feature = "internals"))]
pub(crate) type Identifier = SmartString;

/// An identifier in Rhai. [`SmartString`](https://crates.io/crates/smartstring) is used because most
/// identifiers are ASCII and short, fewer than 23 characters, so they can be stored inline.
#[cfg(feature = "internals")]
pub type Identifier = SmartString;

/// Alias to [`Rc`][std::rc::Rc] or [`Arc`][std::sync::Arc] depending on the `sync` feature flag.
pub use func::Shared;

/// Alias to [`RefCell`][std::cell::RefCell] or [`RwLock`][std::sync::RwLock] depending on the `sync` feature flag.
pub use func::Locked;

pub(crate) use func::{
    calc_fn_hash, calc_fn_params_hash, calc_qualified_fn_hash, calc_qualified_var_hash,
    combine_hashes,
};

pub use rhai_codegen::*;

pub use func::{plugin, FuncArgs};

#[cfg(not(feature = "no_function"))]
pub use func::Func;

#[cfg(not(feature = "no_function"))]
pub use ast::ScriptFnMetadata;

/// Variable-sized array of [`Dynamic`] values.
/// Not available under `no_index`.
#[cfg(not(feature = "no_index"))]
pub type Array = Vec<Dynamic>;

/// Variable-sized array of [`u8`] values (byte array).
/// Not available under `no_index`.
#[cfg(not(feature = "no_index"))]
pub type Blob = Vec<u8>;

/// Hash map of [`Dynamic`] values with [`SmartString`](https://crates.io/crates/smartstring) keys.
/// Not available under `no_object`.
#[cfg(not(feature = "no_object"))]
pub type Map = std::collections::BTreeMap<Identifier, Dynamic>;

#[cfg(not(feature = "no_module"))]
pub use module::ModuleResolver;

/// Module containing all built-in _module resolvers_ available to Rhai.
#[cfg(not(feature = "no_module"))]
pub use module::resolvers as module_resolvers;

#[cfg(feature = "serde")]
pub mod serde;

#[cfg(not(feature = "no_optimize"))]
pub use optimizer::OptimizationLevel;

// Expose internal data structures.

#[cfg(feature = "internals")]
pub use types::dynamic::{AccessMode, DynamicReadLock, DynamicWriteLock, Variant};

#[cfg(feature = "internals")]
pub use tokenizer::{get_next_token, parse_string_literal};

#[cfg(feature = "internals")]
pub use tokenizer::{
    InputStream, MultiInputsStream, Token, TokenIterator, TokenizeState, TokenizerControl,
    TokenizerControlBlock,
};

#[cfg(feature = "internals")]
pub use parser::{IdentifierBuilder, ParseState};

#[cfg(feature = "internals")]
pub use ast::{
    ASTNode, BinaryExpr, CustomExpr, Expr, FnCallExpr, FnCallHashes, Ident, OpAssignment,
    OptionFlags, ScriptFnDef, Stmt, StmtBlock, AST_OPTION_FLAGS::*,
};

#[cfg(feature = "internals")]
#[cfg(not(feature = "no_float"))]
pub use ast::FloatWrapper;

#[cfg(feature = "internals")]
pub use engine::{EvalState, FnResolutionCache, FnResolutionCacheEntry, Imports};

#[cfg(feature = "internals")]
pub use module::NamespaceRef;

/// Alias to [`smallvec::SmallVec<[T; 3]>`](https://crates.io/crates/smallvec), which is a
/// specialized [`Vec`] backed by a small, inline, fixed-size array when there are ≤ 3 items stored.
///
/// # History
///
/// And Saint Attila raised the `SmallVec` up on high, saying, "O Lord, bless this Thy `SmallVec`
/// that, with it, Thou mayest blow Thine allocation costs to tiny bits in Thy mercy."
///
/// And the Lord did grin, and the people did feast upon the lambs and sloths and carp and anchovies
/// and orangutans and breakfast cereals and fruit bats and large chu...
///
/// And the Lord spake, saying, "First shalt thou depend on the [`smallvec`](https://crates.io/crates/smallvec) crate.
/// Then, shalt thou keep three inline. No more. No less. Three shalt be the number thou shalt keep inline,
/// and the number to keep inline shalt be three. Four shalt thou not keep inline, nor either keep inline
/// thou two, excepting that thou then proceed to three. Five is right out. Once the number three,
/// being the third number, be reached, then, lobbest thou thy `SmallVec` towards thy heap, who,
/// being slow and cache-naughty in My sight, shall snuff it."
///
/// # Why Three
///
/// `StaticVec` is used frequently to keep small lists of items in inline (non-heap) storage in
/// order to improve cache friendliness and reduce indirections.
///
/// The number 3, other than being the holy number, is carefully chosen for a balance between
/// storage space and reduce allocations. That is because most function calls (and most functions,
/// for that matter) contain fewer than 4 arguments, the exception being closures that capture a
/// large number of external variables.
///
/// In addition, most script blocks either contain many statements, or just one or two lines;
/// most scripts load fewer than 4 external modules; most module paths contain fewer than 4 levels
/// (e.g. `std::collections::map::HashMap` is 4 levels and it is just about as long as they get).
#[cfg(not(feature = "internals"))]
type StaticVec<T> = smallvec::SmallVec<[T; 3]>;

/// _(internals)_ Alias to [`smallvec::SmallVec<[T; 3]>`](https://crates.io/crates/smallvec),
/// which is a [`Vec`] backed by a small, inline, fixed-size array when there are ≤ 3 items stored.
/// Exported under the `internals` feature only.
///
/// # History
///
/// And Saint Attila raised the `SmallVec` up on high, saying, "O Lord, bless this Thy `SmallVec`
/// that, with it, Thou mayest blow Thine allocation costs to tiny bits in Thy mercy."
///
/// And the Lord did grin, and the people did feast upon the lambs and sloths and carp and anchovies
/// and orangutans and breakfast cereals and fruit bats and large chu...
///
/// And the Lord spake, saying, "First shalt thou depend on the [`smallvec`](https://crates.io/crates/smallvec) crate.
/// Then, shalt thou keep three inline. No more. No less. Three shalt be the number thou shalt keep inline,
/// and the number to keep inline shalt be three. Four shalt thou not keep inline, nor either keep inline
/// thou two, excepting that thou then proceed to three. Five is right out. Once the number three,
/// being the third number, be reached, then, lobbest thou thy `SmallVec` towards thy heap, who,
/// being slow and cache-naughty in My sight, shall snuff it."
///
/// # Why Three
///
/// `StaticVec` is used frequently to keep small lists of items in inline (non-heap) storage in
/// order to improve cache friendliness and reduce indirections.
///
/// The number 3, other than being the holy number, is carefully chosen for a balance between
/// storage space and reduce allocations. That is because most function calls (and most functions,
/// for that matter) contain fewer than 4 arguments, the exception being closures that capture a
/// large number of external variables.
///
/// In addition, most script blocks either contain many statements, or just one or two lines;
/// most scripts load fewer than 4 external modules; most module paths contain fewer than 4 levels
/// (e.g. `std::collections::map::HashMap` is 4 levels and it is just about as long as they get).
#[cfg(feature = "internals")]
pub type StaticVec<T> = smallvec::SmallVec<[T; 3]>;

pub(crate) type SmartString = smartstring::SmartString<smartstring::LazyCompact>;

// Compiler guards against mutually-exclusive feature flags

#[cfg(feature = "no_float")]
#[cfg(feature = "f32_float")]
compile_error!("`f32_float` cannot be used with `no_float`");

#[cfg(feature = "only_i32")]
#[cfg(feature = "only_i64")]
compile_error!("`only_i32` and `only_i64` cannot be used together");

#[cfg(feature = "no_std")]
#[cfg(feature = "wasm-bindgen")]
compile_error!("`wasm-bindgen` cannot be used with `no-std`");

#[cfg(feature = "no_std")]
#[cfg(feature = "stdweb")]
compile_error!("`stdweb` cannot be used with `no-std`");

#[cfg(any(target_arch = "wasm32", target_arch = "wasm64"))]
#[cfg(feature = "no_std")]
compile_error!("`no_std` cannot be used for WASM target");

#[cfg(not(any(target_arch = "wasm32", target_arch = "wasm64")))]
#[cfg(feature = "wasm-bindgen")]
compile_error!("`wasm-bindgen` cannot be used for non-WASM target");

#[cfg(not(any(target_arch = "wasm32", target_arch = "wasm64")))]
#[cfg(feature = "stdweb")]
compile_error!("`stdweb` cannot be used non-WASM target");

#[cfg(feature = "wasm-bindgen")]
#[cfg(feature = "stdweb")]
compile_error!("`wasm-bindgen` and `stdweb` cannot be used together");
