#![cfg(not(feature = "no_index"))]
#![allow(non_snake_case)]

use crate::plugin::*;
use crate::{
    def_package, Blob, Dynamic, EvalAltResult, ExclusiveRange, InclusiveRange, NativeCallContext,
    Position, INT,
};
#[cfg(feature = "no_std")]
use std::prelude::v1::*;
use std::{any::TypeId, mem};

#[cfg(not(feature = "no_float"))]
use crate::FLOAT;

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
        blob.resize(len, (value & 0x00ff) as u8);
        Ok(blob)
    }
    #[rhai_fn(name = "len", get = "len", pure)]
    pub fn len(blob: &mut Blob) -> INT {
        blob.len() as INT
    }
    #[rhai_fn(name = "push", name = "+=")]
    pub fn push(blob: &mut Blob, item: INT) {
        let item = (item & 0x00ff) as u8;
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
        let item = (item & 0x00ff) as u8;

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

        let item = (item & 0x00ff) as u8;
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
    #[rhai_fn(name = "splice")]
    pub fn splice_range(blob: &mut Blob, range: ExclusiveRange, replace: Blob) {
        let start = INT::max(range.start, 0);
        let end = INT::max(range.end, start);
        splice(blob, start, end - start, replace)
    }
    #[rhai_fn(name = "splice")]
    pub fn splice_range_inclusive(blob: &mut Blob, range: InclusiveRange, replace: Blob) {
        let start = INT::max(*range.start(), 0);
        let end = INT::max(*range.end(), start);
        splice(blob, start, end - start + 1, replace)
    }
    pub fn splice(blob: &mut Blob, start: INT, len: INT, replace: Blob) {
        if blob.is_empty() {
            *blob = replace;
            return;
        }
        let blob_len = blob.len();

        let start = if start < 0 {
            start
                .checked_abs()
                .map_or(0, |n| blob_len - (n as usize).min(blob_len))
        } else if start as usize >= blob_len {
            blob.extend(replace.into_iter());
            return;
        } else {
            start as usize
        };

        let len = if len < 0 {
            0
        } else if len as usize > blob_len - start {
            blob_len - start
        } else {
            len as usize
        };

        blob.splice(start..start + len, replace.into_iter());
    }
    #[rhai_fn(name = "extract")]
    pub fn extract_range(blob: &mut Blob, range: ExclusiveRange) -> Blob {
        let start = INT::max(range.start, 0);
        let end = INT::max(range.end, start);
        extract(blob, start, end - start)
    }
    #[rhai_fn(name = "extract")]
    pub fn extract_range_inclusive(blob: &mut Blob, range: InclusiveRange) -> Blob {
        let start = INT::max(*range.start(), 0);
        let end = INT::max(*range.end(), start);
        extract(blob, start, end - start + 1)
    }
    pub fn extract(blob: &mut Blob, start: INT, len: INT) -> Blob {
        if blob.is_empty() || len <= 0 {
            return Blob::new();
        }
        let blob_len = blob.len();

        let start = if start < 0 {
            start
                .checked_abs()
                .map_or(0, |n| blob_len - (n as usize).min(blob_len))
        } else if start as usize >= blob_len {
            return Blob::new();
        } else {
            start as usize
        };

        let len = if len as usize > blob_len - start {
            blob_len - start
        } else {
            len as usize
        };

        blob[start..start + len].to_vec()
    }
    #[rhai_fn(name = "extract")]
    pub fn extract_tail(blob: &mut Blob, start: INT) -> Blob {
        extract(blob, start, INT::MAX)
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
    #[rhai_fn(name = "drain")]
    pub fn drain_range(blob: &mut Blob, range: ExclusiveRange) -> Blob {
        let start = INT::max(range.start, 0);
        let end = INT::max(range.end, start);
        drain(blob, start, end - start)
    }
    #[rhai_fn(name = "drain")]
    pub fn drain_range_inclusive(blob: &mut Blob, range: InclusiveRange) -> Blob {
        let start = INT::max(*range.start(), 0);
        let end = INT::max(*range.end(), start);
        drain(blob, start, end - start + 1)
    }
    pub fn drain(blob: &mut Blob, start: INT, len: INT) -> Blob {
        if blob.is_empty() || len <= 0 {
            return Blob::new();
        }
        let blob_len = blob.len();

        let start = if start < 0 {
            start
                .checked_abs()
                .map_or(0, |n| blob_len - (n as usize).min(blob_len))
        } else if start as usize >= blob_len {
            return Blob::new();
        } else {
            start as usize
        };

        let len = if len as usize > blob_len - start {
            blob_len - start
        } else {
            len as usize
        };

        blob.drain(start..start + len).collect()
    }
    #[rhai_fn(name = "retain")]
    pub fn retain_range(blob: &mut Blob, range: ExclusiveRange) -> Blob {
        let start = INT::max(range.start, 0);
        let end = INT::max(range.end, start);
        retain(blob, start, end - start)
    }
    #[rhai_fn(name = "retain")]
    pub fn retain_range_inclusive(blob: &mut Blob, range: InclusiveRange) -> Blob {
        let start = INT::max(*range.start(), 0);
        let end = INT::max(*range.end(), start);
        retain(blob, start, end - start + 1)
    }
    pub fn retain(blob: &mut Blob, start: INT, len: INT) -> Blob {
        if blob.is_empty() || len <= 0 {
            return Blob::new();
        }
        let blob_len = blob.len();

        let start = if start < 0 {
            start
                .checked_abs()
                .map_or(0, |n| blob_len - (n as usize).min(blob_len))
        } else if start as usize >= blob_len {
            return mem::take(blob);
        } else {
            start as usize
        };

        let len = if len as usize > blob_len - start {
            blob_len - start
        } else {
            len as usize
        };

        let mut drained: Blob = blob.drain(..start).collect();
        drained.extend(blob.drain(len..));

        drained
    }
    pub fn contains(blob: &mut Blob, value: Dynamic) -> bool {
        if blob.is_empty() {
            return false;
        }

        let value = match value.as_int() {
            Ok(value) => value as u8,
            _ => return false,
        };

        blob.contains(&value)
    }
    #[rhai_fn(name = "==", pure)]
    pub fn equals(blob1: &mut Blob, blob2: Blob) -> bool {
        &*blob1 == &blob2
    }
    #[rhai_fn(name = "!=", pure)]
    pub fn not_equals(blob1: &mut Blob, blob2: Blob) -> bool {
        &*blob1 != &blob2
    }

    #[inline]
    fn parse_int(blob: &mut Blob, start: INT, len: INT, is_le: bool) -> INT {
        if blob.is_empty() || len <= 0 {
            return 0;
        }
        let blob_len = blob.len();

        let start = if start < 0 {
            start
                .checked_abs()
                .map_or(0, |n| blob_len - (n as usize).min(blob_len))
        } else if start as usize >= blob_len {
            return 0;
        } else {
            start as usize
        };

        let len = if len as usize > blob_len - start {
            blob_len - start
        } else {
            len as usize
        };

        const INT_BYTES: usize = mem::size_of::<INT>();

        let len = usize::min(len, INT_BYTES);

        let mut buf = [0_u8; INT_BYTES];

        buf[..len].copy_from_slice(&blob[start..][..len]);

        if is_le {
            INT::from_le_bytes(buf)
        } else {
            INT::from_be_bytes(buf)
        }
    }

    #[rhai_fn(name = "parse_le_int")]
    pub fn parse_le_int_range(blob: &mut Blob, range: ExclusiveRange) -> INT {
        let start = INT::max(range.start, 0);
        let end = INT::max(range.end, start);
        parse_le_int(blob, start, end - start)
    }
    #[rhai_fn(name = "parse_le_int")]
    pub fn parse_le_int_range_inclusive(blob: &mut Blob, range: InclusiveRange) -> INT {
        let start = INT::max(*range.start(), 0);
        let end = INT::max(*range.end(), start);
        parse_le_int(blob, start, end - start + 1)
    }
    pub fn parse_le_int(blob: &mut Blob, start: INT, len: INT) -> INT {
        parse_int(blob, start, len, true)
    }
    #[rhai_fn(name = "parse_be_int")]
    pub fn parse_be_int_range(blob: &mut Blob, range: ExclusiveRange) -> INT {
        let start = INT::max(range.start, 0);
        let end = INT::max(range.end, start);
        parse_be_int(blob, start, end - start)
    }
    #[rhai_fn(name = "parse_be_int")]
    pub fn parse_be_int_range_inclusive(blob: &mut Blob, range: InclusiveRange) -> INT {
        let start = INT::max(*range.start(), 0);
        let end = INT::max(*range.end(), start);
        parse_be_int(blob, start, end - start + 1)
    }
    pub fn parse_be_int(blob: &mut Blob, start: INT, len: INT) -> INT {
        parse_int(blob, start, len, false)
    }

    #[inline]
    fn write_int(blob: &mut Blob, start: INT, len: INT, value: INT, is_le: bool) {
        if blob.is_empty() || len <= 0 {
            return;
        }
        let blob_len = blob.len();

        let start = if start < 0 {
            start
                .checked_abs()
                .map_or(0, |n| blob_len - (n as usize).min(blob_len))
        } else if start as usize >= blob_len {
            return;
        } else {
            start as usize
        };

        let len = if len as usize > blob_len - start {
            blob_len - start
        } else {
            len as usize
        };

        const INT_BYTES: usize = mem::size_of::<INT>();

        let len = usize::min(len, INT_BYTES);

        let mut buf = [0_u8; INT_BYTES];

        buf.copy_from_slice(&if is_le {
            value.to_le_bytes()
        } else {
            value.to_be_bytes()
        });

        blob[start..][..len].copy_from_slice(&buf[..len]);
    }
    #[rhai_fn(name = "write_le_int")]
    pub fn write_le_int_range(blob: &mut Blob, range: ExclusiveRange, value: INT) {
        let start = INT::max(range.start, 0);
        let end = INT::max(range.end, start);
        write_le_int(blob, start, end - start, value)
    }
    #[rhai_fn(name = "write_le_int")]
    pub fn write_le_int_range_inclusive(blob: &mut Blob, range: InclusiveRange, value: INT) {
        let start = INT::max(*range.start(), 0);
        let end = INT::max(*range.end(), start);
        write_le_int(blob, start, end - start + 1, value)
    }
    #[rhai_fn(name = "write_le")]
    pub fn write_le_int(blob: &mut Blob, start: INT, len: INT, value: INT) {
        write_int(blob, start, len, value, true)
    }
    #[rhai_fn(name = "write_be")]
    pub fn write_be_int_range(blob: &mut Blob, range: ExclusiveRange, value: INT) {
        let start = INT::max(range.start, 0);
        let end = INT::max(range.end, start);
        write_be_int(blob, start, end - start, value)
    }
    #[rhai_fn(name = "write_be")]
    pub fn write_be_int_range_inclusive(blob: &mut Blob, range: InclusiveRange, value: INT) {
        let start = INT::max(*range.start(), 0);
        let end = INT::max(*range.end(), start);
        write_be_int(blob, start, end - start + 1, value)
    }
    #[rhai_fn(name = "write_be")]
    pub fn write_be_int(blob: &mut Blob, start: INT, len: INT, value: INT) {
        write_int(blob, start, len, value, false)
    }

    #[cfg(not(feature = "no_float"))]
    #[inline]
    fn parse_float(blob: &mut Blob, start: INT, len: INT, is_le: bool) -> FLOAT {
        if blob.is_empty() || len <= 0 {
            return 0.0;
        }
        let blob_len = blob.len();

        let start = if start < 0 {
            start
                .checked_abs()
                .map_or(0, |n| blob_len - (n as usize).min(blob_len))
        } else if start as usize >= blob_len {
            return 0.0;
        } else {
            start as usize
        };

        let len = if len as usize > blob_len - start {
            blob_len - start
        } else {
            len as usize
        };

        const FLOAT_BYTES: usize = mem::size_of::<FLOAT>();

        let len = usize::min(len, FLOAT_BYTES);

        let mut buf = [0_u8; FLOAT_BYTES];

        buf[..len].copy_from_slice(&blob[start..][..len]);

        if is_le {
            FLOAT::from_le_bytes(buf)
        } else {
            FLOAT::from_be_bytes(buf)
        }
    }

    #[cfg(not(feature = "no_float"))]
    #[rhai_fn(name = "parse_le_float")]
    pub fn parse_le_float_range(blob: &mut Blob, range: ExclusiveRange) -> FLOAT {
        let start = INT::max(range.start, 0);
        let end = INT::max(range.end, start);
        parse_le_float(blob, start, end - start)
    }
    #[cfg(not(feature = "no_float"))]
    #[rhai_fn(name = "parse_le_float")]
    pub fn parse_le_float_range_inclusive(blob: &mut Blob, range: InclusiveRange) -> FLOAT {
        let start = INT::max(*range.start(), 0);
        let end = INT::max(*range.end(), start);
        parse_le_float(blob, start, end - start + 1)
    }
    #[cfg(not(feature = "no_float"))]
    pub fn parse_le_float(blob: &mut Blob, start: INT, len: INT) -> FLOAT {
        parse_float(blob, start, len, true)
    }
    #[cfg(not(feature = "no_float"))]
    #[rhai_fn(name = "parse_be_float")]
    pub fn parse_be_float_range(blob: &mut Blob, range: ExclusiveRange) -> FLOAT {
        let start = INT::max(range.start, 0);
        let end = INT::max(range.end, start);
        parse_be_float(blob, start, end - start)
    }
    #[cfg(not(feature = "no_float"))]
    #[rhai_fn(name = "parse_be_float")]
    pub fn parse_be_float_range_inclusive(blob: &mut Blob, range: InclusiveRange) -> FLOAT {
        let start = INT::max(*range.start(), 0);
        let end = INT::max(*range.end(), start);
        parse_be_float(blob, start, end - start + 1)
    }
    #[cfg(not(feature = "no_float"))]
    pub fn parse_be_float(blob: &mut Blob, start: INT, len: INT) -> FLOAT {
        parse_float(blob, start, len, false)
    }

    #[cfg(not(feature = "no_float"))]
    #[inline]
    fn write_float(blob: &mut Blob, start: INT, len: INT, value: FLOAT, is_le: bool) {
        if blob.is_empty() || len <= 0 {
            return;
        }
        let blob_len = blob.len();

        let start = if start < 0 {
            start
                .checked_abs()
                .map_or(0, |n| blob_len - (n as usize).min(blob_len))
        } else if start as usize >= blob_len {
            return;
        } else {
            start as usize
        };

        let len = if len as usize > blob_len - start {
            blob_len - start
        } else {
            len as usize
        };

        const FLOAT_BYTES: usize = mem::size_of::<FLOAT>();

        let len = usize::min(len, FLOAT_BYTES);

        let mut buf = [0_u8; FLOAT_BYTES];

        buf.copy_from_slice(&if is_le {
            value.to_le_bytes()
        } else {
            value.to_be_bytes()
        });

        blob[start..][..len].copy_from_slice(&buf[..len]);
    }
    #[cfg(not(feature = "no_float"))]
    #[rhai_fn(name = "write_le_float")]
    pub fn write_le_float_range(blob: &mut Blob, range: ExclusiveRange, value: FLOAT) {
        let start = INT::max(range.start, 0);
        let end = INT::max(range.end, start);
        write_le_float(blob, start, end - start, value)
    }
    #[cfg(not(feature = "no_float"))]
    #[rhai_fn(name = "write_le_float")]
    pub fn write_le_float_range_inclusive(blob: &mut Blob, range: InclusiveRange, value: FLOAT) {
        let start = INT::max(*range.start(), 0);
        let end = INT::max(*range.end(), start);
        write_le_float(blob, start, end - start + 1, value)
    }
    #[cfg(not(feature = "no_float"))]
    #[rhai_fn(name = "write_le")]
    pub fn write_le_float(blob: &mut Blob, start: INT, len: INT, value: FLOAT) {
        write_float(blob, start, len, value, true)
    }
    #[cfg(not(feature = "no_float"))]
    #[rhai_fn(name = "write_be")]
    pub fn write_be_float_range(blob: &mut Blob, range: ExclusiveRange, value: FLOAT) {
        let start = INT::max(range.start, 0);
        let end = INT::max(range.end, start);
        write_be_float(blob, start, end - start, value)
    }
    #[cfg(not(feature = "no_float"))]
    #[rhai_fn(name = "write_be")]
    pub fn write_be_float_range_inclusive(blob: &mut Blob, range: InclusiveRange, value: FLOAT) {
        let start = INT::max(*range.start(), 0);
        let end = INT::max(*range.end(), start);
        write_be_float(blob, start, end - start + 1, value)
    }
    #[cfg(not(feature = "no_float"))]
    #[rhai_fn(name = "write_be")]
    pub fn write_be_float(blob: &mut Blob, start: INT, len: INT, value: FLOAT) {
        write_float(blob, start, len, value, false)
    }
}
