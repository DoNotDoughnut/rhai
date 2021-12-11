//! Module defining interfaces to native-Rust functions.

use super::call::FnCallArgs;
use crate::ast::{FnAccess, FnCallHashes};
use crate::engine::{EvalState, Imports};
use crate::plugin::PluginFunction;
use crate::tokenizer::{Token, TokenizeState};
use crate::{
    calc_fn_hash, Dynamic, Engine, EvalAltResult, EvalContext, Module, Position, RhaiResult,
};
use std::fmt;
#[cfg(feature = "no_std")]
use std::prelude::v1::*;

/// Trait that maps to `Send + Sync` only under the `sync` feature.
#[cfg(feature = "sync")]
pub trait SendSync: Send + Sync {}
/// Trait that maps to `Send + Sync` only under the `sync` feature.
#[cfg(feature = "sync")]
impl<T: Send + Sync> SendSync for T {}

/// Trait that maps to `Send + Sync` only under the `sync` feature.
#[cfg(not(feature = "sync"))]
pub trait SendSync {}
/// Trait that maps to `Send + Sync` only under the `sync` feature.
#[cfg(not(feature = "sync"))]
impl<T> SendSync for T {}

/// Immutable reference-counted container.
#[cfg(not(feature = "sync"))]
pub use std::rc::Rc as Shared;
/// Immutable reference-counted container.
#[cfg(feature = "sync")]
pub use std::sync::Arc as Shared;

/// Synchronized shared object.
#[cfg(not(feature = "sync"))]
#[allow(dead_code)]
pub use std::cell::RefCell as Locked;

/// Lock guard for synchronized shared object.
#[cfg(not(feature = "sync"))]
#[allow(dead_code)]
pub type LockGuard<'a, T> = std::cell::RefMut<'a, T>;

/// Synchronized shared object.
#[cfg(feature = "sync")]
#[allow(dead_code)]
pub use std::sync::RwLock as Locked;

/// Lock guard for synchronized shared object.
#[cfg(feature = "sync")]
#[allow(dead_code)]
pub type LockGuard<'a, T> = std::sync::RwLockWriteGuard<'a, T>;

/// Context of a native Rust function call.
#[derive(Debug)]
pub struct NativeCallContext<'a> {
    engine: &'a Engine,
    fn_name: &'a str,
    source: Option<&'a str>,
    mods: Option<&'a Imports>,
    lib: &'a [&'a Module],
    pos: Position,
}

impl<'a, M: AsRef<[&'a Module]> + ?Sized, S: AsRef<str> + 'a + ?Sized>
    From<(
        &'a Engine,
        &'a S,
        Option<&'a S>,
        &'a Imports,
        &'a M,
        Position,
    )> for NativeCallContext<'a>
{
    #[inline(always)]
    fn from(
        value: (
            &'a Engine,
            &'a S,
            Option<&'a S>,
            &'a Imports,
            &'a M,
            Position,
        ),
    ) -> Self {
        Self {
            engine: value.0,
            fn_name: value.1.as_ref(),
            source: value.2.map(|v| v.as_ref()),
            mods: Some(value.3),
            lib: value.4.as_ref(),
            pos: value.5,
        }
    }
}

impl<'a, M: AsRef<[&'a Module]> + ?Sized, S: AsRef<str> + 'a + ?Sized>
    From<(&'a Engine, &'a S, &'a M)> for NativeCallContext<'a>
{
    #[inline(always)]
    fn from(value: (&'a Engine, &'a S, &'a M)) -> Self {
        Self {
            engine: value.0,
            fn_name: value.1.as_ref(),
            source: None,
            mods: None,
            lib: value.2.as_ref(),
            pos: Position::NONE,
        }
    }
}

impl<'a> NativeCallContext<'a> {
    /// _(internals)_ Create a new [`NativeCallContext`].
    /// Exported under the `metadata` feature only.
    #[deprecated(
        since = "1.3.0",
        note = "`NativeCallContext::new` will be moved under `internals`. Use `FnPtr::call` to call a function pointer directly."
    )]
    #[inline(always)]
    #[must_use]
    pub fn new(
        engine: &'a Engine,
        fn_name: &'a (impl AsRef<str> + 'a + ?Sized),
        lib: &'a [&Module],
    ) -> Self {
        Self {
            engine,
            fn_name: fn_name.as_ref(),
            source: None,
            mods: None,
            lib,
            pos: Position::NONE,
        }
    }
    /// _(internals)_ Create a new [`NativeCallContext`].
    /// Exported under the `internals` feature only.
    ///
    /// Not available under `no_module`.
    #[cfg(feature = "internals")]
    #[cfg(not(feature = "no_module"))]
    #[inline(always)]
    #[must_use]
    pub fn new_with_all_fields(
        engine: &'a Engine,
        fn_name: &'a (impl AsRef<str> + 'a + ?Sized),
        source: Option<&'a (impl AsRef<str> + 'a + ?Sized)>,
        imports: &'a Imports,
        lib: &'a [&Module],
        pos: Position,
    ) -> Self {
        Self {
            engine,
            fn_name: fn_name.as_ref(),
            source: source.map(|v| v.as_ref()),
            mods: Some(imports),
            lib,
            pos,
        }
    }
    /// The current [`Engine`].
    #[inline(always)]
    #[must_use]
    pub const fn engine(&self) -> &Engine {
        self.engine
    }
    /// Name of the function called.
    #[inline(always)]
    #[must_use]
    pub const fn fn_name(&self) -> &str {
        self.fn_name
    }
    /// [Position][`Position`] of the function call.
    #[inline(always)]
    #[must_use]
    pub const fn position(&self) -> Position {
        self.pos
    }
    /// The current source.
    #[inline(always)]
    #[must_use]
    pub const fn source(&self) -> Option<&str> {
        self.source
    }
    /// Get an iterator over the current set of modules imported via `import` statements.
    ///
    /// Not available under `no_module`.
    #[cfg(not(feature = "no_module"))]
    #[inline]
    pub fn iter_imports(&self) -> impl Iterator<Item = (&str, &Module)> {
        self.mods.iter().flat_map(|&m| m.iter())
    }
    /// Get an iterator over the current set of modules imported via `import` statements.
    #[cfg(not(feature = "no_module"))]
    #[allow(dead_code)]
    #[inline]
    pub(crate) fn iter_imports_raw(
        &self,
    ) -> impl Iterator<Item = (&crate::Identifier, &Shared<Module>)> {
        self.mods.iter().flat_map(|&m| m.iter_raw())
    }
    /// _(internals)_ The current set of modules imported via `import` statements.
    /// Exported under the `internals` feature only.
    ///
    /// Not available under `no_module`.
    #[cfg(feature = "internals")]
    #[cfg(not(feature = "no_module"))]
    #[inline(always)]
    #[must_use]
    pub const fn imports(&self) -> Option<&Imports> {
        self.mods
    }
    /// Get an iterator over the namespaces containing definitions of all script-defined functions.
    #[inline]
    pub fn iter_namespaces(&self) -> impl Iterator<Item = &Module> {
        self.lib.iter().cloned()
    }
    /// _(internals)_ The current set of namespaces containing definitions of all script-defined functions.
    /// Exported under the `internals` feature only.
    #[cfg(feature = "internals")]
    #[inline(always)]
    #[must_use]
    pub const fn namespaces(&self) -> &[&Module] {
        self.lib
    }
    /// Call a function inside the call context.
    ///
    /// If `is_method_call` is [`true`], the first argument is assumed to be the `this` pointer for
    /// a script-defined function (or the object of a method call).
    ///
    /// # WARNING - Low Level API
    ///
    /// This function is very low level.
    ///
    /// # Arguments
    ///
    /// All arguments may be _consumed_, meaning that they may be replaced by `()`. This is to avoid
    /// unnecessarily cloning the arguments.
    ///
    /// **DO NOT** reuse the arguments after this call. If they are needed afterwards, clone them
    /// _before_ calling this function.
    ///
    /// If `is_ref_mut` is [`true`], the first argument is assumed to be passed by reference and is
    /// not consumed.
    pub fn call_fn_raw(
        &self,
        fn_name: impl AsRef<str>,
        is_ref_mut: bool,
        is_method_call: bool,
        args: &mut [&mut Dynamic],
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        let fn_name = fn_name.as_ref();

        let hash = if is_method_call {
            FnCallHashes::from_all(
                #[cfg(not(feature = "no_function"))]
                calc_fn_hash(fn_name, args.len() - 1),
                calc_fn_hash(fn_name, args.len()),
            )
        } else {
            calc_fn_hash(fn_name, args.len()).into()
        };

        self.engine()
            .exec_fn_call(
                &mut self.mods.cloned().unwrap_or_else(|| Imports::new()),
                &mut EvalState::new(),
                self.lib,
                fn_name,
                hash,
                args,
                is_ref_mut,
                is_method_call,
                Position::NONE,
                None,
                0,
            )
            .map(|(r, _)| r)
    }
}

/// Consume a [`Shared`] resource and return a mutable reference to the wrapped value.
/// If the resource is shared (i.e. has other outstanding references), a cloned copy is used.
#[inline(always)]
#[must_use]
#[allow(dead_code)]
pub fn shared_make_mut<T: Clone>(value: &mut Shared<T>) -> &mut T {
    Shared::make_mut(value)
}

/// Consume a [`Shared`] resource if is unique (i.e. not shared), or clone it otherwise.
#[inline]
#[must_use]
#[allow(dead_code)]
pub fn shared_take_or_clone<T: Clone>(value: Shared<T>) -> T {
    shared_try_take(value).unwrap_or_else(|v| v.as_ref().clone())
}

/// Consume a [`Shared`] resource if is unique (i.e. not shared).
#[inline(always)]
#[allow(dead_code)]
pub fn shared_try_take<T>(value: Shared<T>) -> Result<T, Shared<T>> {
    Shared::try_unwrap(value)
}

/// Consume a [`Shared`] resource, assuming that it is unique (i.e. not shared).
///
/// # Panics
///
/// Panics if the resource is shared (i.e. has other outstanding references).
#[inline]
#[must_use]
#[allow(dead_code)]
pub fn shared_take<T>(value: Shared<T>) -> T {
    shared_try_take(value).ok().expect("not shared")
}

/// Lock a [`Shared`] resource.
#[inline(always)]
#[must_use]
#[allow(dead_code)]
pub fn shared_write_lock<'a, T>(value: &'a Locked<T>) -> LockGuard<'a, T> {
    #[cfg(not(feature = "sync"))]
    return value.borrow_mut();

    #[cfg(feature = "sync")]
    return value.write().unwrap();
}

/// A general function trail object.
#[cfg(not(feature = "sync"))]
pub type FnAny = dyn Fn(NativeCallContext, &mut FnCallArgs) -> RhaiResult;
/// A general function trail object.
#[cfg(feature = "sync")]
pub type FnAny = dyn Fn(NativeCallContext, &mut FnCallArgs) -> RhaiResult + Send + Sync;

/// A standard function that gets an iterator from a type.
pub type IteratorFn = fn(Dynamic) -> Box<dyn Iterator<Item = Dynamic>>;

#[cfg(not(feature = "sync"))]
pub type FnPlugin = dyn PluginFunction;
#[cfg(feature = "sync")]
pub type FnPlugin = dyn PluginFunction + Send + Sync;

/// A standard callback function for progress reporting.
#[cfg(not(feature = "unchecked"))]
#[cfg(not(feature = "sync"))]
pub type OnProgressCallback = Box<dyn Fn(u64) -> Option<Dynamic> + 'static>;
/// A standard callback function for progress reporting.
#[cfg(not(feature = "unchecked"))]
#[cfg(feature = "sync")]
pub type OnProgressCallback = Box<dyn Fn(u64) -> Option<Dynamic> + Send + Sync + 'static>;

/// A standard callback function for printing.
#[cfg(not(feature = "sync"))]
pub type OnPrintCallback = Box<dyn Fn(&str) + 'static>;
/// A standard callback function for printing.
#[cfg(feature = "sync")]
pub type OnPrintCallback = Box<dyn Fn(&str) + Send + Sync + 'static>;

/// A standard callback function for debugging.
#[cfg(not(feature = "sync"))]
pub type OnDebugCallback = Box<dyn Fn(&str, Option<&str>, Position) + 'static>;
/// A standard callback function for debugging.
#[cfg(feature = "sync")]
pub type OnDebugCallback = Box<dyn Fn(&str, Option<&str>, Position) + Send + Sync + 'static>;

/// A standard callback function for mapping tokens during parsing.
#[cfg(not(feature = "sync"))]
pub type OnParseTokenCallback = dyn Fn(Token, Position, &TokenizeState) -> Token;
/// A standard callback function for mapping tokens during parsing.
#[cfg(feature = "sync")]
pub type OnParseTokenCallback =
    dyn Fn(Token, Position, &TokenizeState) -> Token + Send + Sync + 'static;

/// A standard callback function for variable access.
#[cfg(not(feature = "sync"))]
pub type OnVarCallback =
    Box<dyn Fn(&str, usize, &EvalContext) -> Result<Option<Dynamic>, Box<EvalAltResult>> + 'static>;
/// A standard callback function for variable access.
#[cfg(feature = "sync")]
pub type OnVarCallback = Box<
    dyn Fn(&str, usize, &EvalContext) -> Result<Option<Dynamic>, Box<EvalAltResult>>
        + Send
        + Sync
        + 'static,
>;

/// A type encapsulating a function callable by Rhai.
#[derive(Clone)]
pub enum CallableFunction {
    /// A pure native Rust function with all arguments passed by value.
    Pure(Shared<FnAny>),
    /// A native Rust object method with the first argument passed by reference,
    /// and the rest passed by value.
    Method(Shared<FnAny>),
    /// An iterator function.
    Iterator(IteratorFn),
    /// A plugin function,
    Plugin(Shared<FnPlugin>),
    /// A script-defined function.
    ///
    /// Not available under `no_function`.
    #[cfg(not(feature = "no_function"))]
    Script(Shared<crate::ast::ScriptFnDef>),
}

impl fmt::Debug for CallableFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pure(_) => write!(f, "NativePureFunction"),
            Self::Method(_) => write!(f, "NativeMethod"),
            Self::Iterator(_) => write!(f, "NativeIterator"),
            Self::Plugin(_) => write!(f, "PluginFunction"),

            #[cfg(not(feature = "no_function"))]
            Self::Script(fn_def) => fmt::Debug::fmt(fn_def, f),
        }
    }
}

impl fmt::Display for CallableFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pure(_) => write!(f, "NativePureFunction"),
            Self::Method(_) => write!(f, "NativeMethod"),
            Self::Iterator(_) => write!(f, "NativeIterator"),
            Self::Plugin(_) => write!(f, "PluginFunction"),

            #[cfg(not(feature = "no_function"))]
            CallableFunction::Script(s) => fmt::Display::fmt(s, f),
        }
    }
}

impl CallableFunction {
    /// Is this a pure native Rust function?
    #[inline]
    #[must_use]
    pub fn is_pure(&self) -> bool {
        match self {
            Self::Pure(_) => true,
            Self::Method(_) | Self::Iterator(_) => false,

            Self::Plugin(p) => !p.is_method_call(),

            #[cfg(not(feature = "no_function"))]
            Self::Script(_) => false,
        }
    }
    /// Is this a native Rust method function?
    #[inline]
    #[must_use]
    pub fn is_method(&self) -> bool {
        match self {
            Self::Method(_) => true,
            Self::Pure(_) | Self::Iterator(_) => false,

            Self::Plugin(p) => p.is_method_call(),

            #[cfg(not(feature = "no_function"))]
            Self::Script(_) => false,
        }
    }
    /// Is this an iterator function?
    #[inline]
    #[must_use]
    pub const fn is_iter(&self) -> bool {
        match self {
            Self::Iterator(_) => true,
            Self::Pure(_) | Self::Method(_) | Self::Plugin(_) => false,

            #[cfg(not(feature = "no_function"))]
            Self::Script(_) => false,
        }
    }
    /// Is this a Rhai-scripted function?
    #[inline]
    #[must_use]
    pub const fn is_script(&self) -> bool {
        match self {
            #[cfg(not(feature = "no_function"))]
            Self::Script(_) => true,

            Self::Pure(_) | Self::Method(_) | Self::Iterator(_) | Self::Plugin(_) => false,
        }
    }
    /// Is this a plugin function?
    #[inline]
    #[must_use]
    pub const fn is_plugin_fn(&self) -> bool {
        match self {
            Self::Plugin(_) => true,
            Self::Pure(_) | Self::Method(_) | Self::Iterator(_) => false,

            #[cfg(not(feature = "no_function"))]
            Self::Script(_) => false,
        }
    }
    /// Is this a native Rust function?
    #[inline]
    #[must_use]
    pub const fn is_native(&self) -> bool {
        match self {
            Self::Pure(_) | Self::Method(_) => true,
            Self::Plugin(_) => true,
            Self::Iterator(_) => true,

            #[cfg(not(feature = "no_function"))]
            Self::Script(_) => false,
        }
    }
    /// Get the access mode.
    #[inline]
    #[must_use]
    pub fn access(&self) -> FnAccess {
        match self {
            Self::Plugin(_) => FnAccess::Public,
            Self::Pure(_) | Self::Method(_) | Self::Iterator(_) => FnAccess::Public,

            #[cfg(not(feature = "no_function"))]
            Self::Script(f) => f.access,
        }
    }
    /// Get a shared reference to a native Rust function.
    #[inline]
    #[must_use]
    pub fn get_native_fn(&self) -> Option<&Shared<FnAny>> {
        match self {
            Self::Pure(f) | Self::Method(f) => Some(f),
            Self::Iterator(_) | Self::Plugin(_) => None,

            #[cfg(not(feature = "no_function"))]
            Self::Script(_) => None,
        }
    }
    /// Get a shared reference to a script-defined function definition.
    ///
    /// Not available under `no_function`.
    #[cfg(not(feature = "no_function"))]
    #[inline]
    #[must_use]
    pub const fn get_script_fn_def(&self) -> Option<&Shared<crate::ast::ScriptFnDef>> {
        match self {
            Self::Pure(_) | Self::Method(_) | Self::Iterator(_) | Self::Plugin(_) => None,
            Self::Script(f) => Some(f),
        }
    }
    /// Get a reference to an iterator function.
    #[inline]
    #[must_use]
    pub fn get_iter_fn(&self) -> Option<IteratorFn> {
        match self {
            Self::Iterator(f) => Some(*f),
            Self::Pure(_) | Self::Method(_) | Self::Plugin(_) => None,

            #[cfg(not(feature = "no_function"))]
            Self::Script(_) => None,
        }
    }
    /// Get a shared reference to a plugin function.
    #[inline]
    #[must_use]
    pub fn get_plugin_fn(&self) -> Option<&Shared<FnPlugin>> {
        match self {
            Self::Plugin(f) => Some(f),
            Self::Pure(_) | Self::Method(_) | Self::Iterator(_) => None,

            #[cfg(not(feature = "no_function"))]
            Self::Script(_) => None,
        }
    }
    /// Create a new [`CallableFunction::Pure`].
    #[inline(always)]
    #[must_use]
    pub fn from_pure(func: Box<FnAny>) -> Self {
        Self::Pure(func.into())
    }
    /// Create a new [`CallableFunction::Method`].
    #[inline(always)]
    #[must_use]
    pub fn from_method(func: Box<FnAny>) -> Self {
        Self::Method(func.into())
    }
    /// Create a new [`CallableFunction::Plugin`].
    #[inline(always)]
    #[must_use]
    pub fn from_plugin(func: impl PluginFunction + 'static + SendSync) -> Self {
        Self::Plugin((Box::new(func) as Box<FnPlugin>).into())
    }
}

impl From<IteratorFn> for CallableFunction {
    #[inline(always)]
    fn from(func: IteratorFn) -> Self {
        Self::Iterator(func)
    }
}

#[cfg(not(feature = "no_function"))]
impl From<crate::ast::ScriptFnDef> for CallableFunction {
    #[inline(always)]
    fn from(_func: crate::ast::ScriptFnDef) -> Self {
        Self::Script(_func.into())
    }
}

#[cfg(not(feature = "no_function"))]
impl From<Shared<crate::ast::ScriptFnDef>> for CallableFunction {
    #[inline(always)]
    fn from(_func: Shared<crate::ast::ScriptFnDef>) -> Self {
        Self::Script(_func)
    }
}

impl<T: PluginFunction + 'static + SendSync> From<T> for CallableFunction {
    #[inline(always)]
    fn from(func: T) -> Self {
        Self::from_plugin(func)
    }
}

impl From<Shared<FnPlugin>> for CallableFunction {
    #[inline(always)]
    fn from(func: Shared<FnPlugin>) -> Self {
        Self::Plugin(func)
    }
}
