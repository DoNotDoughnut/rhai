#![cfg(not(feature = "no_index"))]
#![allow(non_snake_case)]

use crate::plugin::*;
use crate::{def_package, Blob, Dynamic, EvalAltResult, NativeCallContext, Position, INT};
#[cfg(feature = "no_std")]
use std::prelude::v1::*;
use std::{any::TypeId, mem};

def_package!(crate:BasicBlobPackage:"Basic BLOB utilities.", lib, {
    lib.standard = true;

    combine_with_exported_module!(lib, "blob", blob_functions);

    // Register blob iterator
    lib.set_iterable::<Blob>();
});

#[export_module]
mod blob_functions {
    pub fn blob() -> Blob {
        Blob::new()
    }
    #[rhai_fn(name = "blob", return_raw)]
    pub fn blob_with_capacity(
        ctx: NativeCallContext,
        len: INT,
    ) -> Result<Blob, Box<EvalAltResult>> {
        blob_with_capacity_and_value(ctx, len, 0)
    }
    #[rhai_fn(name = "blob", return_raw)]
    pub fn blob_with_capacity_and_value(
        ctx: NativeCallContext,
        len: INT,
        value: INT,
    ) -> Result<Blob, Box<EvalAltResult>> {
        let len = if len < 0 { 0 } else { len as usize };
        let _ctx = ctx;

        // Check if blob will be over max size limit
        #[cfg(not(feature = "unchecked"))]
        if _ctx.engine().max_array_size() > 0 && len > _ctx.engine().max_array_size() {
            return Err(EvalAltResult::ErrorDataTooLarge(
                "Size of BLOB".to_string(),
                Position::NONE,
            )
            .into());
        }

        let mut blob = Blob::new();
        blob.resize(len, (value & 0x000f) as u8);
        Ok(blob)
    }
    #[rhai_fn(name = "len", get = "len", pure)]
    pub fn len(blob: &mut Blob) -> INT {
        blob.len() as INT
    }
    #[rhai_fn(name = "push", name = "+=")]
    pub fn push(blob: &mut Blob, item: INT) {
        let item = (item & 0x000f) as u8;
        blob.push(item);
    }
    #[rhai_fn(name = "append", name = "+=")]
    pub fn append(blob: &mut Blob, y: Blob) {
        if !y.is_empty() {
            if blob.is_empty() {
                *blob = y;
            } else {
                blob.extend(y);
            }
        }
    }
    #[rhai_fn(name = "+")]
    pub fn concat(mut blob: Blob, y: Blob) -> Blob {
        if !y.is_empty() {
            if blob.is_empty() {
                blob = y;
            } else {
                blob.extend(y);
            }
        }
        blob
    }
    pub fn insert(blob: &mut Blob, position: INT, item: INT) {
        let item = (item & 0x000f) as u8;

        if blob.is_empty() {
            blob.push(item);
        } else if position < 0 {
            if let Some(n) = position.checked_abs() {
                if n as usize > blob.len() {
                    blob.insert(0, item);
                } else {
                    blob.insert(blob.len() - n as usize, item);
                }
            } else {
                blob.insert(0, item);
            }
        } else if (position as usize) >= blob.len() {
            blob.push(item);
        } else {
            blob.insert(position as usize, item);
        }
    }
    #[rhai_fn(return_raw)]
    pub fn pad(
        ctx: NativeCallContext,
        blob: &mut Blob,
        len: INT,
        item: INT,
    ) -> Result<(), Box<EvalAltResult>> {
        if len <= 0 {
            return Ok(());
        }

        let item = (item & 0x000f) as u8;
        let _ctx = ctx;

        // Check if blob will be over max size limit
        #[cfg(not(feature = "unchecked"))]
        if _ctx.engine().max_array_size() > 0 && (len as usize) > _ctx.engine().max_array_size() {
            return Err(EvalAltResult::ErrorDataTooLarge(
                "Size of BLOB".to_string(),
                Position::NONE,
            )
            .into());
        }

        if len as usize > blob.len() {
            blob.resize(len as usize, item);
        }

        Ok(())
    }
    pub fn pop(blob: &mut Blob) -> INT {
        if blob.is_empty() {
            0
        } else {
            blob.pop().map_or_else(|| 0, |v| v as INT)
        }
    }
    pub fn shift(blob: &mut Blob) -> INT {
        if blob.is_empty() {
            0
        } else {
            blob.remove(0) as INT
        }
    }
    pub fn remove(blob: &mut Blob, len: INT) -> INT {
        if len < 0 || (len as usize) >= blob.len() {
            0
        } else {
            blob.remove(len as usize) as INT
        }
    }
    pub fn clear(blob: &mut Blob) {
        if !blob.is_empty() {
            blob.clear();
        }
    }
    pub fn truncate(blob: &mut Blob, len: INT) {
        if !blob.is_empty() {
            if len >= 0 {
                blob.truncate(len as usize);
            } else {
                blob.clear();
            }
        }
    }
    pub fn chop(blob: &mut Blob, len: INT) {
        if !blob.is_empty() && len as usize >= blob.len() {
            if len >= 0 {
                blob.drain(0..blob.len() - len as usize);
            } else {
                blob.clear();
            }
        }
    }
    pub fn reverse(blob: &mut Blob) {
        if !blob.is_empty() {
            blob.reverse();
        }
    }
    pub fn splice(blob: &mut Blob, start: INT, len: INT, replace: Blob) {
        if blob.is_empty() {
            *blob = replace;
            return;
        }

        let start = if start < 0 {
            let arr_len = blob.len();
            start
                .checked_abs()
                .map_or(0, |n| arr_len - (n as usize).min(arr_len))
        } else if start as usize >= blob.len() {
            blob.extend(replace.into_iter());
            return;
        } else {
            start as usize
        };

        let len = if len < 0 {
            0
        } else if len as usize > blob.len() - start {
            blob.len() - start
        } else {
            len as usize
        };

        blob.splice(start..start + len, replace.into_iter());
    }
    pub fn extract(blob: &mut Blob, start: INT, len: INT) -> Blob {
        if blob.is_empty() || len <= 0 {
            return Blob::new();
        }

        let start = if start < 0 {
            let arr_len = blob.len();
            start
                .checked_abs()
                .map_or(0, |n| arr_len - (n as usize).min(arr_len))
        } else if start as usize >= blob.len() {
            return Blob::new();
        } else {
            start as usize
        };

        let len = if len <= 0 {
            0
        } else if len as usize > blob.len() - start {
            blob.len() - start
        } else {
            len as usize
        };

        if len == 0 {
            Blob::new()
        } else {
            blob[start..start + len].to_vec()
        }
    }
    #[rhai_fn(name = "extract")]
    pub fn extract_tail(blob: &mut Blob, start: INT) -> Blob {
        if blob.is_empty() {
            return Blob::new();
        }

        let start = if start < 0 {
            let arr_len = blob.len();
            start
                .checked_abs()
                .map_or(0, |n| arr_len - (n as usize).min(arr_len))
        } else if start as usize >= blob.len() {
            return Blob::new();
        } else {
            start as usize
        };

        blob[start..].to_vec()
    }
    #[rhai_fn(name = "split")]
    pub fn split_at(blob: &mut Blob, start: INT) -> Blob {
        if blob.is_empty() {
            Blob::new()
        } else if start < 0 {
            if let Some(n) = start.checked_abs() {
                if n as usize > blob.len() {
                    mem::take(blob)
                } else {
                    let mut result = Blob::new();
                    result.extend(blob.drain(blob.len() - n as usize..));
                    result
                }
            } else {
                mem::take(blob)
            }
        } else if start as usize >= blob.len() {
            Blob::new()
        } else {
            let mut result = Blob::new();
            result.extend(blob.drain(start as usize..));
            result
        }
    }
    pub fn drain(blob: &mut Blob, start: INT, len: INT) -> Blob {
        if blob.is_empty() || len <= 0 {
            return Blob::new();
        }

        let start = if start < 0 {
            let arr_len = blob.len();
            start
                .checked_abs()
                .map_or(0, |n| arr_len - (n as usize).min(arr_len))
        } else if start as usize >= blob.len() {
            return Blob::new();
        } else {
            start as usize
        };

        let len = if len <= 0 {
            0
        } else if len as usize > blob.len() - start {
            blob.len() - start
        } else {
            len as usize
        };

        blob.drain(start..start + len).collect()
    }
    pub fn retain(blob: &mut Blob, start: INT, len: INT) -> Blob {
        if blob.is_empty() || len <= 0 {
            return Blob::new();
        }

        let start = if start < 0 {
            let arr_len = blob.len();
            start
                .checked_abs()
                .map_or(0, |n| arr_len - (n as usize).min(arr_len))
        } else if start as usize >= blob.len() {
            return mem::take(blob);
        } else {
            start as usize
        };

        let len = if len < 0 {
            0
        } else if len as usize > blob.len() - start {
            blob.len() - start
        } else {
            len as usize
        };

        let mut drained: Blob = blob.drain(..start).collect();
        drained.extend(blob.drain(len..));

        drained
    }
    #[rhai_fn(name = "==", pure)]
    pub fn equals(blob1: &mut Blob, blob2: Blob) -> bool {
        if blob1.len() != blob2.len() {
            false
        } else if blob1.is_empty() {
            true
        } else {
            blob1.iter().zip(blob2.iter()).all(|(&v1, &v2)| v1 == v2)
        }
    }
    #[rhai_fn(name = "!=", pure)]
    pub fn not_equals(blob1: &mut Blob, blob2: Blob) -> bool {
        !equals(blob1, blob2)
    }
}
