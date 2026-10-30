//! The well-known types (`google.protobuf`), with their proto3 JSON
//! mapping: [`FieldMask`], [`Timestamp`], [`Duration`], [`Empty`] and the
//! others.
//!
//! They are [pbjson-types](https://docs.rs/pbjson-types)', except for
//! three whose JSON form there isn't quite proto3's: [`FieldMask`], written
//! as one string of camel case paths, [`Timestamp`], written in UTC with a
//! `Z`, and [`Any`], whose message's fields sit next to its `@type`.

pub use pbjson_types::*;

pub use crate::rpc::Any;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::de::Error as _;
use serde::ser::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A point in time (`google.protobuf.Timestamp`): seconds and nanoseconds
/// since 1970-01-01T00:00:00Z. In JSON it is RFC 3339 in UTC, such as
/// `"2026-10-30T12:00:00.500Z"`, with 0, 3, 6 or 9 fractional digits.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Message)]
pub struct Timestamp {
    /// Seconds since the Unix epoch.
    #[prost(int64, tag = "1")]
    pub seconds: i64,
    /// Nanoseconds of the second, from 0 to 999,999,999.
    #[prost(int32, tag = "2")]
    pub nanos: i32,
}

impl From<DateTime<Utc>> for Timestamp {
    fn from(t: DateTime<Utc>) -> Self {
        Self {
            seconds: t.timestamp(),
            nanos: i32::try_from(t.timestamp_subsec_nanos()).unwrap_or(999_999_999),
        }
    }
}

impl From<std::time::SystemTime> for Timestamp {
    fn from(t: std::time::SystemTime) -> Self {
        DateTime::<Utc>::from(t).into()
    }
}

impl TryFrom<Timestamp> for DateTime<Utc> {
    type Error = String;

    fn try_from(t: Timestamp) -> Result<Self, String> {
        u32::try_from(t.nanos)
            .ok()
            .and_then(|nanos| DateTime::from_timestamp(t.seconds, nanos))
            .ok_or_else(|| {
                format!(
                    "{}s {}ns is outside the times a timestamp holds",
                    t.seconds, t.nanos
                )
            })
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let t = DateTime::<Utc>::try_from(*self).map_err(S::Error::custom)?;
        serializer.serialize_str(&t.to_rfc3339_opts(SecondsFormat::AutoSi, true))
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let t = DateTime::parse_from_rfc3339(&text).map_err(D::Error::custom)?;
        Ok(t.with_timezone(&Utc).into())
    }
}

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
    fn a_timestamp_is_rfc_3339_in_utc_with_a_z() {
        for (seconds, nanos, json) in [
            (1_793_361_600, 0, r#""2026-10-30T12:00:00Z""#),
            (1_793_361_600, 500_000_000, r#""2026-10-30T12:00:00.500Z""#),
            (0, 1, r#""1970-01-01T00:00:00.000000001Z""#),
        ] {
            let t = Timestamp { seconds, nanos };
            assert_eq!(serde_json::to_string(&t).unwrap(), json);
            assert_eq!(serde_json::from_str::<Timestamp>(json).unwrap(), t);
        }
        let offset: Timestamp = serde_json::from_str(r#""2026-10-30T14:00:00+02:00""#).unwrap();
        assert_eq!(offset.seconds, 1_793_361_600);
        assert!(
            serde_json::to_string(&Timestamp {
                seconds: 0,
                nanos: -1
            })
            .is_err()
        );
    }

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
