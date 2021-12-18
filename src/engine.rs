//! Main module defining the script evaluation [`Engine`].

use crate::ast::{Expr, FnCallExpr, Ident, OpAssignment, Stmt, AST_OPTION_FLAGS::*};
use crate::custom_syntax::CustomSyntax;
use crate::func::{
    get_hasher,
    native::{OnDebugCallback, OnParseTokenCallback, OnPrintCallback, OnVarCallback},
    CallableFunction, IteratorFn,
};
use crate::module::NamespaceRef;
use crate::packages::{Package, StandardPackage};
use crate::r#unsafe::unsafe_cast_var_name_to_lifetime;
use crate::tokenizer::Token;
use crate::types::dynamic::{map_std_type_name, AccessMode, Union, Variant};
use crate::{
    calc_fn_params_hash, combine_hashes, Dynamic, EvalAltResult, Identifier, ImmutableString,
    Module, Position, RhaiResult, Scope, Shared, StaticVec, INT,
};
#[cfg(feature = "no_std")]
use std::prelude::v1::*;
use std::{
    any::{type_name, TypeId},
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    fmt,
    hash::{Hash, Hasher},
    iter::{FromIterator, Rev, Zip},
    num::{NonZeroU8, NonZeroUsize},
    ops::{Deref, DerefMut},
};

pub type Precedence = NonZeroU8;

/// _(internals)_ A stack of imported [modules][Module] plus mutable runtime global states.
/// Exported under the `internals` feature only.
//
// # Implementation Notes
//
// This implementation splits the module names from the shared modules to improve data locality.
// Most usage will be looking up a particular key from the list and then getting the module that
// corresponds to that key.
#[derive(Clone)]
pub struct Imports {
    /// Stack of module names.
    //
    // # Implementation Notes
    //
    // We cannot use Cow<str> here because `eval` may load a [module][Module] and
    // the module name will live beyond the AST of the eval script text.
    keys: StaticVec<Identifier>,
    /// Stack of imported modules.
    modules: StaticVec<Shared<Module>>,
    /// Source of the current context.
    pub source: Option<Identifier>,
    /// Number of operations performed.
    pub num_operations: u64,
    /// Number of modules loaded.
    pub num_modules: usize,
    /// Function call hashes to index getters and setters.
    ///
    /// Not available under `no_index` and `no_object`.
    #[cfg(any(not(feature = "no_index"), not(feature = "no_object")))]
    fn_hash_indexing: (u64, u64),
    /// Embedded module resolver.
    ///
    /// Not available under `no_module`.
    #[cfg(not(feature = "no_module"))]
    pub embedded_module_resolver: Option<Shared<crate::module::resolvers::StaticModuleResolver>>,
    /// Cache of globally-defined constants.
    ///
    /// Not available under `no_module` and `no_function`.
    #[cfg(not(feature = "no_module"))]
    #[cfg(not(feature = "no_function"))]
    global_constants: Option<Shared<crate::Locked<BTreeMap<Identifier, Dynamic>>>>,
}

impl Default for Imports {
    #[inline(always)]
    fn default() -> Self {
        Self::new()
    }
}

impl Imports {
    /// Create a new stack of imported [modules][Module].
    #[inline(always)]
    #[must_use]
    pub const fn new() -> Self {
        Self {
            keys: StaticVec::new_const(),
            modules: StaticVec::new_const(),
            source: None,
            num_operations: 0,
            num_modules: 0,
            #[cfg(not(feature = "no_module"))]
            embedded_module_resolver: None,
            #[cfg(any(not(feature = "no_index"), not(feature = "no_object")))]
            fn_hash_indexing: (0, 0),
            #[cfg(not(feature = "no_module"))]
            #[cfg(not(feature = "no_function"))]
            global_constants: None,
        }
    }
    /// Get the length of this stack of imported [modules][Module].
    #[inline(always)]
    #[must_use]
    pub fn len(&self) -> usize {
        self.keys.len()
    }
    /// Is this stack of imported [modules][Module] empty?
    #[inline(always)]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
    /// Get the imported [module][Module] at a particular index.
    #[inline(always)]
    #[must_use]
    pub fn get(&self, index: usize) -> Option<Shared<Module>> {
        self.modules.get(index).cloned()
    }
    /// Get the imported [module][Module] at a particular index.
    #[allow(dead_code)]
    #[inline(always)]
    #[must_use]
    pub(crate) fn get_mut(&mut self, index: usize) -> Option<&mut Shared<Module>> {
        self.modules.get_mut(index)
    }
    /// Get the index of an imported [module][Module] by name.
    #[inline]
    #[must_use]
    pub fn find(&self, name: impl AsRef<str>) -> Option<usize> {
        let name = name.as_ref();
        let len = self.keys.len();

        self.keys.iter().rev().enumerate().find_map(|(i, key)| {
            if key == name {
                Some(len - 1 - i)
            } else {
                None
            }
        })
    }
    /// Push an imported [module][Module] onto the stack.
    #[inline(always)]
    pub fn push(&mut self, name: impl Into<Identifier>, module: impl Into<Shared<Module>>) {
        self.keys.push(name.into());
        self.modules.push(module.into());
    }
    /// Truncate the stack of imported [modules][Module] to a particular length.
    #[inline(always)]
    pub fn truncate(&mut self, size: usize) {
        self.keys.truncate(size);
        self.modules.truncate(size);
    }
    /// Get an iterator to this stack of imported [modules][Module] in reverse order.
    #[allow(dead_code)]
    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Module)> {
        self.keys
            .iter()
            .rev()
            .zip(self.modules.iter().rev())
            .map(|(name, module)| (name.as_str(), module.as_ref()))
    }
    /// Get an iterator to this stack of imported [modules][Module] in reverse order.
    #[allow(dead_code)]
    #[inline]
    pub(crate) fn iter_raw(&self) -> impl Iterator<Item = (&Identifier, &Shared<Module>)> {
        self.keys.iter().rev().zip(self.modules.iter().rev())
    }
    /// Get an iterator to this stack of imported [modules][Module] in forward order.
    #[allow(dead_code)]
    #[inline]
    pub(crate) fn scan_raw(&self) -> impl Iterator<Item = (&Identifier, &Shared<Module>)> {
        self.keys.iter().zip(self.modules.iter())
    }
    /// Does the specified function hash key exist in this stack of imported [modules][Module]?
    #[allow(dead_code)]
    #[inline]
    #[must_use]
    pub fn contains_fn(&self, hash: u64) -> bool {
        self.modules.iter().any(|m| m.contains_qualified_fn(hash))
    }
    /// Get the specified function via its hash key from this stack of imported [modules][Module].
    #[inline]
    #[must_use]
    pub fn get_fn(&self, hash: u64) -> Option<(&CallableFunction, Option<&Identifier>)> {
        self.modules
            .iter()
            .rev()
            .find_map(|m| m.get_qualified_fn(hash).map(|f| (f, m.id_raw())))
    }
    /// Does the specified [`TypeId`][std::any::TypeId] iterator exist in this stack of
    /// imported [modules][Module]?
    #[allow(dead_code)]
    #[inline]
    #[must_use]
    pub fn contains_iter(&self, id: TypeId) -> bool {
        self.modules.iter().any(|m| m.contains_qualified_iter(id))
    }
    /// Get the specified [`TypeId`][std::any::TypeId] iterator from this stack of imported
    /// [modules][Module].
    #[inline]
    #[must_use]
    pub fn get_iter(&self, id: TypeId) -> Option<IteratorFn> {
        self.modules
            .iter()
            .rev()
            .find_map(|m| m.get_qualified_iter(id))
    }
    /// Get a mutable reference to the cache of globally-defined constants.
    #[cfg(not(feature = "no_module"))]
    #[cfg(not(feature = "no_function"))]
    #[must_use]
    pub(crate) fn global_constants_mut<'a>(
        &'a mut self,
    ) -> Option<impl DerefMut<Target = BTreeMap<Identifier, Dynamic>> + 'a> {
        if let Some(ref global_constants) = self.global_constants {
            Some(crate::func::native::shared_write_lock(global_constants))
        } else {
            None
        }
    }
    /// Set a constant into the cache of globally-defined constants.
    #[cfg(not(feature = "no_module"))]
    #[cfg(not(feature = "no_function"))]
    pub(crate) fn set_global_constant(&mut self, name: impl Into<Identifier>, value: Dynamic) {
        if self.global_constants.is_none() {
            let dict: crate::Locked<_> = BTreeMap::new().into();
            self.global_constants = Some(dict.into());
        }

        crate::func::native::shared_write_lock(self.global_constants.as_mut().expect("`Some`"))
            .insert(name.into(), value);
    }
    /// Get the pre-calculated index getter hash.
    #[cfg(any(not(feature = "no_index"), not(feature = "no_object")))]
    #[must_use]
    pub(crate) fn hash_idx_get(&mut self) -> u64 {
        if self.fn_hash_indexing != (0, 0) {
            self.fn_hash_indexing.0
        } else {
            let n1 = crate::calc_fn_hash(FN_IDX_GET, 2);
            let n2 = crate::calc_fn_hash(FN_IDX_SET, 3);
            self.fn_hash_indexing = (n1, n2);
            n1
        }
    }
    /// Get the pre-calculated index setter hash.
    #[cfg(any(not(feature = "no_index"), not(feature = "no_object")))]
    #[must_use]
    pub(crate) fn hash_idx_set(&mut self) -> u64 {
        if self.fn_hash_indexing != (0, 0) {
            self.fn_hash_indexing.1
        } else {
            let n1 = crate::calc_fn_hash(FN_IDX_GET, 2);
            let n2 = crate::calc_fn_hash(FN_IDX_SET, 3);
            self.fn_hash_indexing = (n1, n2);
            n2
        }
    }
}

impl IntoIterator for Imports {
    type Item = (Identifier, Shared<Module>);
    type IntoIter =
        Zip<Rev<smallvec::IntoIter<[Identifier; 3]>>, Rev<smallvec::IntoIter<[Shared<Module>; 3]>>>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.keys
            .into_iter()
            .rev()
            .zip(self.modules.into_iter().rev())
    }
}

impl<K: Into<Identifier>, M: Into<Shared<Module>>> FromIterator<(K, M)> for Imports {
    fn from_iter<T: IntoIterator<Item = (K, M)>>(iter: T) -> Self {
        let mut lib = Self::new();
        lib.extend(iter);
        lib
    }
}

impl<K: Into<Identifier>, M: Into<Shared<Module>>> Extend<(K, M)> for Imports {
    fn extend<T: IntoIterator<Item = (K, M)>>(&mut self, iter: T) {
        iter.into_iter().for_each(|(k, m)| {
            self.keys.push(k.into());
            self.modules.push(m.into());
        })
    }
}

impl fmt::Debug for Imports {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Imports")?;

        if self.is_empty() {
            f.debug_map().finish()
        } else {
            f.debug_map()
                .entries(self.keys.iter().zip(self.modules.iter()))
                .finish()
        }
    }
}

#[cfg(not(feature = "unchecked"))]
#[cfg(debug_assertions)]
#[cfg(not(feature = "no_function"))]
pub const MAX_CALL_STACK_DEPTH: usize = 8;
#[cfg(not(feature = "unchecked"))]
#[cfg(debug_assertions)]
pub const MAX_EXPR_DEPTH: usize = 32;
#[cfg(not(feature = "unchecked"))]
#[cfg(not(feature = "no_function"))]
#[cfg(debug_assertions)]
pub const MAX_FUNCTION_EXPR_DEPTH: usize = 16;

#[cfg(not(feature = "unchecked"))]
#[cfg(not(debug_assertions))]
#[cfg(not(feature = "no_function"))]
pub const MAX_CALL_STACK_DEPTH: usize = 64;
#[cfg(not(feature = "unchecked"))]
#[cfg(not(debug_assertions))]
pub const MAX_EXPR_DEPTH: usize = 64;
#[cfg(not(feature = "unchecked"))]
#[cfg(not(feature = "no_function"))]
#[cfg(not(debug_assertions))]
pub const MAX_FUNCTION_EXPR_DEPTH: usize = 32;

pub const MAX_DYNAMIC_PARAMETERS: usize = 16;

pub const KEYWORD_PRINT: &str = "print";
pub const KEYWORD_DEBUG: &str = "debug";
pub const KEYWORD_TYPE_OF: &str = "type_of";
pub const KEYWORD_EVAL: &str = "eval";
pub const KEYWORD_FN_PTR: &str = "Fn";
pub const KEYWORD_FN_PTR_CALL: &str = "call";
pub const KEYWORD_FN_PTR_CURRY: &str = "curry";
#[cfg(not(feature = "no_closure"))]
pub const KEYWORD_IS_SHARED: &str = "is_shared";
pub const KEYWORD_IS_DEF_VAR: &str = "is_def_var";
#[cfg(not(feature = "no_function"))]
pub const KEYWORD_IS_DEF_FN: &str = "is_def_fn";
pub const KEYWORD_THIS: &str = "this";
#[cfg(not(feature = "no_function"))]
#[cfg(not(feature = "no_module"))]
pub const KEYWORD_GLOBAL: &str = "global";
#[cfg(not(feature = "no_object"))]
pub const FN_GET: &str = "get$";
#[cfg(not(feature = "no_object"))]
pub const FN_SET: &str = "set$";
#[cfg(any(not(feature = "no_index"), not(feature = "no_object")))]
pub const FN_IDX_GET: &str = "index$get$";
#[cfg(any(not(feature = "no_index"), not(feature = "no_object")))]
pub const FN_IDX_SET: &str = "index$set$";
#[cfg(not(feature = "no_function"))]
pub const FN_ANONYMOUS: &str = "anon$";

/// Standard equality comparison operator.
pub const OP_EQUALS: &str = "==";

/// Standard method function for containment testing.
/// The `in` operator is implemented as a call to this method.
pub const OP_CONTAINS: &str = "contains";

/// Standard exclusive range operator.
pub const OP_EXCLUSIVE_RANGE: &str = "..";

/// Standard inclusive range operator.
pub const OP_INCLUSIVE_RANGE: &str = "..=";

/// Standard concatenation operator token.
pub const TOKEN_OP_CONCAT: Token = Token::PlusAssign;

/// Method of chaining.
#[cfg(any(not(feature = "no_index"), not(feature = "no_object")))]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
enum ChainType {
    /// Indexing.
    #[cfg(not(feature = "no_index"))]
    Indexing,
    /// Dotting.
    #[cfg(not(feature = "no_object"))]
    Dotting,
}

/// Value of a chaining argument.
#[cfg(any(not(feature = "no_index"), not(feature = "no_object")))]
#[derive(Debug, Clone, Hash)]
enum ChainArgument {
    /// Dot-property access.
    #[cfg(not(feature = "no_object"))]
    Property(Position),
    /// Arguments to a dot method call.
    /// Wrapped values are the arguments plus the [position][Position] of the first argument.
    #[cfg(not(feature = "no_object"))]
    MethodCallArgs(StaticVec<Dynamic>, Position),
    /// Index value and [position][Position].
    #[cfg(not(feature = "no_index"))]
    IndexValue(Dynamic, Position),
}

#[cfg(any(not(feature = "no_index"), not(feature = "no_object")))]
impl ChainArgument {
    /// Return the index value.
    #[inline(always)]
    #[cfg(not(feature = "no_index"))]
    #[must_use]
    pub fn into_index_value(self) -> Option<Dynamic> {
        match self {
            Self::IndexValue(value, _) => Some(value),
            #[cfg(not(feature = "no_object"))]
            _ => None,
        }
    }
    /// Return the list of method call arguments.
    #[inline(always)]
    #[cfg(not(feature = "no_object"))]
    #[must_use]
    pub fn into_fn_call_args(self) -> Option<(StaticVec<Dynamic>, Position)> {
        match self {
            Self::MethodCallArgs(values, pos) => Some((values, pos)),
            _ => None,
        }
    }
    /// Return the [position][Position].
    #[inline(always)]
    #[must_use]
    #[allow(dead_code)]
    pub const fn position(&self) -> Position {
        match self {
            #[cfg(not(feature = "no_object"))]
            ChainArgument::Property(pos) => *pos,
            #[cfg(not(feature = "no_object"))]
            ChainArgument::MethodCallArgs(_, pos) => *pos,
            #[cfg(not(feature = "no_index"))]
            ChainArgument::IndexValue(_, pos) => *pos,
        }
    }
}

#[cfg(not(feature = "no_object"))]
impl From<(StaticVec<Dynamic>, Position)> for ChainArgument {
    #[inline(always)]
    fn from((values, pos): (StaticVec<Dynamic>, Position)) -> Self {
        Self::MethodCallArgs(values, pos)
    }
}

#[cfg(not(feature = "no_index"))]
impl From<(Dynamic, Position)> for ChainArgument {
    #[inline(always)]
    fn from((value, pos): (Dynamic, Position)) -> Self {
        Self::IndexValue(value, pos)
    }
}

/// Get the chaining type for an [`Expr`].
#[cfg(any(not(feature = "no_index"), not(feature = "no_object")))]
fn match_chaining_type(expr: &Expr) -> ChainType {
    match expr {
        #[cfg(not(feature = "no_index"))]
        Expr::Index(_, _, _) => ChainType::Indexing,
        #[cfg(not(feature = "no_object"))]
        Expr::Dot(_, _, _) => ChainType::Dotting,
        _ => unreachable!("`expr` should only be `Index` or `Dot`, but got {:?}", expr),
    }
}

/// A type that encapsulates a mutation target for an expression with side effects.
#[derive(Debug)]
pub enum Target<'a> {
    /// The target is a mutable reference to a `Dynamic` value somewhere.
    RefMut(&'a mut Dynamic),
    /// The target is a mutable reference to a Shared `Dynamic` value.
    /// It holds both the access guard and the original shared value.
    #[cfg(not(feature = "no_closure"))]
    LockGuard(
        (
            crate::types::dynamic::DynamicWriteLock<'a, Dynamic>,
            Dynamic,
        ),
    ),
    /// The target is a temporary `Dynamic` value (i.e. the mutation can cause no side effects).
    TempValue(Dynamic),
    /// The target is a bit inside an [`INT`][crate::INT].
    /// This is necessary because directly pointing to a bit inside an [`INT`][crate::INT] is impossible.
    #[cfg(not(feature = "no_index"))]
    Bit(&'a mut Dynamic, Dynamic, u8),
    /// The target is a range of bits inside an [`INT`][crate::INT].
    /// This is necessary because directly pointing to a range of bits inside an [`INT`][crate::INT] is impossible.
    #[cfg(not(feature = "no_index"))]
    BitField(&'a mut Dynamic, INT, Dynamic, u8),
    /// The target is a byte inside a Blob.
    /// This is necessary because directly pointing to a byte (in [`Dynamic`] form) inside a blob is impossible.
    #[cfg(not(feature = "no_index"))]
    BlobByte(&'a mut Dynamic, usize, Dynamic),
    /// The target is a character inside a String.
    /// This is necessary because directly pointing to a char inside a String is impossible.
    #[cfg(not(feature = "no_index"))]
    StringChar(&'a mut Dynamic, usize, Dynamic),
}

impl<'a> Target<'a> {
    /// Is the `Target` a reference pointing to other data?
    #[allow(dead_code)]
    #[inline]
    #[must_use]
    pub const fn is_ref(&self) -> bool {
        match self {
            Self::RefMut(_) => true,
            #[cfg(not(feature = "no_closure"))]
            Self::LockGuard(_) => true,
            Self::TempValue(_) => false,
            #[cfg(not(feature = "no_index"))]
            Self::Bit(_, _, _)
            | Self::BitField(_, _, _, _)
            | Self::BlobByte(_, _, _)
            | Self::StringChar(_, _, _) => false,
        }
    }
    /// Is the `Target` a temp value?
    #[inline]
    #[must_use]
    pub const fn is_temp_value(&self) -> bool {
        match self {
            Self::RefMut(_) => false,
            #[cfg(not(feature = "no_closure"))]
            Self::LockGuard(_) => false,
            Self::TempValue(_) => true,
            #[cfg(not(feature = "no_index"))]
            Self::Bit(_, _, _)
            | Self::BitField(_, _, _, _)
            | Self::BlobByte(_, _, _)
            | Self::StringChar(_, _, _) => false,
        }
    }
    /// Is the `Target` a shared value?
    #[cfg(not(feature = "no_closure"))]
    #[inline]
    #[must_use]
    pub fn is_shared(&self) -> bool {
        match self {
            Self::RefMut(r) => r.is_shared(),
            #[cfg(not(feature = "no_closure"))]
            Self::LockGuard(_) => true,
            Self::TempValue(r) => r.is_shared(),
            #[cfg(not(feature = "no_index"))]
            Self::Bit(_, _, _)
            | Self::BitField(_, _, _, _)
            | Self::BlobByte(_, _, _)
            | Self::StringChar(_, _, _) => false,
        }
    }
    /// Is the `Target` a specific type?
    #[allow(dead_code)]
    #[inline]
    #[must_use]
    pub fn is<T: Variant + Clone>(&self) -> bool {
        match self {
            Self::RefMut(r) => r.is::<T>(),
            #[cfg(not(feature = "no_closure"))]
            Self::LockGuard((r, _)) => r.is::<T>(),
            Self::TempValue(r) => r.is::<T>(),
            #[cfg(not(feature = "no_index"))]
            Self::Bit(_, _, _) => TypeId::of::<T>() == TypeId::of::<bool>(),
            #[cfg(not(feature = "no_index"))]
            Self::BitField(_, _, _, _) => TypeId::of::<T>() == TypeId::of::<INT>(),
            #[cfg(not(feature = "no_index"))]
            Self::BlobByte(_, _, _) => TypeId::of::<T>() == TypeId::of::<crate::Blob>(),
            #[cfg(not(feature = "no_index"))]
            Self::StringChar(_, _, _) => TypeId::of::<T>() == TypeId::of::<char>(),
        }
    }
    /// Get the value of the `Target` as a `Dynamic`, cloning a referenced value if necessary.
    #[inline]
    #[must_use]
    pub fn take_or_clone(self) -> Dynamic {
        match self {
            Self::RefMut(r) => r.clone(), // Referenced value is cloned
            #[cfg(not(feature = "no_closure"))]
            Self::LockGuard((_, orig)) => orig, // Original value is simply taken
            Self::TempValue(v) => v,      // Owned value is simply taken
            #[cfg(not(feature = "no_index"))]
            Self::Bit(_, value, _) => value, // Boolean is taken
            #[cfg(not(feature = "no_index"))]
            Self::BitField(_, _, value, _) => value, // INT is taken
            #[cfg(not(feature = "no_index"))]
            Self::BlobByte(_, _, value) => value, // Byte is taken
            #[cfg(not(feature = "no_index"))]
            Self::StringChar(_, _, ch) => ch, // Character is taken
        }
    }
    /// Take a `&mut Dynamic` reference from the `Target`.
    #[inline(always)]
    #[must_use]
    pub fn take_ref(self) -> Option<&'a mut Dynamic> {
        match self {
            Self::RefMut(r) => Some(r),
            _ => None,
        }
    }
    /// Convert a shared or reference `Target` into a target with an owned value.
    #[inline(always)]
    #[must_use]
    pub fn into_owned(self) -> Target<'static> {
        self.take_or_clone().into()
    }
    /// Propagate a changed value back to the original source.
    /// This has no effect except for string indexing.
    #[inline]
    pub fn propagate_changed_value(&mut self) -> Result<(), Box<EvalAltResult>> {
        match self {
            Self::RefMut(_) | Self::TempValue(_) => (),
            #[cfg(not(feature = "no_closure"))]
            Self::LockGuard(_) => (),
            #[cfg(not(feature = "no_index"))]
            Self::Bit(value, new_val, index) => {
                // Replace the bit at the specified index position
                let new_bit = new_val.as_bool().map_err(|err| {
                    Box::new(EvalAltResult::ErrorMismatchDataType(
                        "bool".to_string(),
                        err.to_string(),
                        Position::NONE,
                    ))
                })?;

                let value = &mut *value.write_lock::<crate::INT>().expect("`INT`");

                let index = *index;

                let mask = 1 << index;
                if new_bit {
                    *value |= mask;
                } else {
                    *value &= !mask;
                }
            }
            #[cfg(not(feature = "no_index"))]
            Self::BitField(value, mask, new_val, shift) => {
                let shift = *shift;
                let mask = *mask;

                // Replace the bit at the specified index position
                let new_value = new_val.as_int().map_err(|err| {
                    Box::new(EvalAltResult::ErrorMismatchDataType(
                        "integer".to_string(),
                        err.to_string(),
                        Position::NONE,
                    ))
                })?;

                let new_value = (new_value << shift) & mask;
                let value = &mut *value.write_lock::<crate::INT>().expect("`INT`");

                *value &= !mask;
                *value |= new_value;
            }
            #[cfg(not(feature = "no_index"))]
            Self::BlobByte(value, index, new_val) => {
                // Replace the byte at the specified index position
                let new_byte = new_val.as_int().map_err(|err| {
                    Box::new(EvalAltResult::ErrorMismatchDataType(
                        "INT".to_string(),
                        err.to_string(),
                        Position::NONE,
                    ))
                })?;

                let value = &mut *value.write_lock::<crate::Blob>().expect("`Blob`");

                let index = *index;

                if index < value.len() {
                    value[index] = (new_byte & 0x00ff) as u8;
                } else {
                    unreachable!("blob index out of bounds: {}", index);
                }
            }
            #[cfg(not(feature = "no_index"))]
            Self::StringChar(s, index, new_val) => {
                // Replace the character at the specified index position
                let new_ch = new_val.as_char().map_err(|err| {
                    Box::new(EvalAltResult::ErrorMismatchDataType(
                        "char".to_string(),
                        err.to_string(),
                        Position::NONE,
                    ))
                })?;

                let s = &mut *s
                    .write_lock::<ImmutableString>()
                    .expect("`ImmutableString`");

                let index = *index;

                *s = s
                    .chars()
                    .enumerate()
                    .map(|(i, ch)| if i == index { new_ch } else { ch })
                    .collect();
            }
        }

        Ok(())
    }
}

impl<'a> From<&'a mut Dynamic> for Target<'a> {
    #[inline]
    fn from(value: &'a mut Dynamic) -> Self {
        #[cfg(not(feature = "no_closure"))]
        if value.is_shared() {
            // Cloning is cheap for a shared value
            let container = value.clone();
            return Self::LockGuard((value.write_lock::<Dynamic>().expect("`Dynamic`"), container));
        }

        Self::RefMut(value)
    }
}

impl Deref for Target<'_> {
    type Target = Dynamic;

    #[inline]
    fn deref(&self) -> &Dynamic {
        match self {
            Self::RefMut(r) => *r,
            #[cfg(not(feature = "no_closure"))]
            Self::LockGuard((r, _)) => &**r,
            Self::TempValue(ref r) => r,
            #[cfg(not(feature = "no_index"))]
            Self::Bit(_, ref r, _)
            | Self::BitField(_, _, ref r, _)
            | Self::BlobByte(_, _, ref r)
            | Self::StringChar(_, _, ref r) => r,
        }
    }
}

impl AsRef<Dynamic> for Target<'_> {
    #[inline(always)]
    fn as_ref(&self) -> &Dynamic {
        self
    }
}

impl DerefMut for Target<'_> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Dynamic {
        match self {
            Self::RefMut(r) => *r,
            #[cfg(not(feature = "no_closure"))]
            Self::LockGuard((r, _)) => r.deref_mut(),
            Self::TempValue(ref mut r) => r,
            #[cfg(not(feature = "no_index"))]
            Self::Bit(_, ref mut r, _)
            | Self::BitField(_, _, ref mut r, _)
            | Self::BlobByte(_, _, ref mut r)
            | Self::StringChar(_, _, ref mut r) => r,
        }
    }
}

impl AsMut<Dynamic> for Target<'_> {
    #[inline(always)]
    fn as_mut(&mut self) -> &mut Dynamic {
        self
    }
}

impl<T: Into<Dynamic>> From<T> for Target<'_> {
    #[inline(always)]
    #[must_use]
    fn from(value: T) -> Self {
        Self::TempValue(value.into())
    }
}

/// _(internals)_ An entry in a function resolution cache.
/// Exported under the `internals` feature only.
#[derive(Debug, Clone)]
pub struct FnResolutionCacheEntry {
    /// Function.
    pub func: CallableFunction,
    /// Optional source.
    pub source: Option<Identifier>,
}

/// _(internals)_ A function resolution cache.
/// Exported under the `internals` feature only.
pub type FnResolutionCache = BTreeMap<u64, Option<Box<FnResolutionCacheEntry>>>;

/// _(internals)_ A type that holds all the current states of the [`Engine`].
/// Exported under the `internals` feature only.
#[derive(Debug, Clone)]
pub struct EvalState {
    /// Normally, access to variables are parsed with a relative offset into the [`Scope`] to avoid a lookup.
    /// In some situation, e.g. after running an `eval` statement, or after a custom syntax statement,
    /// subsequent offsets may become mis-aligned.
    /// When that happens, this flag is turned on to force a [`Scope`] search by name.
    pub always_search_scope: bool,
    /// Level of the current scope.  The global (root) level is zero, a new block (or function call)
    /// is one level higher, and so on.
    pub scope_level: usize,
    /// Stack of function resolution caches.
    fn_resolution_caches: StaticVec<FnResolutionCache>,
}

impl EvalState {
    /// Create a new [`EvalState`].
    #[inline(always)]
    #[must_use]
    pub const fn new() -> Self {
        Self {
            always_search_scope: false,
            scope_level: 0,
            fn_resolution_caches: StaticVec::new_const(),
        }
    }
    /// Is the state currently at global (root) level?
    #[inline(always)]
    #[must_use]
    pub const fn is_global(&self) -> bool {
        self.scope_level == 0
    }
    /// Get the number of function resolution cache(s) in the stack.
    #[inline(always)]
    #[must_use]
    pub fn fn_resolution_caches_len(&self) -> usize {
        self.fn_resolution_caches.len()
    }
    /// Get a mutable reference to the current function resolution cache.
    #[inline]
    #[must_use]
    pub fn fn_resolution_cache_mut(&mut self) -> &mut FnResolutionCache {
        if self.fn_resolution_caches.is_empty() {
            // Push a new function resolution cache if the stack is empty
            self.push_fn_resolution_cache();
        }
        self.fn_resolution_caches.last_mut().expect("not empty")
    }
    /// Push an empty function resolution cache onto the stack and make it current.
    #[allow(dead_code)]
    #[inline(always)]
    pub fn push_fn_resolution_cache(&mut self) {
        self.fn_resolution_caches.push(BTreeMap::new());
    }
    /// Rewind the function resolution caches stack to a particular size.
    #[inline(always)]
    pub fn rewind_fn_resolution_caches(&mut self, len: usize) {
        self.fn_resolution_caches.truncate(len);
    }
}

/// Context of a script evaluation process.
#[derive(Debug)]
pub struct EvalContext<'a, 'x, 'px, 'm, 's, 'b, 't, 'pt> {
    pub(crate) engine: &'a Engine,
    pub(crate) scope: &'x mut Scope<'px>,
    pub(crate) mods: &'m mut Imports,
    pub(crate) state: &'s mut EvalState,
    pub(crate) lib: &'b [&'b Module],
    pub(crate) this_ptr: &'t mut Option<&'pt mut Dynamic>,
    pub(crate) level: usize,
}

impl<'x, 'px, 'pt> EvalContext<'_, 'x, 'px, '_, '_, '_, '_, 'pt> {
    /// The current [`Engine`].
    #[inline(always)]
    #[must_use]
    pub const fn engine(&self) -> &Engine {
        self.engine
    }
    /// The current source.
    #[inline(always)]
    #[must_use]
    pub fn source(&self) -> Option<&str> {
        self.mods.source.as_ref().map(|s| s.as_str())
    }
    /// The current [`Scope`].
    #[inline(always)]
    #[must_use]
    pub const fn scope(&self) -> &Scope<'px> {
        self.scope
    }
    /// Mutable reference to the current [`Scope`].
    #[inline(always)]
    #[must_use]
    pub fn scope_mut(&mut self) -> &mut &'x mut Scope<'px> {
        &mut self.scope
    }
    /// Get an iterator over the current set of modules imported via `import` statements.
    #[cfg(not(feature = "no_module"))]
    #[inline(always)]
    pub fn iter_imports(&self) -> impl Iterator<Item = (&str, &Module)> {
        self.mods.iter()
    }
    /// _(internals)_ The current set of modules imported via `import` statements.
    /// Exported under the `internals` feature only.
    #[cfg(feature = "internals")]
    #[cfg(not(feature = "no_module"))]
    #[inline(always)]
    #[must_use]
    pub const fn imports(&self) -> &Imports {
        self.mods
    }
    /// Get an iterator over the namespaces containing definition of all script-defined functions.
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
    /// The current bound `this` pointer, if any.
    #[inline(always)]
    #[must_use]
    pub fn this_ptr(&self) -> Option<&Dynamic> {
        self.this_ptr.as_ref().map(|v| &**v)
    }
    /// Mutable reference to the current bound `this` pointer, if any.
    #[inline(always)]
    #[must_use]
    pub fn this_ptr_mut(&mut self) -> Option<&mut &'pt mut Dynamic> {
        self.this_ptr.as_mut()
    }
    /// The current nesting level of function calls.
    #[inline(always)]
    #[must_use]
    pub const fn call_level(&self) -> usize {
        self.level
    }
}

/// Rhai main scripting engine.
///
/// # Thread Safety
///
/// [`Engine`] is re-entrant.
///
/// Currently, [`Engine`] is neither [`Send`] nor [`Sync`].
/// Use the `sync` feature to make it [`Send`] `+` [`Sync`].
///
/// # Example
///
/// ```
/// # fn main() -> Result<(), Box<rhai::EvalAltResult>> {
/// use quad_compat_rhai::Engine;
///
/// let engine = Engine::new();
///
/// let result = engine.eval::<i64>("40 + 2")?;
///
/// println!("Answer: {}", result);  // prints 42
/// # Ok(())
/// # }
/// ```
pub struct Engine {
    /// A collection of all modules loaded into the global namespace of the Engine.
    pub(crate) global_modules: StaticVec<Shared<Module>>,
    /// A collection of all sub-modules directly loaded into the Engine.
    pub(crate) global_sub_modules: BTreeMap<Identifier, Shared<Module>>,

    /// A module resolution service.
    #[cfg(not(feature = "no_module"))]
    pub(crate) module_resolver: Option<Box<dyn crate::ModuleResolver>>,

    /// A map mapping type names to pretty-print names.
    pub(crate) type_names: BTreeMap<Identifier, Identifier>,

    /// An empty [`ImmutableString`] for cloning purposes.
    pub(crate) empty_string: ImmutableString,

    /// A set of symbols to disable.
    pub(crate) disabled_symbols: BTreeSet<Identifier>,
    /// A map containing custom keywords and precedence to recognize.
    pub(crate) custom_keywords: BTreeMap<Identifier, Option<Precedence>>,
    /// Custom syntax.
    pub(crate) custom_syntax: BTreeMap<Identifier, Box<CustomSyntax>>,
    /// Callback closure for resolving variable access.
    pub(crate) resolve_var: Option<OnVarCallback>,
    /// Callback closure to remap tokens during parsing.
    pub(crate) token_mapper: Option<Box<OnParseTokenCallback>>,

    /// Callback closure for implementing the `print` command.
    pub(crate) print: Option<OnPrintCallback>,
    /// Callback closure for implementing the `debug` command.
    pub(crate) debug: Option<OnDebugCallback>,
    /// Callback closure for progress reporting.
    #[cfg(not(feature = "unchecked"))]
    pub(crate) progress: Option<crate::func::native::OnProgressCallback>,

    /// Optimize the [`AST`][crate::AST] after compilation.
    #[cfg(not(feature = "no_optimize"))]
    pub(crate) optimization_level: crate::OptimizationLevel,

    /// Language options.
    pub(crate) options: crate::api::options::LanguageOptions,

    /// Max limits.
    #[cfg(not(feature = "unchecked"))]
    pub(crate) limits: crate::api::limits::Limits,
}

impl fmt::Debug for Engine {
    #[inline(always)]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Engine")
    }
}

impl Default for Engine {
    #[inline(always)]
    fn default() -> Self {
        Self::new()
    }
}

/// Make getter function
#[cfg(not(feature = "no_object"))]
#[inline]
#[must_use]
pub fn make_getter(id: impl AsRef<str>) -> String {
    format!("{}{}", FN_GET, id.as_ref())
}

/// Make setter function
#[cfg(not(feature = "no_object"))]
#[inline]
#[must_use]
pub fn make_setter(id: impl AsRef<str>) -> String {
    format!("{}{}", FN_SET, id.as_ref())
}

/// Is this function an anonymous function?
#[cfg(not(feature = "no_function"))]
#[inline(always)]
#[must_use]
pub fn is_anonymous_fn(fn_name: impl AsRef<str>) -> bool {
    fn_name.as_ref().starts_with(FN_ANONYMOUS)
}

/// Print to `stdout`
#[inline]
#[allow(unused_variables)]
fn print_to_stdout(s: &str) {
    #[cfg(not(feature = "no_std"))]
    #[cfg(not(any(target_arch = "wasm32", target_arch = "wasm64")))]
    println!("{}", s);
}

/// Debug to `stdout`
#[inline]
#[allow(unused_variables)]
fn debug_to_stdout(s: &str, source: Option<&str>, pos: Position) {
    #[cfg(not(feature = "no_std"))]
    #[cfg(not(any(target_arch = "wasm32", target_arch = "wasm64")))]
    if let Some(source) = source {
        println!("{}{:?} | {}", source, pos, s);
    } else if pos.is_none() {
        println!("{}", s);
    } else {
        println!("{:?} | {}", pos, s);
    }
}

impl Engine {
    /// Create a new [`Engine`].
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        // Create the new scripting Engine
        let mut engine = Self::new_raw();

        #[cfg(not(feature = "no_module"))]
        #[cfg(not(feature = "no_std"))]
        #[cfg(not(any(target_arch = "wasm32", target_arch = "wasm64")))]
        {
            engine.module_resolver =
                Some(Box::new(crate::module::resolvers::FileModuleResolver::new()));
        }

        // default print/debug implementations
        engine.print = Some(Box::new(print_to_stdout));
        engine.debug = Some(Box::new(debug_to_stdout));

        engine.register_global_module(StandardPackage::new().as_shared_module());

        engine
    }

    /// Create a new [`Engine`] with minimal built-in functions.
    ///
    /// Use [`register_global_module`][Engine::register_global_module] to add packages of functions.
    #[inline]
    #[must_use]
    pub fn new_raw() -> Self {
        let mut engine = Self {
            global_modules: StaticVec::new_const(),
            global_sub_modules: BTreeMap::new(),

            #[cfg(not(feature = "no_module"))]
            module_resolver: None,

            type_names: BTreeMap::new(),
            empty_string: ImmutableString::new(),
            disabled_symbols: BTreeSet::new(),
            custom_keywords: BTreeMap::new(),
            custom_syntax: BTreeMap::new(),

            resolve_var: None,
            token_mapper: None,

            print: None,
            debug: None,

            #[cfg(not(feature = "unchecked"))]
            progress: None,

            #[cfg(not(feature = "no_optimize"))]
            optimization_level: crate::OptimizationLevel::default(),

            options: crate::api::options::LanguageOptions::new(),

            #[cfg(not(feature = "unchecked"))]
            limits: crate::api::limits::Limits::new(),
        };

        // Add the global namespace module
        let mut global_namespace = Module::new();
        global_namespace.internal = true;
        engine.global_modules.push(global_namespace.into());

        engine
    }

    /// Get an empty [`ImmutableString`].
    ///
    /// [`Engine`] keeps a single instance of an empty [`ImmutableString`] and uses this to create
    /// shared instances for subsequent uses. This minimizes unnecessary allocations for empty strings.
    #[inline(always)]
    #[must_use]
    pub fn const_empty_string(&self) -> ImmutableString {
        self.empty_string.clone()
    }

    /// Search for a module within an imports stack.
    #[inline]
    #[must_use]
    pub(crate) fn search_imports(
        &self,
        mods: &Imports,
        state: &mut EvalState,
        namespace: &NamespaceRef,
    ) -> Option<Shared<Module>> {
        let root = &namespace[0].name;

        // Qualified - check if the root module is directly indexed
        let index = if state.always_search_scope {
            None
        } else {
            namespace.index()
        };

        if let Some(index) = index {
            let offset = mods.len() - index.get();
            Some(mods.get(offset).expect("within range"))
        } else {
            mods.find(root)
                .map(|n| mods.get(n).expect("valid index"))
                .or_else(|| self.global_sub_modules.get(root).cloned())
        }
    }

    /// Search for a variable within the scope or within imports,
    /// depending on whether the variable name is namespace-qualified.
    pub(crate) fn search_namespace<'s>(
        &self,
        scope: &'s mut Scope,
        mods: &mut Imports,
        state: &mut EvalState,
        lib: &[&Module],
        this_ptr: &'s mut Option<&mut Dynamic>,
        expr: &Expr,
    ) -> Result<(Target<'s>, Position), Box<EvalAltResult>> {
        match expr {
            Expr::Variable(Some(_), _, _) => {
                self.search_scope_only(scope, mods, state, lib, this_ptr, expr)
            }
            Expr::Variable(None, _var_pos, v) => match v.as_ref() {
                // Normal variable access
                (_, None, _) => self.search_scope_only(scope, mods, state, lib, this_ptr, expr),

                // Qualified variable access
                #[cfg(not(feature = "no_module"))]
                (_, Some((namespace, hash_var)), var_name) => {
                    if let Some(module) = self.search_imports(mods, state, namespace) {
                        // foo:bar::baz::VARIABLE
                        return match module.get_qualified_var(*hash_var) {
                            Ok(target) => {
                                let mut target = target.clone();
                                // Module variables are constant
                                target.set_access_mode(AccessMode::ReadOnly);
                                Ok((target.into(), *_var_pos))
                            }
                            Err(err) => Err(match *err {
                                EvalAltResult::ErrorVariableNotFound(_, _) => {
                                    EvalAltResult::ErrorVariableNotFound(
                                        format!(
                                            "{}{}{}",
                                            namespace,
                                            Token::DoubleColon.literal_syntax(),
                                            var_name
                                        ),
                                        namespace[0].pos,
                                    )
                                    .into()
                                }
                                _ => err.fill_position(*_var_pos),
                            }),
                        };
                    }

                    #[cfg(not(feature = "no_function"))]
                    if namespace.len() == 1 && namespace[0].name == KEYWORD_GLOBAL {
                        // global::VARIABLE
                        let global_constants = mods.global_constants_mut();

                        if let Some(mut guard) = global_constants {
                            if let Some(value) = guard.get_mut(var_name) {
                                let mut target: Target = value.clone().into();
                                // Module variables are constant
                                target.set_access_mode(AccessMode::ReadOnly);
                                return Ok((target.into(), *_var_pos));
                            }
                        }

                        return Err(EvalAltResult::ErrorVariableNotFound(
                            format!(
                                "{}{}{}",
                                namespace,
                                Token::DoubleColon.literal_syntax(),
                                var_name
                            ),
                            namespace[0].pos,
                        )
                        .into());
                    }

                    Err(
                        EvalAltResult::ErrorModuleNotFound(namespace.to_string(), namespace[0].pos)
                            .into(),
                    )
                }

                #[cfg(feature = "no_module")]
                (_, Some((_, _)), _) => unreachable!("qualified access under no_module"),
            },
            _ => unreachable!("Expr::Variable expected, but gets {:?}", expr),
        }
    }

    /// Search for a variable within the scope
    ///
    /// # Panics
    ///
    /// Panics if `expr` is not [`Expr::Variable`].
    pub(crate) fn search_scope_only<'s>(
        &self,
        scope: &'s mut Scope,
        mods: &mut Imports,
        state: &mut EvalState,
        lib: &[&Module],
        this_ptr: &'s mut Option<&mut Dynamic>,
        expr: &Expr,
    ) -> Result<(Target<'s>, Position), Box<EvalAltResult>> {
        // Make sure that the pointer indirection is taken only when absolutely necessary.

        let (index, var_pos) = match expr {
            // Check if the variable is `this`
            Expr::Variable(None, pos, v) if v.0.is_none() && v.2 == KEYWORD_THIS => {
                return if let Some(val) = this_ptr {
                    Ok(((*val).into(), *pos))
                } else {
                    Err(EvalAltResult::ErrorUnboundThis(*pos).into())
                }
            }
            _ if state.always_search_scope => (0, expr.position()),
            Expr::Variable(Some(i), pos, _) => (i.get() as usize, *pos),
            Expr::Variable(None, pos, v) => (v.0.map(NonZeroUsize::get).unwrap_or(0), *pos),
            _ => unreachable!("Expr::Variable expected, but gets {:?}", expr),
        };

        // Check the variable resolver, if any
        if let Some(ref resolve_var) = self.resolve_var {
            let context = EvalContext {
                engine: self,
                scope,
                mods,
                state,
                lib,
                this_ptr,
                level: 0,
            };
            match resolve_var(
                expr.get_variable_name(true).expect("`Variable`"),
                index,
                &context,
            ) {
                Ok(Some(mut result)) => {
                    result.set_access_mode(AccessMode::ReadOnly);
                    return Ok((result.into(), var_pos));
                }
                Ok(None) => (),
                Err(err) => return Err(err.fill_position(var_pos)),
            }
        }

        let index = if index > 0 {
            scope.len() - index
        } else {
            // Find the variable in the scope
            let var_name = expr.get_variable_name(true).expect("`Variable`");
            scope
                .get_index(var_name)
                .ok_or_else(|| EvalAltResult::ErrorVariableNotFound(var_name.to_string(), var_pos))?
                .0
        };

        let val = scope.get_mut_by_index(index);

        Ok((val.into(), var_pos))
    }

    /// Chain-evaluate a dot/index chain.
    /// [`Position`] in [`EvalAltResult`] is [`NONE`][Position::NONE] and must be set afterwards.
    #[cfg(any(not(feature = "no_index"), not(feature = "no_object")))]
    fn eval_dot_index_chain_helper(
        &self,
        mods: &mut Imports,
        state: &mut EvalState,
        lib: &[&Module],
        this_ptr: &mut Option<&mut Dynamic>,
        target: &mut Target,
        root: (&str, Position),
        rhs: &Expr,
        terminate_chaining: bool,
        idx_values: &mut StaticVec<ChainArgument>,
        chain_type: ChainType,
        level: usize,
        new_val: Option<((Dynamic, Position), (Option<OpAssignment>, Position))>,
    ) -> Result<(Dynamic, bool), Box<EvalAltResult>> {
        let is_ref_mut = target.is_ref();
        let _terminate_chaining = terminate_chaining;

        // Pop the last index value
        let idx_val = idx_values.pop().expect("not empty");

        match chain_type {
            #[cfg(not(feature = "no_index"))]
            ChainType::Indexing => {
                let pos = rhs.position();
                let root_pos = idx_val.position();
                let idx_val = idx_val.into_index_value().expect("`ChainType::Index`");

                match rhs {
                    // xxx[idx].expr... | xxx[idx][expr]...
                    Expr::Dot(x, term, x_pos) | Expr::Index(x, term, x_pos)
                        if !_terminate_chaining =>
                    {
                        let mut idx_val_for_setter = idx_val.clone();
                        let idx_pos = x.lhs.position();
                        let rhs_chain = match_chaining_type(rhs);

                        let (try_setter, result) = {
                            let mut obj = self.get_indexed_mut(
                                mods, state, lib, target, idx_val, idx_pos, false, true, level,
                            )?;
                            let is_obj_temp_val = obj.is_temp_value();
                            let obj_ptr = &mut obj;

                            match self.eval_dot_index_chain_helper(
                                mods, state, lib, this_ptr, obj_ptr, root, &x.rhs, *term,
                                idx_values, rhs_chain, level, new_val,
                            ) {
                                Ok((result, true)) if is_obj_temp_val => {
                                    (Some(obj.take_or_clone()), (result, true))
                                }
                                Ok(result) => (None, result),
                                Err(err) => return Err(err.fill_position(*x_pos)),
                            }
                        };

                        if let Some(mut new_val) = try_setter {
                            // Try to call index setter if value is changed
                            let hash_set =
                                crate::ast::FnCallHashes::from_native(mods.hash_idx_set());
                            let args = &mut [target, &mut idx_val_for_setter, &mut new_val];

                            if let Err(err) = self.exec_fn_call(
                                mods, state, lib, FN_IDX_SET, hash_set, args, is_ref_mut, true,
                                root_pos, None, level,
                            ) {
                                // Just ignore if there is no index setter
                                if !matches!(*err, EvalAltResult::ErrorFunctionNotFound(_, _)) {
                                    return Err(err);
                                }
                            }
                        }

                        self.check_data_size(target.as_ref())
                            .map_err(|err| err.fill_position(root.1))?;

                        Ok(result)
                    }
                    // xxx[rhs] op= new_val
                    _ if new_val.is_some() => {
                        let ((new_val, new_pos), (op_info, op_pos)) = new_val.expect("`Some`");
                        let mut idx_val_for_setter = idx_val.clone();

                        let try_setter = match self.get_indexed_mut(
                            mods, state, lib, target, idx_val, pos, true, false, level,
                        ) {
                            // Indexed value is a reference - update directly
                            Ok(ref mut obj_ptr) => {
                                self.eval_op_assignment(
                                    mods, state, lib, op_info, op_pos, obj_ptr, root, new_val,
                                )
                                .map_err(|err| err.fill_position(new_pos))?;
                                None
                            }
                            // Can't index - try to call an index setter
                            #[cfg(not(feature = "no_index"))]
                            Err(err) if matches!(*err, EvalAltResult::ErrorIndexingType(_, _)) => {
                                Some(new_val)
                            }
                            // Any other error
                            Err(err) => return Err(err),
                        };

                        if let Some(mut new_val) = try_setter {
                            // Try to call index setter
                            let hash_set =
                                crate::ast::FnCallHashes::from_native(mods.hash_idx_set());
                            let args = &mut [target, &mut idx_val_for_setter, &mut new_val];

                            self.exec_fn_call(
                                mods, state, lib, FN_IDX_SET, hash_set, args, is_ref_mut, true,
                                root_pos, None, level,
                            )?;
                        }

                        self.check_data_size(target.as_ref())
                            .map_err(|err| err.fill_position(root.1))?;

                        Ok((Dynamic::UNIT, true))
                    }
                    // xxx[rhs]
                    _ => self
                        .get_indexed_mut(mods, state, lib, target, idx_val, pos, false, true, level)
                        .map(|v| (v.take_or_clone(), false)),
                }
            }

            #[cfg(not(feature = "no_object"))]
            ChainType::Dotting => {
                match rhs {
                    // xxx.fn_name(arg_expr_list)
                    Expr::FnCall(x, pos) if !x.is_qualified() && new_val.is_none() => {
                        let FnCallExpr { name, hashes, .. } = x.as_ref();
                        let call_args = &mut idx_val
                            .into_fn_call_args()
                            .expect("`ChainType::Dot` with `Expr::FnCallExpr`");
                        self.make_method_call(
                            mods, state, lib, name, *hashes, target, call_args, *pos, level,
                        )
                    }
                    // xxx.fn_name(...) = ???
                    Expr::FnCall(_, _) if new_val.is_some() => {
                        unreachable!("method call cannot be assigned to")
                    }
                    // xxx.module::fn_name(...) - syntax error
                    Expr::FnCall(_, _) => {
                        unreachable!("function call in dot chain should not be namespace-qualified")
                    }
                    // {xxx:map}.id op= ???
                    Expr::Property(x) if target.is::<crate::Map>() && new_val.is_some() => {
                        let (name, pos) = &x.2;
                        let ((new_val, new_pos), (op_info, op_pos)) = new_val.expect("`Some`");
                        let index = name.into();
                        {
                            let val_target = &mut self.get_indexed_mut(
                                mods, state, lib, target, index, *pos, true, false, level,
                            )?;
                            self.eval_op_assignment(
                                mods, state, lib, op_info, op_pos, val_target, root, new_val,
                            )
                            .map_err(|err| err.fill_position(new_pos))?;
                        }
                        self.check_data_size(target.as_ref())
                            .map_err(|err| err.fill_position(root.1))?;
                        Ok((Dynamic::UNIT, true))
                    }
                    // {xxx:map}.id
                    Expr::Property(x) if target.is::<crate::Map>() => {
                        let (name, pos) = &x.2;
                        let index = name.into();
                        let val = self.get_indexed_mut(
                            mods, state, lib, target, index, *pos, false, false, level,
                        )?;
                        Ok((val.take_or_clone(), false))
                    }
                    // xxx.id op= ???
                    Expr::Property(x) if new_val.is_some() => {
                        let ((getter, hash_get), (setter, hash_set), (name, pos)) = x.as_ref();
                        let ((mut new_val, new_pos), (op_info, op_pos)) = new_val.expect("`Some`");

                        if op_info.is_some() {
                            let hash = crate::ast::FnCallHashes::from_native(*hash_get);
                            let args = &mut [target.as_mut()];
                            let (mut orig_val, _) = self
                                .exec_fn_call(
                                    mods, state, lib, getter, hash, args, is_ref_mut, true, *pos,
                                    None, level,
                                )
                                .or_else(|err| match *err {
                                    // Try an indexer if property does not exist
                                    EvalAltResult::ErrorDotExpr(_, _) => {
                                        let prop = name.into();
                                        self.get_indexed_mut(
                                            mods, state, lib, target, prop, *pos, false, true,
                                            level,
                                        )
                                        .map(|v| (v.take_or_clone(), false))
                                        .map_err(
                                            |idx_err| match *idx_err {
                                                EvalAltResult::ErrorIndexingType(_, _) => err,
                                                _ => idx_err,
                                            },
                                        )
                                    }
                                    _ => Err(err),
                                })?;

                            self.eval_op_assignment(
                                mods,
                                state,
                                lib,
                                op_info,
                                op_pos,
                                &mut (&mut orig_val).into(),
                                root,
                                new_val,
                            )
                            .map_err(|err| err.fill_position(new_pos))?;

                            self.check_data_size(target.as_ref())
                                .map_err(|err| err.fill_position(root.1))?;

                            new_val = orig_val;
                        }

                        let hash = crate::ast::FnCallHashes::from_native(*hash_set);
                        let args = &mut [target.as_mut(), &mut new_val];
                        self.exec_fn_call(
                            mods, state, lib, setter, hash, args, is_ref_mut, true, *pos, None,
                            level,
                        )
                        .or_else(|err| match *err {
                            // Try an indexer if property does not exist
                            EvalAltResult::ErrorDotExpr(_, _) => {
                                let args = &mut [target, &mut name.into(), &mut new_val];
                                let hash_set =
                                    crate::ast::FnCallHashes::from_native(mods.hash_idx_set());
                                let pos = Position::NONE;

                                self.exec_fn_call(
                                    mods, state, lib, FN_IDX_SET, hash_set, args, is_ref_mut, true,
                                    pos, None, level,
                                )
                                .map_err(
                                    |idx_err| match *idx_err {
                                        EvalAltResult::ErrorIndexingType(_, _) => err,
                                        _ => idx_err,
                                    },
                                )
                            }
                            _ => Err(err),
                        })
                    }
                    // xxx.id
                    Expr::Property(x) => {
                        let ((getter, hash_get), _, (name, pos)) = x.as_ref();
                        let hash = crate::ast::FnCallHashes::from_native(*hash_get);
                        let args = &mut [target.as_mut()];
                        self.exec_fn_call(
                            mods, state, lib, getter, hash, args, is_ref_mut, true, *pos, None,
                            level,
                        )
                        .map_or_else(
                            |err| match *err {
                                // Try an indexer if property does not exist
                                EvalAltResult::ErrorDotExpr(_, _) => {
                                    let prop = name.into();
                                    self.get_indexed_mut(
                                        mods, state, lib, target, prop, *pos, false, true, level,
                                    )
                                    .map(|v| (v.take_or_clone(), false))
                                    .map_err(|idx_err| {
                                        match *idx_err {
                                            EvalAltResult::ErrorIndexingType(_, _) => err,
                                            _ => idx_err,
                                        }
                                    })
                                }
                                _ => Err(err),
                            },
                            // Assume getters are always pure
                            |(v, _)| Ok((v, false)),
                        )
                    }
                    // {xxx:map}.sub_lhs[expr] | {xxx:map}.sub_lhs.expr
                    Expr::Index(x, term, x_pos) | Expr::Dot(x, term, x_pos)
                        if target.is::<crate::Map>() =>
                    {
                        let val_target = &mut match x.lhs {
                            Expr::Property(ref p) => {
                                let (name, pos) = &p.2;
                                let index = name.into();
                                self.get_indexed_mut(
                                    mods, state, lib, target, index, *pos, false, true, level,
                                )?
                            }
                            // {xxx:map}.fn_name(arg_expr_list)[expr] | {xxx:map}.fn_name(arg_expr_list).expr
                            Expr::FnCall(ref x, pos) if !x.is_qualified() => {
                                let FnCallExpr { name, hashes, .. } = x.as_ref();
                                let call_args = &mut idx_val.into_fn_call_args().expect(
                                    "`chain_type` is `ChainType::Dot` with `Expr::FnCallExpr`",
                                );
                                let (val, _) = self.make_method_call(
                                    mods, state, lib, name, *hashes, target, call_args, pos, level,
                                )?;
                                val.into()
                            }
                            // {xxx:map}.module::fn_name(...) - syntax error
                            Expr::FnCall(_, _) => unreachable!(
                                "function call in dot chain should not be namespace-qualified"
                            ),
                            // Others - syntax error
                            ref expr => unreachable!("invalid dot expression: {:?}", expr),
                        };
                        let rhs_chain = match_chaining_type(rhs);

                        self.eval_dot_index_chain_helper(
                            mods, state, lib, this_ptr, val_target, root, &x.rhs, *term,
                            idx_values, rhs_chain, level, new_val,
                        )
                        .map_err(|err| err.fill_position(*x_pos))
                    }
                    // xxx.sub_lhs[expr] | xxx.sub_lhs.expr
                    Expr::Index(x, term, x_pos) | Expr::Dot(x, term, x_pos) => {
                        match x.lhs {
                            // xxx.prop[expr] | xxx.prop.expr
                            Expr::Property(ref p) => {
                                let ((getter, hash_get), (setter, hash_set), (name, pos)) =
                                    p.as_ref();
                                let rhs_chain = match_chaining_type(rhs);
                                let hash_get = crate::ast::FnCallHashes::from_native(*hash_get);
                                let hash_set = crate::ast::FnCallHashes::from_native(*hash_set);
                                let mut arg_values = [target.as_mut(), &mut Dynamic::UNIT.clone()];
                                let args = &mut arg_values[..1];

                                // Assume getters are always pure
                                let (mut val, _) = self
                                    .exec_fn_call(
                                        mods, state, lib, getter, hash_get, args, is_ref_mut, true,
                                        *pos, None, level,
                                    )
                                    .or_else(|err| match *err {
                                        // Try an indexer if property does not exist
                                        EvalAltResult::ErrorDotExpr(_, _) => {
                                            let prop = name.into();
                                            self.get_indexed_mut(
                                                mods, state, lib, target, prop, *pos, false, true,
                                                level,
                                            )
                                            .map(|v| (v.take_or_clone(), false))
                                            .map_err(
                                                |idx_err| match *idx_err {
                                                    EvalAltResult::ErrorIndexingType(_, _) => err,
                                                    _ => idx_err,
                                                },
                                            )
                                        }
                                        _ => Err(err),
                                    })?;

                                let val = &mut val;

                                let (result, may_be_changed) = self
                                    .eval_dot_index_chain_helper(
                                        mods,
                                        state,
                                        lib,
                                        this_ptr,
                                        &mut val.into(),
                                        root,
                                        &x.rhs,
                                        *term,
                                        idx_values,
                                        rhs_chain,
                                        level,
                                        new_val,
                                    )
                                    .map_err(|err| err.fill_position(*x_pos))?;

                                // Feed the value back via a setter just in case it has been updated
                                if may_be_changed {
                                    // Re-use args because the first &mut parameter will not be consumed
                                    let mut arg_values = [target.as_mut(), val];
                                    let args = &mut arg_values;
                                    self.exec_fn_call(
                                        mods, state, lib, setter, hash_set, args, is_ref_mut, true,
                                        *pos, None, level,
                                    )
                                    .or_else(
                                        |err| match *err {
                                            // Try an indexer if property does not exist
                                            EvalAltResult::ErrorDotExpr(_, _) => {
                                                let args =
                                                    &mut [target.as_mut(), &mut name.into(), val];
                                                let hash_set =
                                                    crate::ast::FnCallHashes::from_native(
                                                        mods.hash_idx_set(),
                                                    );
                                                self.exec_fn_call(
                                                    mods, state, lib, FN_IDX_SET, hash_set, args,
                                                    is_ref_mut, true, *pos, None, level,
                                                )
                                                .or_else(|idx_err| match *idx_err {
                                                    EvalAltResult::ErrorIndexingType(_, _) => {
                                                        // If there is no setter, no need to feed it back because
                                                        // the property is read-only
                                                        Ok((Dynamic::UNIT, false))
                                                    }
                                                    _ => Err(idx_err),
                                                })
                                            }
                                            _ => Err(err),
                                        },
                                    )?;
                                    self.check_data_size(target.as_ref())
                                        .map_err(|err| err.fill_position(root.1))?;
                                }

                                Ok((result, may_be_changed))
                            }
                            // xxx.fn_name(arg_expr_list)[expr] | xxx.fn_name(arg_expr_list).expr
                            Expr::FnCall(ref f, pos) if !f.is_qualified() => {
                                let FnCallExpr { name, hashes, .. } = f.as_ref();
                                let rhs_chain = match_chaining_type(rhs);
                                let args = &mut idx_val.into_fn_call_args().expect(
                                    "`chain_type` is `ChainType::Dot` with `Expr::FnCallExpr`",
                                );
                                let (mut val, _) = self.make_method_call(
                                    mods, state, lib, name, *hashes, target, args, pos, level,
                                )?;
                                let val = &mut val;
                                let target = &mut val.into();

                                self.eval_dot_index_chain_helper(
                                    mods, state, lib, this_ptr, target, root, &x.rhs, *term,
                                    idx_values, rhs_chain, level, new_val,
                                )
                                .map_err(|err| err.fill_position(pos))
                            }
                            // xxx.module::fn_name(...) - syntax error
                            Expr::FnCall(_, _) => unreachable!(
                                "function call in dot chain should not be namespace-qualified"
                            ),
                            // Others - syntax error
                            ref expr => unreachable!("invalid dot expression: {:?}", expr),
                        }
                    }
                    // Syntax error
                    _ => Err(EvalAltResult::ErrorDotExpr("".into(), rhs.position()).into()),
                }
            }
        }
    }

    /// Evaluate a dot/index chain.
    #[cfg(any(not(feature = "no_index"), not(feature = "no_object")))]
    fn eval_dot_index_chain(
        &self,
        scope: &mut Scope,
        mods: &mut Imports,
        state: &mut EvalState,
        lib: &[&Module],
        this_ptr: &mut Option<&mut Dynamic>,
        expr: &Expr,
        level: usize,
        new_val: Option<((Dynamic, Position), (Option<OpAssignment>, Position))>,
    ) -> RhaiResult {
        let (crate::ast::BinaryExpr { lhs, rhs }, chain_type, term, op_pos) = match expr {
            #[cfg(not(feature = "no_index"))]
            Expr::Index(x, term, pos) => (x.as_ref(), ChainType::Indexing, *term, *pos),
            #[cfg(not(feature = "no_object"))]
            Expr::Dot(x, term, pos) => (x.as_ref(), ChainType::Dotting, *term, *pos),
            _ => unreachable!("index or dot chain expected, but gets {:?}", expr),
        };

        let idx_values = &mut StaticVec::new_const();

        self.eval_dot_index_chain_arguments(
            scope, mods, state, lib, this_ptr, rhs, term, chain_type, idx_values, 0, level,
        )?;

        match lhs {
            // id.??? or id[???]
            Expr::Variable(_, var_pos, x) => {
                #[cfg(not(feature = "unchecked"))]
                self.inc_operations(&mut mods.num_operations, *var_pos)?;

                let (mut target, _) =
                    self.search_namespace(scope, mods, state, lib, this_ptr, lhs)?;

                let obj_ptr = &mut target;
                let root = (x.2.as_str(), *var_pos);

                self.eval_dot_index_chain_helper(
                    mods, state, lib, &mut None, obj_ptr, root, rhs, term, idx_values, chain_type,
                    level, new_val,
                )
                .map(|(v, _)| v)
                .map_err(|err| err.fill_position(op_pos))
            }
            // {expr}.??? = ??? or {expr}[???] = ???
            _ if new_val.is_some() => unreachable!("cannot assign to an expression"),
            // {expr}.??? or {expr}[???]
            expr => {
                let value = self.eval_expr(scope, mods, state, lib, this_ptr, expr, level)?;
                let obj_ptr = &mut value.into();
                let root = ("", expr.position());
                self.eval_dot_index_chain_helper(
                    mods, state, lib, this_ptr, obj_ptr, root, rhs, term, idx_values, chain_type,
                    level, new_val,
                )
                .map(|(v, _)| v)
                .map_err(|err| err.fill_position(op_pos))
            }
        }
    }

    /// Evaluate a chain of indexes and store the results in a [`StaticVec`].
    /// [`StaticVec`] is used to avoid an allocation in the overwhelming cases of
    /// just a few levels of indexing.
    #[cfg(any(not(feature = "no_index"), not(feature = "no_object")))]
    fn eval_dot_index_chain_arguments(
        &self,
        scope: &mut Scope,
        mods: &mut Imports,
        state: &mut EvalState,
        lib: &[&Module],
        this_ptr: &mut Option<&mut Dynamic>,
        expr: &Expr,
        terminate_chaining: bool,
        parent_chain_type: ChainType,
        idx_values: &mut StaticVec<ChainArgument>,
        size: usize,
        level: usize,
    ) -> Result<(), Box<EvalAltResult>> {
        #[cfg(not(feature = "unchecked"))]
        self.inc_operations(&mut mods.num_operations, expr.position())?;

        let _parent_chain_type = parent_chain_type;

        match expr {
            #[cfg(not(feature = "no_object"))]
            Expr::FnCall(x, _) if _parent_chain_type == ChainType::Dotting && !x.is_qualified() => {
                let crate::ast::FnCallExpr {
                    args, constants, ..
                } = x.as_ref();

                let (values, pos) = args.iter().try_fold(
                    (StaticVec::with_capacity(args.len()), Position::NONE),
                    |(mut values, mut pos), expr| -> Result<_, Box<EvalAltResult>> {
                        let (value, arg_pos) = self.get_arg_value(
                            scope, mods, state, lib, this_ptr, level, expr, constants,
                        )?;
                        if values.is_empty() {
                            pos = arg_pos;
                        }
                        values.push(value.flatten());
                        Ok((values, pos))
                    },
                )?;

                idx_values.push((values, pos).into());
            }
            #[cfg(not(feature = "no_object"))]
            Expr::FnCall(_, _) if _parent_chain_type == ChainType::Dotting => {
                unreachable!("function call in dot chain should not be namespace-qualified")
            }

            #[cfg(not(feature = "no_object"))]
            Expr::Property(x) if _parent_chain_type == ChainType::Dotting => {
                idx_values.push(ChainArgument::Property((x.2).1))
            }
            Expr::Property(_) => unreachable!("unexpected Expr::Property for indexing"),

            Expr::Index(x, term, _) | Expr::Dot(x, term, _) if !terminate_chaining => {
                let crate::ast::BinaryExpr { lhs, rhs, .. } = x.as_ref();

                // Evaluate in left-to-right order
                let lhs_val = match lhs {
                    #[cfg(not(feature = "no_object"))]
                    Expr::Property(x) if _parent_chain_type == ChainType::Dotting => {
                        ChainArgument::Property((x.2).1)
                    }
                    Expr::Property(_) => unreachable!("unexpected Expr::Property for indexing"),

                    #[cfg(not(feature = "no_object"))]
                    Expr::FnCall(x, _)
                        if _parent_chain_type == ChainType::Dotting && !x.is_qualified() =>
                    {
                        let crate::ast::FnCallExpr {
                            args, constants, ..
                        } = x.as_ref();

                        args.iter()
                            .try_fold(
                                (StaticVec::with_capacity(args.len()), Position::NONE),
                                |(mut values, mut pos), expr| -> Result<_, Box<EvalAltResult>> {
                                    let (value, arg_pos) = self.get_arg_value(
                                        scope, mods, state, lib, this_ptr, level, expr, constants,
                                    )?;
                                    if values.is_empty() {
                                        pos = arg_pos
                                    }
                                    values.push(value.flatten());
                                    Ok((values, pos))
                                },
                            )?
                            .into()
                    }
                    #[cfg(not(feature = "no_object"))]
                    Expr::FnCall(_, _) if _parent_chain_type == ChainType::Dotting => {
                        unreachable!("function call in dot chain should not be namespace-qualified")
                    }
                    #[cfg(not(feature = "no_object"))]
                    expr if _parent_chain_type == ChainType::Dotting => {
                        unreachable!("invalid dot expression: {:?}", expr);
                    }
                    #[cfg(not(feature = "no_index"))]
                    _ if _parent_chain_type == ChainType::Indexing => self
                        .eval_expr(scope, mods, state, lib, this_ptr, lhs, level)
                        .map(|v| (v.flatten(), lhs.position()).into())?,
                    expr => unreachable!("unknown chained expression: {:?}", expr),
                };

                // Push in reverse order
                let chain_type = match_chaining_type(expr);

                self.eval_dot_index_chain_arguments(
                    scope, mods, state, lib, this_ptr, rhs, *term, chain_type, idx_values, size,
                    level,
                )?;

                idx_values.push(lhs_val);
            }

            #[cfg(not(feature = "no_object"))]
            _ if _parent_chain_type == ChainType::Dotting => {
                unreachable!("invalid dot expression: {:?}", expr);
            }
            #[cfg(not(feature = "no_index"))]
            _ if _parent_chain_type == ChainType::Indexing => idx_values.push(
                self.eval_expr(scope, mods, state, lib, this_ptr, expr, level)
                    .map(|v| (v.flatten(), expr.position()).into())?,
            ),
            _ => unreachable!("unknown chained expression: {:?}", expr),
        }

        Ok(())
    }

    /// Get the value at the indexed position of a base type.
    /// [`Position`] in [`EvalAltResult`] may be [`NONE`][Position::NONE] and should be set afterwards.
    #[cfg(any(not(feature = "no_index"), not(feature = "no_object")))]
    fn get_indexed_mut<'t>(
        &self,
        mods: &mut Imports,
        state: &mut EvalState,
        lib: &[&Module],
        target: &'t mut Dynamic,
        idx: Dynamic,
        idx_pos: Position,
        add_if_not_found: bool,
        use_indexers: bool,
        level: usize,
    ) -> Result<Target<'t>, Box<EvalAltResult>> {
        #[cfg(not(feature = "unchecked"))]
        self.inc_operations(&mut mods.num_operations, Position::NONE)?;

        let mut idx = idx;
        let _add_if_not_found = add_if_not_found;

        match target {
            #[cfg(not(feature = "no_index"))]
            Dynamic(Union::Array(arr, _, _)) => {
                // val_array[idx]
                let index = idx
                    .as_int()
                    .map_err(|typ| self.make_type_mismatch_err::<crate::INT>(typ, idx_pos))?;

                let arr_len = arr.len();

                #[cfg(not(feature = "unchecked"))]
                let arr_idx = if index < 0 {
                    // Count from end if negative
                    arr_len
                        - index
                            .checked_abs()
                            .ok_or_else(|| EvalAltResult::ErrorArrayBounds(arr_len, index, idx_pos))
                            .and_then(|n| {
                                if n as usize > arr_len {
                                    Err(EvalAltResult::ErrorArrayBounds(arr_len, index, idx_pos)
                                        .into())
                                } else {
                                    Ok(n as usize)
                                }
                            })?
                } else {
                    index as usize
                };
                #[cfg(feature = "unchecked")]
                let arr_idx = if index < 0 {
                    // Count from end if negative
                    arr_len - index.abs() as usize
                } else {
                    index as usize
                };

                arr.get_mut(arr_idx)
                    .map(Target::from)
                    .ok_or_else(|| EvalAltResult::ErrorArrayBounds(arr_len, index, idx_pos).into())
            }

            #[cfg(not(feature = "no_index"))]
            Dynamic(Union::Blob(arr, _, _)) => {
                // val_blob[idx]
                let index = idx
                    .as_int()
                    .map_err(|typ| self.make_type_mismatch_err::<crate::INT>(typ, idx_pos))?;

                let arr_len = arr.len();

                #[cfg(not(feature = "unchecked"))]
                let arr_idx = if index < 0 {
                    // Count from end if negative
                    arr_len
                        - index
                            .checked_abs()
                            .ok_or_else(|| EvalAltResult::ErrorArrayBounds(arr_len, index, idx_pos))
                            .and_then(|n| {
                                if n as usize > arr_len {
                                    Err(EvalAltResult::ErrorArrayBounds(arr_len, index, idx_pos)
                                        .into())
                                } else {
                                    Ok(n as usize)
                                }
                            })?
                } else {
                    index as usize
                };
                #[cfg(feature = "unchecked")]
                let arr_idx = if index < 0 {
                    // Count from end if negative
                    arr_len - index.abs() as usize
                } else {
                    index as usize
                };

                let value = arr
                    .get(arr_idx)
                    .map(|&v| (v as INT).into())
                    .ok_or_else(|| {
                        Box::new(EvalAltResult::ErrorArrayBounds(arr_len, index, idx_pos))
                    })?;
                Ok(Target::BlobByte(target, arr_idx, value))
            }

            #[cfg(not(feature = "no_object"))]
            Dynamic(Union::Map(map, _, _)) => {
                // val_map[idx]
                let index = idx.read_lock::<ImmutableString>().ok_or_else(|| {
                    self.make_type_mismatch_err::<ImmutableString>(idx.type_name(), idx_pos)
                })?;

                if _add_if_not_found && !map.contains_key(index.as_str()) {
                    map.insert(index.clone().into(), Dynamic::UNIT);
                }

                Ok(map
                    .get_mut(index.as_str())
                    .map(Target::from)
                    .unwrap_or_else(|| Target::from(Dynamic::UNIT)))
            }

            #[cfg(not(feature = "no_index"))]
            Dynamic(Union::Int(value, _, _))
                if idx.is::<crate::ExclusiveRange>() || idx.is::<crate::InclusiveRange>() =>
            {
                #[cfg(not(feature = "only_i32"))]
                type BASE = u64;
                #[cfg(feature = "only_i32")]
                type BASE = u32;

                // val_int[range]
                const BITS: usize = std::mem::size_of::<INT>() * 8;

                let (shift, mask) = if let Some(range) = idx.read_lock::<crate::ExclusiveRange>() {
                    let start = range.start;
                    let end = range.end;

                    if start < 0 || start as usize >= BITS {
                        return Err(EvalAltResult::ErrorBitFieldBounds(BITS, start, idx_pos).into());
                    } else if end < 0 || end as usize >= BITS {
                        return Err(EvalAltResult::ErrorBitFieldBounds(BITS, end, idx_pos).into());
                    } else if end <= start {
                        (0, 0)
                    } else if end as usize == BITS && start == 0 {
                        // -1 = all bits set
                        (0, -1)
                    } else {
                        (
                            start as u8,
                            // 2^bits - 1
                            (((2 as BASE).pow((end - start) as u32) - 1) as INT) << start,
                        )
                    }
                } else if let Some(range) = idx.read_lock::<crate::InclusiveRange>() {
                    let start = *range.start();
                    let end = *range.end();

                    if start < 0 || start as usize >= BITS {
                        return Err(EvalAltResult::ErrorBitFieldBounds(BITS, start, idx_pos).into());
                    } else if end < 0 || end as usize >= BITS {
                        return Err(EvalAltResult::ErrorBitFieldBounds(BITS, end, idx_pos).into());
                    } else if end < start {
                        (0, 0)
                    } else if end as usize == BITS - 1 && start == 0 {
                        // -1 = all bits set
                        (0, -1)
                    } else {
                        (
                            start as u8,
                            // 2^bits - 1
                            (((2 as BASE).pow((end - start + 1) as u32) - 1) as INT) << start,
                        )
                    }
                } else {
                    unreachable!("`Range` or `RangeInclusive`");
                };

                let field_value = (*value & mask) >> shift;

                Ok(Target::BitField(target, mask, field_value.into(), shift))
            }

            #[cfg(not(feature = "no_index"))]
            Dynamic(Union::Int(value, _, _)) => {
                // val_int[idx]
                let index = idx
                    .as_int()
                    .map_err(|typ| self.make_type_mismatch_err::<crate::INT>(typ, idx_pos))?;

                const BITS: usize = std::mem::size_of::<INT>() * 8;

                let (bit_value, offset) = if index >= 0 {
                    let offset = index as usize;
                    (
                        if offset >= BITS {
                            return Err(
                                EvalAltResult::ErrorBitFieldBounds(BITS, index, idx_pos).into()
                            );
                        } else {
                            (*value & (1 << offset)) != 0
                        },
                        offset as u8,
                    )
                } else if let Some(abs_index) = index.checked_abs() {
                    let offset = abs_index as usize;
                    (
                        // Count from end if negative
                        if offset > BITS {
                            return Err(
                                EvalAltResult::ErrorBitFieldBounds(BITS, index, idx_pos).into()
                            );
                        } else {
                            (*value & (1 << (BITS - offset))) != 0
                        },
                        offset as u8,
                    )
                } else {
                    return Err(EvalAltResult::ErrorBitFieldBounds(BITS, index, idx_pos).into());
                };

                Ok(Target::Bit(target, bit_value.into(), offset))
            }

            #[cfg(not(feature = "no_index"))]
            Dynamic(Union::Str(s, _, _)) => {
                // val_string[idx]
                let index = idx
                    .as_int()
                    .map_err(|typ| self.make_type_mismatch_err::<crate::INT>(typ, idx_pos))?;

                let (ch, offset) = if index >= 0 {
                    let offset = index as usize;
                    (
                        s.chars().nth(offset).ok_or_else(|| {
                            let chars_len = s.chars().count();
                            EvalAltResult::ErrorStringBounds(chars_len, index, idx_pos)
                        })?,
                        offset,
                    )
                } else if let Some(abs_index) = index.checked_abs() {
                    let offset = abs_index as usize;
                    (
                        // Count from end if negative
                        s.chars().rev().nth(offset - 1).ok_or_else(|| {
                            let chars_len = s.chars().count();
                            EvalAltResult::ErrorStringBounds(chars_len, index, idx_pos)
                        })?,
                        offset,
                    )
                } else {
                    let chars_len = s.chars().count();
                    return Err(EvalAltResult::ErrorStringBounds(chars_len, index, idx_pos).into());
                };

                Ok(Target::StringChar(target, offset, ch.into()))
            }

            _ if use_indexers => {
                let args = &mut [target, &mut idx];
                let hash_get = crate::ast::FnCallHashes::from_native(mods.hash_idx_get());
                let idx_pos = Position::NONE;

                self.exec_fn_call(
                    mods, state, lib, FN_IDX_GET, hash_get, args, true, true, idx_pos, None, level,
                )
                .map(|(v, _)| v.into())
            }

            _ => Err(EvalAltResult::ErrorIndexingType(
                format!(
                    "{} [{}]",
                    self.map_type_name(target.type_name()),
                    self.map_type_name(idx.type_name())
                ),
                Position::NONE,
            )
            .into()),
        }
    }

    /// Evaluate an expression.
    pub(crate) fn eval_expr(
        &self,
        scope: &mut Scope,
        mods: &mut Imports,
        state: &mut EvalState,
        lib: &[&Module],
        this_ptr: &mut Option<&mut Dynamic>,
        expr: &Expr,
        level: usize,
    ) -> RhaiResult {
        #[cfg(not(feature = "unchecked"))]
        self.inc_operations(&mut mods.num_operations, expr.position())?;

        let result = match expr {
            Expr::DynamicConstant(x, _) => Ok(x.as_ref().clone()),
            Expr::IntegerConstant(x, _) => Ok((*x).into()),
            #[cfg(not(feature = "no_float"))]
            Expr::FloatConstant(x, _) => Ok((*x).into()),
            Expr::StringConstant(x, _) => Ok(x.clone().into()),
            Expr::CharConstant(x, _) => Ok((*x).into()),

            Expr::Variable(None, var_pos, x) if x.0.is_none() && x.2 == KEYWORD_THIS => this_ptr
                .as_deref()
                .cloned()
                .ok_or_else(|| EvalAltResult::ErrorUnboundThis(*var_pos).into()),
            Expr::Variable(_, _, _) => self
                .search_namespace(scope, mods, state, lib, this_ptr, expr)
                .map(|(val, _)| val.take_or_clone()),

            // Statement block
            Expr::Stmt(x) if x.is_empty() => Ok(Dynamic::UNIT),
            Expr::Stmt(x) => {
                self.eval_stmt_block(scope, mods, state, lib, this_ptr, x, true, true, level)
            }

            // lhs[idx_expr]
            #[cfg(not(feature = "no_index"))]
            Expr::Index(_, _, _) => {
                self.eval_dot_index_chain(scope, mods, state, lib, this_ptr, expr, level, None)
            }

            // lhs.dot_rhs
            #[cfg(not(feature = "no_object"))]
            Expr::Dot(_, _, _) => {
                self.eval_dot_index_chain(scope, mods, state, lib, this_ptr, expr, level, None)
            }

            // `... ${...} ...`
            Expr::InterpolatedString(x, pos) => {
                let mut pos = *pos;
                let mut result: Dynamic = self.const_empty_string().into();

                x.iter().try_for_each(|expr| {
                    let item = self.eval_expr(scope, mods, state, lib, this_ptr, expr, level)?;

                    self.eval_op_assignment(
                        mods,
                        state,
                        lib,
                        Some(OpAssignment::new(TOKEN_OP_CONCAT)),
                        pos,
                        &mut (&mut result).into(),
                        ("", Position::NONE),
                        item,
                    )
                    .map_err(|err| err.fill_position(expr.position()))?;

                    pos = expr.position();

                    self.check_data_size(&result)
                        .map_err(|err| err.fill_position(pos))
                })?;

                assert!(
                    result.is::<ImmutableString>(),
                    "interpolated string must be a string"
                );

                Ok(result)
            }

            #[cfg(not(feature = "no_index"))]
            Expr::Array(x, _) => Ok(x
                .iter()
                .try_fold(
                    crate::Array::with_capacity(x.len()),
                    |mut arr, item| -> Result<_, Box<EvalAltResult>> {
                        arr.push(
                            self.eval_expr(scope, mods, state, lib, this_ptr, item, level)?
                                .flatten(),
                        );
                        Ok(arr)
                    },
                )?
                .into()),

            #[cfg(not(feature = "no_object"))]
            Expr::Map(x, _) => Ok(x
                .0
                .iter()
                .try_fold(
                    x.1.clone(),
                    |mut map, (Ident { name: key, .. }, expr)| -> Result<_, Box<EvalAltResult>> {
                        let value_ref = map.get_mut(key.as_str()).expect("contains all keys");
                        *value_ref = self
                            .eval_expr(scope, mods, state, lib, this_ptr, expr, level)?
                            .flatten();
                        Ok(map)
                    },
                )?
                .into()),

            // Namespace-qualified function call
            Expr::FnCall(x, pos) if x.is_qualified() => {
                let FnCallExpr {
                    name,
                    namespace,
                    hashes,
                    args,
                    constants,
                    ..
                } = x.as_ref();
                let namespace = namespace.as_ref().expect("qualified function call");
                let hash = hashes.native;
                self.make_qualified_function_call(
                    scope, mods, state, lib, this_ptr, namespace, name, args, constants, hash,
                    *pos, level,
                )
            }

            // Normal function call
            Expr::FnCall(x, pos) => {
                let FnCallExpr {
                    name,
                    capture_parent_scope: capture,
                    hashes,
                    args,
                    constants,
                    ..
                } = x.as_ref();
                self.make_function_call(
                    scope, mods, state, lib, this_ptr, name, args, constants, *hashes, *pos,
                    *capture, level,
                )
            }

            Expr::And(x, _) => {
                Ok((self
                    .eval_expr(scope, mods, state, lib, this_ptr, &x.lhs, level)?
                    .as_bool()
                    .map_err(|typ| self.make_type_mismatch_err::<bool>(typ, x.lhs.position()))?
                    && // Short-circuit using &&
                self
                    .eval_expr(scope, mods, state, lib, this_ptr, &x.rhs, level)?
                    .as_bool()
                    .map_err(|typ| self.make_type_mismatch_err::<bool>(typ, x.rhs.position()))?)
                .into())
            }

            Expr::Or(x, _) => {
                Ok((self
                    .eval_expr(scope, mods, state, lib, this_ptr, &x.lhs, level)?
                    .as_bool()
                    .map_err(|typ| self.make_type_mismatch_err::<bool>(typ, x.lhs.position()))?
                    || // Short-circuit using ||
                self
                    .eval_expr(scope, mods, state, lib, this_ptr, &x.rhs, level)?
                    .as_bool()
                    .map_err(|typ| self.make_type_mismatch_err::<bool>(typ, x.rhs.position()))?)
                .into())
            }

            Expr::BoolConstant(x, _) => Ok((*x).into()),
            Expr::Unit(_) => Ok(Dynamic::UNIT),

            Expr::Custom(custom, _) => {
                let expressions: StaticVec<_> = custom.inputs.iter().map(Into::into).collect();
                let key_token = custom.tokens.first().expect("not empty");
                let custom_def = self.custom_syntax.get(key_token).expect("must match");
                let mut context = EvalContext {
                    engine: self,
                    scope,
                    mods,
                    state,
                    lib,
                    this_ptr,
                    level,
                };
                (custom_def.func)(&mut context, &expressions)
            }

            _ => unreachable!("expression cannot be evaluated: {:?}", expr),
        };

        self.check_return_value(result)
            .map_err(|err| err.fill_position(expr.position()))
    }

    /// Evaluate a statements block.
    pub(crate) fn eval_stmt_block(
        &self,
        scope: &mut Scope,
        mods: &mut Imports,
        state: &mut EvalState,
        lib: &[&Module],
        this_ptr: &mut Option<&mut Dynamic>,
        statements: &[Stmt],
        restore_orig_state: bool,
        rewind_scope: bool,
        level: usize,
    ) -> RhaiResult {
        if statements.is_empty() {
            return Ok(Dynamic::UNIT);
        }

        let orig_always_search_scope = state.always_search_scope;
        let orig_scope_len = scope.len();
        let orig_mods_len = mods.len();
        let orig_fn_resolution_caches_len = state.fn_resolution_caches_len();

        if rewind_scope {
            state.scope_level += 1;
        }

        let result = statements.iter().try_fold(Dynamic::UNIT, |_, stmt| {
            let _mods_len = mods.len();

            let r = self.eval_stmt(scope, mods, state, lib, this_ptr, stmt, level)?;

            #[cfg(not(feature = "no_module"))]
            if matches!(stmt, Stmt::Import(_, _, _)) {
                // Get the extra modules - see if any functions are marked global.
                // Without global functions, the extra modules never affect function resolution.
                if mods
                    .scan_raw()
                    .skip(_mods_len)
                    .any(|(_, m)| m.contains_indexed_global_functions())
                {
                    if state.fn_resolution_caches_len() > orig_fn_resolution_caches_len {
                        // When new module is imported with global functions and there is already
                        // a new cache, clear it - notice that this is expensive as all function
                        // resolutions must start again
                        state.fn_resolution_cache_mut().clear();
                    } else if restore_orig_state {
                        // When new module is imported with global functions, push a new cache
                        state.push_fn_resolution_cache();
                    } else {
                        // When the block is to be evaluated in-place, just clear the current cache
                        state.fn_resolution_cache_mut().clear();
                    }
                }
            }

            Ok(r)
        });

        // If imports list is modified, pop the functions lookup cache
        state.rewind_fn_resolution_caches(orig_fn_resolution_caches_len);

        if rewind_scope {
            scope.rewind(orig_scope_len);
            state.scope_level -= 1;
        }
        if restore_orig_state {
            mods.truncate(orig_mods_len);

            // The impact of new local variables goes away at the end of a block
            // because any new variables introduced will go out of scope
            state.always_search_scope = orig_always_search_scope;
        }

        result
    }

    /// Evaluate an op-assignment statement.
    /// [`Position`] in [`EvalAltResult`] is [`NONE`][Position::NONE] and should be set afterwards.
    pub(crate) fn eval_op_assignment(
        &self,
        mods: &mut Imports,
        state: &mut EvalState,
        lib: &[&Module],
        op_info: Option<OpAssignment>,
        op_pos: Position,
        target: &mut Target,
        root: (&str, Position),
        new_val: Dynamic,
    ) -> Result<(), Box<EvalAltResult>> {
        if target.is_read_only() {
            // Assignment to constant variable
            return Err(
                EvalAltResult::ErrorAssignmentToConstant(root.0.to_string(), root.1).into(),
            );
        }

        let mut new_val = new_val;

        if let Some(OpAssignment {
            hash_op_assign,
            hash_op,
            op,
        }) = op_info
        {
            {
                let mut lock_guard;
                let lhs_ptr_inner;

                #[cfg(not(feature = "no_closure"))]
                let target_is_shared = target.is_shared();
                #[cfg(feature = "no_closure")]
                let target_is_shared = false;

                if target_is_shared {
                    lock_guard = target.write_lock::<Dynamic>().expect("`Dynamic`");
                    lhs_ptr_inner = &mut *lock_guard;
                } else {
                    lhs_ptr_inner = &mut *target;
                }

                let hash = hash_op_assign;
                let args = &mut [lhs_ptr_inner, &mut new_val];

                match self.call_native_fn(mods, state, lib, op, hash, args, true, true, op_pos) {
                    Err(err) if matches!(*err, EvalAltResult::ErrorFunctionNotFound(ref f, _) if f.starts_with(op)) =>
                    {
                        // Expand to `var = var op rhs`
                        let op = &op[..op.len() - 1]; // extract operator without =

                        // Run function
                        let (value, _) = self.call_native_fn(
                            mods, state, lib, op, hash_op, args, true, false, op_pos,
                        )?;

                        *args[0] = value.flatten();
                    }
                    err => return err.map(|_| ()),
                }
            }
        } else {
            // Normal assignment
            *target.as_mut() = new_val;
        }

        target.propagate_changed_value()
    }

    /// Evaluate a statement.
    ///
    /// # Safety
    ///
    /// This method uses some unsafe code, mainly for avoiding cloning of local variable names via
    /// direct lifetime casting.
    pub(crate) fn eval_stmt(
        &self,
        scope: &mut Scope,
        mods: &mut Imports,
        state: &mut EvalState,
        lib: &[&Module],
        this_ptr: &mut Option<&mut Dynamic>,
        stmt: &Stmt,
        level: usize,
    ) -> RhaiResult {
        #[cfg(not(feature = "unchecked"))]
        self.inc_operations(&mut mods.num_operations, stmt.position())?;

        let result = match stmt {
            // No-op
            Stmt::Noop(_) => Ok(Dynamic::UNIT),

            // Expression as statement
            Stmt::Expr(expr) => Ok(self
                .eval_expr(scope, mods, state, lib, this_ptr, expr, level)?
                .flatten()),

            // var op= rhs
            Stmt::Assignment(x, op_pos) if x.0.is_variable_access(false) => {
                let (lhs_expr, op_info, rhs_expr) = x.as_ref();
                let rhs_val = self
                    .eval_expr(scope, mods, state, lib, this_ptr, rhs_expr, level)?
                    .flatten();
                let (mut lhs_ptr, pos) =
                    self.search_namespace(scope, mods, state, lib, this_ptr, lhs_expr)?;

                let var_name = lhs_expr.get_variable_name(false).expect("`Variable`");

                if !lhs_ptr.is_ref() {
                    return Err(EvalAltResult::ErrorAssignmentToConstant(
                        var_name.to_string(),
                        pos,
                    )
                    .into());
                }

                #[cfg(not(feature = "unchecked"))]
                self.inc_operations(&mut mods.num_operations, pos)?;

                self.eval_op_assignment(
                    mods,
                    state,
                    lib,
                    *op_info,
                    *op_pos,
                    &mut lhs_ptr,
                    (var_name, pos),
                    rhs_val,
                )
                .map_err(|err| err.fill_position(rhs_expr.position()))?;

                if op_info.is_some() {
                    self.check_data_size(lhs_ptr.as_ref())
                        .map_err(|err| err.fill_position(lhs_expr.position()))?;
                }

                Ok(Dynamic::UNIT)
            }

            // lhs op= rhs
            Stmt::Assignment(x, op_pos) => {
                let (lhs_expr, op_info, rhs_expr) = x.as_ref();
                let rhs_val = self
                    .eval_expr(scope, mods, state, lib, this_ptr, rhs_expr, level)?
                    .flatten();
                let _new_val = Some(((rhs_val, rhs_expr.position()), (*op_info, *op_pos)));

                // Must be either `var[index] op= val` or `var.prop op= val`
                match lhs_expr {
                    // name op= rhs (handled above)
                    Expr::Variable(_, _, _) => {
                        unreachable!("Expr::Variable case should already been handled")
                    }
                    // idx_lhs[idx_expr] op= rhs
                    #[cfg(not(feature = "no_index"))]
                    Expr::Index(_, _, _) => {
                        self.eval_dot_index_chain(
                            scope, mods, state, lib, this_ptr, lhs_expr, level, _new_val,
                        )?;
                        Ok(Dynamic::UNIT)
                    }
                    // dot_lhs.dot_rhs op= rhs
                    #[cfg(not(feature = "no_object"))]
                    Expr::Dot(_, _, _) => {
                        self.eval_dot_index_chain(
                            scope, mods, state, lib, this_ptr, lhs_expr, level, _new_val,
                        )?;
                        Ok(Dynamic::UNIT)
                    }
                    _ => unreachable!("cannot assign to expression: {:?}", lhs_expr),
                }
            }

            // Block scope
            Stmt::Block(statements, _) if statements.is_empty() => Ok(Dynamic::UNIT),
            Stmt::Block(statements, _) => self.eval_stmt_block(
                scope, mods, state, lib, this_ptr, statements, true, true, level,
            ),

            // If statement
            Stmt::If(expr, x, _) => {
                let guard_val = self
                    .eval_expr(scope, mods, state, lib, this_ptr, expr, level)?
                    .as_bool()
                    .map_err(|typ| self.make_type_mismatch_err::<bool>(typ, expr.position()))?;

                if guard_val {
                    if !x.0.is_empty() {
                        self.eval_stmt_block(
                            scope, mods, state, lib, this_ptr, &x.0, true, true, level,
                        )
                    } else {
                        Ok(Dynamic::UNIT)
                    }
                } else {
                    if !x.1.is_empty() {
                        self.eval_stmt_block(
                            scope, mods, state, lib, this_ptr, &x.1, true, true, level,
                        )
                    } else {
                        Ok(Dynamic::UNIT)
                    }
                }
            }

            // Switch statement
            Stmt::Switch(match_expr, x, _) => {
                let (table, def_stmt, ranges) = x.as_ref();

                let value = self.eval_expr(scope, mods, state, lib, this_ptr, match_expr, level)?;

                let stmt_block = if value.is_hashable() {
                    let hasher = &mut get_hasher();
                    value.hash(hasher);
                    let hash = hasher.finish();

                    // First check hashes
                    if let Some(t) = table.get(&hash) {
                        if let Some(ref c) = t.0 {
                            if self
                                .eval_expr(scope, mods, state, lib, this_ptr, &c, level)
                                .and_then(|v| {
                                    v.as_bool().map_err(|typ| {
                                        self.make_type_mismatch_err::<bool>(typ, c.position())
                                    })
                                })?
                            {
                                Some(&t.1)
                            } else {
                                None
                            }
                        } else {
                            Some(&t.1)
                        }
                    } else if value.is::<INT>() && !ranges.is_empty() {
                        // Then check integer ranges
                        let value = value.as_int().expect("`INT`");
                        let mut result = None;

                        for (_, _, _, condition, stmt_block) in
                            ranges.iter().filter(|&&(start, end, inclusive, _, _)| {
                                (!inclusive && (start..end).contains(&value))
                                    || (inclusive && (start..=end).contains(&value))
                            })
                        {
                            if let Some(c) = condition {
                                if !self
                                    .eval_expr(scope, mods, state, lib, this_ptr, &c, level)
                                    .and_then(|v| {
                                        v.as_bool().map_err(|typ| {
                                            self.make_type_mismatch_err::<bool>(typ, c.position())
                                        })
                                    })?
                                {
                                    continue;
                                }
                            }

                            result = Some(stmt_block);
                            break;
                        }

                        result
                    } else {
                        // Nothing matches
                        None
                    }
                } else {
                    // Non-hashable
                    None
                };

                if let Some(statements) = stmt_block {
                    if !statements.is_empty() {
                        self.eval_stmt_block(
                            scope, mods, state, lib, this_ptr, statements, true, true, level,
                        )
                    } else {
                        Ok(Dynamic::UNIT)
                    }
                } else {
                    // Default match clause
                    if !def_stmt.is_empty() {
                        self.eval_stmt_block(
                            scope, mods, state, lib, this_ptr, def_stmt, true, true, level,
                        )
                    } else {
                        Ok(Dynamic::UNIT)
                    }
                }
            }

            // Loop
            Stmt::While(Expr::Unit(_), body, _) => loop {
                if !body.is_empty() {
                    match self
                        .eval_stmt_block(scope, mods, state, lib, this_ptr, body, true, true, level)
                    {
                        Ok(_) => (),
                        Err(err) => match *err {
                            EvalAltResult::LoopBreak(false, _) => (),
                            EvalAltResult::LoopBreak(true, _) => return Ok(Dynamic::UNIT),
                            _ => return Err(err),
                        },
                    }
                } else {
                    #[cfg(not(feature = "unchecked"))]
                    self.inc_operations(&mut mods.num_operations, body.position())?;
                }
            },

            // While loop
            Stmt::While(expr, body, _) => loop {
                let condition = self
                    .eval_expr(scope, mods, state, lib, this_ptr, expr, level)?
                    .as_bool()
                    .map_err(|typ| self.make_type_mismatch_err::<bool>(typ, expr.position()))?;

                if !condition {
                    return Ok(Dynamic::UNIT);
                }
                if !body.is_empty() {
                    match self
                        .eval_stmt_block(scope, mods, state, lib, this_ptr, body, true, true, level)
                    {
                        Ok(_) => (),
                        Err(err) => match *err {
                            EvalAltResult::LoopBreak(false, _) => (),
                            EvalAltResult::LoopBreak(true, _) => return Ok(Dynamic::UNIT),
                            _ => return Err(err),
                        },
                    }
                }
            },

            // Do loop
            Stmt::Do(body, expr, options, _) => loop {
                let is_while = !options.contains(AST_OPTION_NEGATED);

                if !body.is_empty() {
                    match self
                        .eval_stmt_block(scope, mods, state, lib, this_ptr, body, true, true, level)
                    {
                        Ok(_) => (),
                        Err(err) => match *err {
                            EvalAltResult::LoopBreak(false, _) => continue,
                            EvalAltResult::LoopBreak(true, _) => return Ok(Dynamic::UNIT),
                            _ => return Err(err),
                        },
                    }
                }

                let condition = self
                    .eval_expr(scope, mods, state, lib, this_ptr, expr, level)?
                    .as_bool()
                    .map_err(|typ| self.make_type_mismatch_err::<bool>(typ, expr.position()))?;

                if condition ^ is_while {
                    return Ok(Dynamic::UNIT);
                }
            },

            // For loop
            Stmt::For(expr, x, _) => {
                let (Ident { name, .. }, counter, statements) = x.as_ref();
                let iter_obj = self
                    .eval_expr(scope, mods, state, lib, this_ptr, expr, level)?
                    .flatten();
                let iter_type = iter_obj.type_id();

                // lib should only contain scripts, so technically they cannot have iterators

                // Search order:
                // 1) Global namespace - functions registered via Engine::register_XXX
                // 2) Global modules - packages
                // 3) Imported modules - functions marked with global namespace
                // 4) Global sub-modules - functions marked with global namespace
                let func = self
                    .global_modules
                    .iter()
                    .find_map(|m| m.get_iter(iter_type))
                    .or_else(|| mods.get_iter(iter_type))
                    .or_else(|| {
                        self.global_sub_modules
                            .values()
                            .find_map(|m| m.get_qualified_iter(iter_type))
                    });

                if let Some(func) = func {
                    // Add the loop variables
                    let orig_scope_len = scope.len();
                    let counter_index = counter.as_ref().map(|Ident { name, .. }| {
                        scope.push(unsafe_cast_var_name_to_lifetime(name), 0 as INT);
                        scope.len() - 1
                    });
                    scope.push(unsafe_cast_var_name_to_lifetime(name), ());
                    let index = scope.len() - 1;

                    for (x, iter_value) in func(iter_obj).enumerate() {
                        // Increment counter
                        if let Some(c) = counter_index {
                            #[cfg(not(feature = "unchecked"))]
                            if x > INT::MAX as usize {
                                return Err(EvalAltResult::ErrorArithmetic(
                                    format!("for-loop counter overflow: {}", x),
                                    counter.as_ref().expect("`Some`").pos,
                                )
                                .into());
                            }

                            let mut counter_var = scope
                                .get_mut_by_index(c)
                                .write_lock::<INT>()
                                .expect("`INT`");
                            *counter_var = x as INT;
                        }

                        let loop_var = scope.get_mut_by_index(index);
                        let value = iter_value.flatten();

                        #[cfg(not(feature = "no_closure"))]
                        let loop_var_is_shared = loop_var.is_shared();
                        #[cfg(feature = "no_closure")]
                        let loop_var_is_shared = false;

                        if loop_var_is_shared {
                            let mut value_ref = loop_var.write_lock().expect("`Dynamic`");
                            *value_ref = value;
                        } else {
                            *loop_var = value;
                        }

                        #[cfg(not(feature = "unchecked"))]
                        self.inc_operations(&mut mods.num_operations, statements.position())?;

                        if statements.is_empty() {
                            continue;
                        }

                        let result = self.eval_stmt_block(
                            scope, mods, state, lib, this_ptr, statements, true, true, level,
                        );

                        match result {
                            Ok(_) => (),
                            Err(err) => match *err {
                                EvalAltResult::LoopBreak(false, _) => (),
                                EvalAltResult::LoopBreak(true, _) => break,
                                _ => return Err(err),
                            },
                        }
                    }

                    scope.rewind(orig_scope_len);
                    Ok(Dynamic::UNIT)
                } else {
                    Err(EvalAltResult::ErrorFor(expr.position()).into())
                }
            }

            // Continue/Break statement
            Stmt::BreakLoop(options, pos) => {
                Err(EvalAltResult::LoopBreak(options.contains(AST_OPTION_BREAK_OUT), *pos).into())
            }

            // Namespace-qualified function call
            Stmt::FnCall(x, pos) if x.is_qualified() => {
                let FnCallExpr {
                    name,
                    namespace,
                    hashes,
                    args,
                    constants,
                    ..
                } = x.as_ref();
                let namespace = namespace.as_ref().expect("qualified function call");
                let hash = hashes.native;
                self.make_qualified_function_call(
                    scope, mods, state, lib, this_ptr, namespace, name, args, constants, hash,
                    *pos, level,
                )
            }

            // Normal function call
            Stmt::FnCall(x, pos) => {
                let FnCallExpr {
                    name,
                    capture_parent_scope: capture,
                    hashes,
                    args,
                    constants,
                    ..
                } = x.as_ref();
                self.make_function_call(
                    scope, mods, state, lib, this_ptr, name, args, constants, *hashes, *pos,
                    *capture, level,
                )
            }

            // Try/Catch statement
            Stmt::TryCatch(x, _) => {
                let (try_stmt, err_var, catch_stmt) = x.as_ref();

                let result = self
                    .eval_stmt_block(
                        scope, mods, state, lib, this_ptr, try_stmt, true, true, level,
                    )
                    .map(|_| Dynamic::UNIT);

                match result {
                    Ok(_) => result,
                    Err(err) if err.is_pseudo_error() => Err(err),
                    Err(err) if !err.is_catchable() => Err(err),
                    Err(mut err) => {
                        let err_value = match *err {
                            EvalAltResult::ErrorRuntime(ref x, _) => x.clone(),

                            #[cfg(feature = "no_object")]
                            _ => {
                                err.take_position();
                                err.to_string().into()
                            }
                            #[cfg(not(feature = "no_object"))]
                            _ => {
                                let mut err_map = crate::Map::new();
                                let err_pos = err.take_position();

                                err_map.insert("message".into(), err.to_string().into());

                                if let Some(ref source) = mods.source {
                                    err_map.insert("source".into(), source.as_str().into());
                                }

                                if err_pos.is_none() {
                                    // No position info
                                } else {
                                    let line = err_pos.line().expect("line number") as INT;
                                    let position = if err_pos.is_beginning_of_line() {
                                        0
                                    } else {
                                        err_pos.position().expect("character position")
                                    } as INT;
                                    err_map.insert("line".into(), line.into());
                                    err_map.insert("position".into(), position.into());
                                }

                                err.dump_fields(&mut err_map);
                                err_map.into()
                            }
                        };

                        let orig_scope_len = scope.len();

                        err_var.as_ref().map(|Ident { name, .. }| {
                            scope.push(unsafe_cast_var_name_to_lifetime(name), err_value)
                        });

                        let result = self.eval_stmt_block(
                            scope, mods, state, lib, this_ptr, catch_stmt, true, true, level,
                        );

                        scope.rewind(orig_scope_len);

                        match result {
                            Ok(_) => Ok(Dynamic::UNIT),
                            Err(result_err) => match *result_err {
                                // Re-throw exception
                                EvalAltResult::ErrorRuntime(Dynamic(Union::Unit(_, _, _)), pos) => {
                                    err.set_position(pos);
                                    Err(err)
                                }
                                _ => Err(result_err),
                            },
                        }
                    }
                }
            }

            // Throw value
            Stmt::Return(options, Some(expr), pos) if options.contains(AST_OPTION_BREAK_OUT) => {
                Err(EvalAltResult::ErrorRuntime(
                    self.eval_expr(scope, mods, state, lib, this_ptr, expr, level)?
                        .flatten(),
                    *pos,
                )
                .into())
            }

            // Empty throw
            Stmt::Return(options, None, pos) if options.contains(AST_OPTION_BREAK_OUT) => {
                Err(EvalAltResult::ErrorRuntime(Dynamic::UNIT, *pos).into())
            }

            // Return value
            Stmt::Return(_, Some(expr), pos) => Err(EvalAltResult::Return(
                self.eval_expr(scope, mods, state, lib, this_ptr, expr, level)?
                    .flatten(),
                *pos,
            )
            .into()),

            // Empty return
            Stmt::Return(_, None, pos) => Err(EvalAltResult::Return(Dynamic::UNIT, *pos).into()),

            // Let/const statement
            Stmt::Var(expr, x, options, _) => {
                let name = &x.name;
                let entry_type = if options.contains(AST_OPTION_CONSTANT) {
                    AccessMode::ReadOnly
                } else {
                    AccessMode::ReadWrite
                };
                let export = options.contains(AST_OPTION_PUBLIC);

                let value = self
                    .eval_expr(scope, mods, state, lib, this_ptr, expr, level)?
                    .flatten();

                let (var_name, _alias): (Cow<'_, str>, _) = if state.is_global() {
                    #[cfg(not(feature = "no_function"))]
                    #[cfg(not(feature = "no_module"))]
                    if entry_type == AccessMode::ReadOnly && lib.iter().any(|&m| !m.is_empty()) {
                        mods.set_global_constant(name.clone(), value.clone());
                    }

                    (
                        name.to_string().into(),
                        if export { Some(name.clone()) } else { None },
                    )
                } else if export {
                    unreachable!("exported variable not on global level");
                } else {
                    (unsafe_cast_var_name_to_lifetime(name).into(), None)
                };

                scope.push_dynamic_value(var_name, entry_type, value);

                #[cfg(not(feature = "no_module"))]
                _alias.map(|alias| scope.add_entry_alias(scope.len() - 1, alias));

                Ok(Dynamic::UNIT)
            }

            // Import statement
            #[cfg(not(feature = "no_module"))]
            Stmt::Import(expr, export, _pos) => {
                // Guard against too many modules
                #[cfg(not(feature = "unchecked"))]
                if mods.num_modules >= self.max_modules() {
                    return Err(EvalAltResult::ErrorTooManyModules(*_pos).into());
                }

                if let Some(path) = self
                    .eval_expr(scope, mods, state, lib, this_ptr, &expr, level)?
                    .try_cast::<ImmutableString>()
                {
                    use crate::ModuleResolver;

                    let source = mods.source.as_ref().map(|s| s.as_str());
                    let path_pos = expr.position();

                    let module = mods
                        .embedded_module_resolver
                        .as_ref()
                        .and_then(|r| match r.resolve(self, source, &path, path_pos) {
                            Err(err)
                                if matches!(*err, EvalAltResult::ErrorModuleNotFound(_, _)) =>
                            {
                                None
                            }
                            result => Some(result),
                        })
                        .or_else(|| {
                            self.module_resolver
                                .as_ref()
                                .map(|r| r.resolve(self, source, &path, path_pos))
                        })
                        .unwrap_or_else(|| {
                            Err(
                                EvalAltResult::ErrorModuleNotFound(path.to_string(), path_pos)
                                    .into(),
                            )
                        })?;

                    if let Some(name) = export.as_ref().map(|x| x.name.clone()) {
                        if !module.is_indexed() {
                            // Index the module (making a clone copy if necessary) if it is not indexed
                            let mut module = crate::func::native::shared_take_or_clone(module);
                            module.build_index();
                            mods.push(name, module);
                        } else {
                            mods.push(name, module);
                        }
                    }

                    mods.num_modules += 1;

                    Ok(Dynamic::UNIT)
                } else {
                    Err(self.make_type_mismatch_err::<ImmutableString>("", expr.position()))
                }
            }

            // Export statement
            #[cfg(not(feature = "no_module"))]
            Stmt::Export(list, _) => {
                list.iter().try_for_each(
                    |(Ident { name, pos, .. }, Ident { name: rename, .. })| {
                        // Mark scope variables as public
                        if let Some((index, _)) = scope.get_index(name) {
                            scope.add_entry_alias(
                                index,
                                if rename.is_empty() { name } else { rename }.clone(),
                            );
                            Ok(()) as Result<_, Box<EvalAltResult>>
                        } else {
                            Err(EvalAltResult::ErrorVariableNotFound(name.to_string(), *pos).into())
                        }
                    },
                )?;
                Ok(Dynamic::UNIT)
            }

            // Share statement
            #[cfg(not(feature = "no_closure"))]
            Stmt::Share(name) => {
                if let Some((index, _)) = scope.get_index(name) {
                    let val = scope.get_mut_by_index(index);

                    if !val.is_shared() {
                        // Replace the variable with a shared value.
                        *val = std::mem::take(val).into_shared();
                    }
                }
                Ok(Dynamic::UNIT)
            }
        };

        self.check_return_value(result)
            .map_err(|err| err.fill_position(stmt.position()))
    }

    // Has a system function a Rust-native override?
    pub(crate) fn has_native_fn_override(&self, hash_script: u64, arg_types: &[TypeId]) -> bool {
        let hash_params = calc_fn_params_hash(arg_types.iter().cloned());
        let hash = combine_hashes(hash_script, hash_params);

        // First check the global namespace and packages, but skip modules that are standard because
        // they should never conflict with system functions.
        self.global_modules.iter().filter(|m| !m.standard).any(|m| m.contains_fn(hash))
            // Then check sub-modules
            || self.global_sub_modules.values().any(|m| m.contains_qualified_fn(hash))
    }

    /// Check a result to ensure that the data size is within allowable limit.
    fn check_return_value(&self, mut result: RhaiResult) -> RhaiResult {
        if let Ok(ref mut r) = result {
            // Concentrate all empty strings into one instance to save memory
            if let Dynamic(crate::types::dynamic::Union::Str(s, _, _)) = r {
                if s.is_empty() {
                    if !s.ptr_eq(&self.empty_string) {
                        *s = self.const_empty_string();
                    }
                    return result;
                }
            }

            #[cfg(not(feature = "unchecked"))]
            self.check_data_size(&r)?;
        }

        result
    }

    #[cfg(feature = "unchecked")]
    #[inline(always)]
    fn check_data_size(&self, _value: &Dynamic) -> Result<(), Box<EvalAltResult>> {
        Ok(())
    }

    #[cfg(not(feature = "unchecked"))]
    fn check_data_size(&self, value: &Dynamic) -> Result<(), Box<EvalAltResult>> {
        // Recursively calculate the size of a value (especially `Array` and `Map`)
        fn calc_size(value: &Dynamic) -> (usize, usize, usize) {
            match value.0 {
                #[cfg(not(feature = "no_index"))]
                Union::Array(ref arr, _, _) => {
                    arr.iter()
                        .fold((0, 0, 0), |(arrays, maps, strings), value| match value.0 {
                            Union::Array(_, _, _) => {
                                let (a, m, s) = calc_size(value);
                                (arrays + a + 1, maps + m, strings + s)
                            }
                            Union::Blob(ref a, _, _) => (arrays + 1 + a.len(), maps, strings),
                            #[cfg(not(feature = "no_object"))]
                            Union::Map(_, _, _) => {
                                let (a, m, s) = calc_size(value);
                                (arrays + a + 1, maps + m, strings + s)
                            }
                            Union::Str(ref s, _, _) => (arrays + 1, maps, strings + s.len()),
                            _ => (arrays + 1, maps, strings),
                        })
                }
                #[cfg(not(feature = "no_index"))]
                Union::Blob(ref arr, _, _) => (arr.len(), 0, 0),
                #[cfg(not(feature = "no_object"))]
                Union::Map(ref map, _, _) => {
                    map.values()
                        .fold((0, 0, 0), |(arrays, maps, strings), value| match value.0 {
                            #[cfg(not(feature = "no_index"))]
                            Union::Array(_, _, _) => {
                                let (a, m, s) = calc_size(value);
                                (arrays + a, maps + m + 1, strings + s)
                            }
                            #[cfg(not(feature = "no_index"))]
                            Union::Blob(ref a, _, _) => (arrays + a.len(), maps, strings),
                            Union::Map(_, _, _) => {
                                let (a, m, s) = calc_size(value);
                                (arrays + a, maps + m + 1, strings + s)
                            }
                            Union::Str(ref s, _, _) => (arrays, maps + 1, strings + s.len()),
                            _ => (arrays, maps + 1, strings),
                        })
                }
                Union::Str(ref s, _, _) => (0, 0, s.len()),
                _ => (0, 0, 0),
            }
        }

        // If no data size limits, just return
        let mut _has_limit = self.limits.max_string_size.is_some();
        #[cfg(not(feature = "no_index"))]
        {
            _has_limit = _has_limit || self.limits.max_array_size.is_some();
        }
        #[cfg(not(feature = "no_object"))]
        {
            _has_limit = _has_limit || self.limits.max_map_size.is_some();
        }

        if !_has_limit {
            return Ok(());
        }

        let (_arr, _map, s) = calc_size(value);

        if s > self
            .limits
            .max_string_size
            .map_or(usize::MAX, NonZeroUsize::get)
        {
            return Err(EvalAltResult::ErrorDataTooLarge(
                "Length of string".to_string(),
                Position::NONE,
            )
            .into());
        }

        #[cfg(not(feature = "no_index"))]
        if _arr
            > self
                .limits
                .max_array_size
                .map_or(usize::MAX, NonZeroUsize::get)
        {
            return Err(EvalAltResult::ErrorDataTooLarge(
                "Size of array".to_string(),
                Position::NONE,
            )
            .into());
        }

        #[cfg(not(feature = "no_object"))]
        if _map
            > self
                .limits
                .max_map_size
                .map_or(usize::MAX, NonZeroUsize::get)
        {
            return Err(EvalAltResult::ErrorDataTooLarge(
                "Size of object map".to_string(),
                Position::NONE,
            )
            .into());
        }

        Ok(())
    }

    /// Check if the number of operations stay within limit.
    #[cfg(not(feature = "unchecked"))]
    pub(crate) fn inc_operations(
        &self,
        num_operations: &mut u64,
        pos: Position,
    ) -> Result<(), Box<EvalAltResult>> {
        *num_operations += 1;

        // Guard against too many operations
        if self.max_operations() > 0 && *num_operations > self.max_operations() {
            return Err(EvalAltResult::ErrorTooManyOperations(pos).into());
        }

        // Report progress - only in steps
        if let Some(ref progress) = self.progress {
            if let Some(token) = progress(*num_operations) {
                // Terminate script if progress returns a termination token
                return Err(EvalAltResult::ErrorTerminated(token, pos).into());
            }
        }

        Ok(())
    }

    /// Pretty-print a type name.
    ///
    /// If a type is registered via [`register_type_with_name`][Engine::register_type_with_name],
    /// the type name provided for the registration will be used.
    #[inline]
    #[must_use]
    pub fn map_type_name<'a>(&'a self, name: &'a str) -> &'a str {
        self.type_names
            .get(name)
            .map(|s| s.as_str())
            .unwrap_or_else(|| map_std_type_name(name))
    }

    /// Make a `Box<`[`EvalAltResult<ErrorMismatchDataType>`][EvalAltResult::ErrorMismatchDataType]`>`.
    #[inline]
    #[must_use]
    pub(crate) fn make_type_mismatch_err<T>(&self, typ: &str, pos: Position) -> Box<EvalAltResult> {
        EvalAltResult::ErrorMismatchDataType(
            self.map_type_name(type_name::<T>()).into(),
            typ.into(),
            pos,
        )
        .into()
    }
}
