//! `google.rpc.Status` and `google.protobuf.Any` in their JSON form: what
//! a failed operation carries, and the details of an error.

use serde::de::{DeserializeOwned, Error as _};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

/// The error a call or an operation failed with (`google.rpc.Status`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Status {
    /// The canonical code, such as 5 for `NOT_FOUND`.
    pub code: i32,
    /// What went wrong, for a person.
    pub message: String,
    /// Details for a program, such as the `google.rpc.ErrorInfo` naming the
    /// reason.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<Any>,
}

/// A message of any type, with the type it is (`google.protobuf.Any`), in
/// its JSON form: the message's fields next to `@type`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Any {
    type_url: String,
    fields: Map<String, Value>,
}

impl Any {
    /// An `Any` of type `type_url`, such as
    /// `type.googleapis.com/google.rpc.ErrorInfo`, holding fields.
    pub fn new(type_url: impl Into<String>, fields: Map<String, Value>) -> Self {
        Self {
            type_url: type_url.into(),
            fields,
        }
    }

    /// The type URL, such as `type.googleapis.com/google.rpc.ErrorInfo`.
    pub fn type_url(&self) -> &str {
        &self.type_url
    }

    /// The type's full name, such as `google.rpc.ErrorInfo`: the type URL
    /// after its last `/`.
    pub fn type_name(&self) -> &str {
        self.type_url.rsplit('/').next().unwrap_or_default()
    }

    /// The message's fields, by their JSON names.
    pub fn fields(&self) -> &Map<String, Value> {
        &self.fields
    }

    /// The message, read as `T`. A well-known type with a JSON form of its
    /// own, such as `google.protobuf.Empty`, is read from `value`.
    pub fn decode<T: DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        if self.type_name().starts_with("google.protobuf.")
            && let Some(value) = self.fields.get("value")
        {
            return T::deserialize(value);
        }
        T::deserialize(Value::Object(self.fields.clone()))
    }

    /// A string field, or `""` when the message has none by that name.
    pub(crate) fn string(&self, name: &str) -> &str {
        self.fields
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
    }
}

impl Serialize for Any {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.fields.len() + 1))?;
        map.serialize_entry("@type", &self.type_url)?;
        for (name, value) in &self.fields {
            map.serialize_entry(name, value)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Any {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut fields = Map::deserialize(deserializer)?;
        let type_url = match fields.remove("@type") {
            Some(Value::String(url)) => url,
            Some(_) => return Err(D::Error::custom("@type isn't a string")),
            None => String::new(),
        };
        Ok(Self { type_url, fields })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_any_keeps_its_type_apart_from_its_fields() {
        let any: Any = serde_json::from_str(
            r#"{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"NOT_FOUND"}"#,
        )
        .unwrap();
        assert_eq!(any.type_name(), "google.rpc.ErrorInfo");
        assert_eq!(any.string("reason"), "NOT_FOUND");
        assert_eq!(
            serde_json::to_string(&any).unwrap(),
            r#"{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"NOT_FOUND"}"#
        );
    }

    #[test]
    fn a_well_known_type_is_read_from_its_value() {
        let any: Any = serde_json::from_str(
            r#"{"@type":"type.googleapis.com/google.protobuf.Empty","value":{}}"#,
        )
        .unwrap();
        any.decode::<crate::protobuf::Empty>().unwrap();
        let bare: Any =
            serde_json::from_str(r#"{"@type":"type.googleapis.com/google.protobuf.Empty"}"#)
                .unwrap();
        bare.decode::<crate::protobuf::Empty>().unwrap();
    }
}
