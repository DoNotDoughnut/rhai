//! _(serde)_ Serialization and deserialization support for [`serde`](https://crates.io/crates/serde).
//! Exported under the `serde` feature only.

mod de;
mod deserialize;
mod metadata;
mod ser;
mod serialize;
mod str;

pub use de::from_dynamic;
pub use ser::to_dynamic;
