//! The well-known types (`google.protobuf`), with their proto3 JSON
//! mapping: [`FieldMask`], [`Timestamp`], [`Duration`], [`Empty`] and the
//! others.
//!
//! They are [pbjson-types](https://docs.rs/pbjson-types)', except for two
//! whose JSON form there isn't proto3's: [`FieldMask`], written as one
//! string of camel case paths, and [`Any`], whose message's fields sit next
//! to its `@type`.

pub use pbjson_types::*;

pub use crate::rpc::Any;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A set of field paths (`google.protobuf.FieldMask`), such as the fields
/// an update changes. The paths are the proto field names, such as
/// `size_gib`; in JSON they are one string of camel case paths, such as
/// `"sizeGib,tags"`.
#[derive(Clone, PartialEq, Eq, Hash, ::prost::Message)]
pub struct FieldMask {
    /// The field paths, such as `size_gib` or `volume.size_gib`.
    #[prost(string, repeated, tag = "1")]
    pub paths: Vec<String>,
}

impl FieldMask {
    /// A mask of `paths`.
    pub fn new<I, S>(paths: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            paths: paths.into_iter().map(Into::into).collect(),
        }
    }
}

impl Serialize for FieldMask {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut joined = String::new();
        for (i, path) in self.paths.iter().enumerate() {
            if i > 0 {
                joined.push(',');
            }
            for (j, part) in path.split('.').enumerate() {
                if j > 0 {
                    joined.push('.');
                }
                joined.push_str(&to_lower_camel(part));
            }
        }
        serializer.serialize_str(&joined)
    }
}

impl<'de> Deserialize<'de> for FieldMask {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let joined = String::deserialize(deserializer)?;
        let mut paths = Vec::new();
        for path in joined.split(',').filter(|p| !p.is_empty()) {
            if path.contains('_') {
                return Err(D::Error::custom(format!(
                    "the field mask path {path:?} isn't in camel case"
                )));
            }
            let parts: Vec<String> = path.split('.').map(to_snake).collect();
            paths.push(parts.join("."));
        }
        Ok(Self { paths })
    }
}

fn to_lower_camel(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut upper = false;
    for c in name.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

fn to_snake(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for c in name.chars() {
        if c.is_ascii_uppercase() {
            out.push('_');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_mask_is_one_string_of_camel_case_paths() {
        let mask = FieldMask::new(["size_gib", "tags", "volume.storage_class"]);
        let json = serde_json::to_string(&mask).unwrap();
        assert_eq!(json, r#""sizeGib,tags,volume.storageClass""#);
        assert_eq!(serde_json::from_str::<FieldMask>(&json).unwrap(), mask);
        assert_eq!(
            serde_json::from_str::<FieldMask>(r#""""#).unwrap(),
            FieldMask::default()
        );
        assert!(serde_json::from_str::<FieldMask>(r#""size_gib""#).is_err());
    }
}
