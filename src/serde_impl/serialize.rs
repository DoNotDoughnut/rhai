//! Implementations of [`serde::Serialize`].

use crate::dynamic::Union;
use crate::stdlib::string::ToString;
use crate::{Dynamic, ImmutableString};
use serde::ser::{Serialize, SerializeMap, Serializer};

impl Serialize for Dynamic {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        match &self.0 {
            Union::Unit(_, _) => ser.serialize_unit(),
            Union::Bool(x, _) => ser.serialize_bool(*x),
            Union::Str(s, _) => ser.serialize_str(s.as_str()),
            Union::Char(c, _) => ser.serialize_str(&c.to_string()),
            #[cfg(not(feature = "only_i32"))]
            Union::Int(x, _) => ser.serialize_i64(*x),
            #[cfg(feature = "only_i32")]
            Union::Int(x, _) => ser.serialize_i32(*x),
            #[cfg(not(feature = "no_float"))]
            #[cfg(not(feature = "f32_float"))]
            Union::Float(x, _) => ser.serialize_f64(**x),
            #[cfg(not(feature = "no_float"))]
            #[cfg(feature = "f32_float")]
            Union::Float(x, _) => ser.serialize_f32(*x),
            #[cfg(not(feature = "no_index"))]
            Union::Array(a, _) => (**a).serialize(ser),
            #[cfg(not(feature = "no_object"))]
            Union::Map(m, _) => {
                let mut map = ser.serialize_map(Some(m.len()))?;
                for (k, v) in m.iter() {
                    map.serialize_entry(k, v)?;
                }
                map.end()
            }
            Union::FnPtr(f, _) => ser.serialize_str(f.fn_name()),
            #[cfg(not(feature = "no_std"))]
            Union::TimeStamp(_, _) => unimplemented!("serialization of timestamp is not supported"),

            Union::Variant(v, _) => ser.serialize_str((***v).type_name()),

            #[cfg(not(feature = "no_closure"))]
            #[cfg(not(feature = "sync"))]
            Union::Shared(cell, _) => cell.borrow().serialize(ser),
            #[cfg(not(feature = "no_closure"))]
            #[cfg(feature = "sync")]
            Union::Shared(cell, _) => cell.read().unwrap().serialize(ser),
        }
    }
}

impl Serialize for ImmutableString {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(self.as_str())
    }
}
