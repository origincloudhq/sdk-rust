//! The common types (`google.type`) the API uses, with their proto3 JSON
//! mapping: [`Date`].

use chrono::{Datelike, NaiveDate};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A calendar day (`google.type.Date`), without a time of day or a time
/// zone: a year from 1 to 9999, a month from 1 to 12 and a day from 1 to 31.
/// A partial date leaves some at 0: the year, for a day of every year; the
/// day, for a month; the month and the day, for a year. In JSON it is an
/// object, such as `{"year":2026,"month":12,"day":15}`, without the fields
/// that are 0.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Message)]
pub struct Date {
    /// The year, from 1 to 9999, or 0 for a date without one.
    #[prost(int32, tag = "1")]
    pub year: i32,
    /// The month, from 1 to 12, or 0 for a year without one.
    #[prost(int32, tag = "2")]
    pub month: i32,
    /// The day of the month, from 1 to 31, or 0 for a year, or a year and
    /// a month, without one.
    #[prost(int32, tag = "3")]
    pub day: i32,
}

impl Date {
    /// The date `year`-`month`-`day`.
    pub fn new(year: i32, month: i32, day: i32) -> Self {
        Self { year, month, day }
    }
}

impl From<NaiveDate> for Date {
    fn from(d: NaiveDate) -> Self {
        Self {
            year: d.year(),
            month: i32::try_from(d.month()).unwrap_or_default(),
            day: i32::try_from(d.day()).unwrap_or_default(),
        }
    }
}

impl TryFrom<Date> for NaiveDate {
    type Error = String;

    /// The day, for a full date; a partial or impossible one fails.
    fn try_from(d: Date) -> Result<Self, String> {
        u32::try_from(d.month)
            .ok()
            .zip(u32::try_from(d.day).ok())
            .filter(|_| d.year != 0)
            .and_then(|(month, day)| NaiveDate::from_ymd_opt(d.year, month, day))
            .ok_or_else(|| {
                format!(
                    "{:04}-{:02}-{:02} isn't a full date",
                    d.year, d.month, d.day
                )
            })
    }
}

impl Serialize for Date {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let fields = [
            ("year", self.year),
            ("month", self.month),
            ("day", self.day),
        ];
        let mut map = serializer.serialize_map(None)?;
        for (name, value) in fields.into_iter().filter(|(_, v)| *v != 0) {
            map.serialize_entry(name, &value)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Date {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// The fields as proto3 JSON allows them: absent or null for 0,
        /// a number, or a number in a string; other fields are ignored.
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Fields {
            year: Option<Int32>,
            month: Option<Int32>,
            day: Option<Int32>,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Int32 {
            Number(i32),
            Text(String),
        }

        fn value<E: serde::de::Error>(name: &str, field: Option<Int32>) -> Result<i32, E> {
            match field {
                None => Ok(0),
                Some(Int32::Number(n)) => Ok(n),
                Some(Int32::Text(s)) => s
                    .parse()
                    .map_err(|_| E::custom(format!("the date's {name} {s:?} isn't an int32"))),
            }
        }

        let fields = Fields::deserialize(deserializer)?;
        Ok(Self {
            year: value::<D::Error>("year", fields.year)?,
            month: value::<D::Error>("month", fields.month)?,
            day: value::<D::Error>("day", fields.day)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_date_is_an_object_without_its_zero_fields() {
        for (date, json) in [
            (
                Date::new(2026, 12, 15),
                r#"{"year":2026,"month":12,"day":15}"#,
            ),
            (Date::new(0, 2, 29), r#"{"month":2,"day":29}"#),
            (Date::new(2027, 3, 0), r#"{"year":2027,"month":3}"#),
            (Date::default(), "{}"),
        ] {
            assert_eq!(serde_json::to_string(&date).unwrap(), json);
            assert_eq!(serde_json::from_str::<Date>(json).unwrap(), date);
        }
    }

    #[test]
    fn a_date_reads_every_form_proto3_json_allows() {
        let date: Date =
            serde_json::from_str(r#"{"year":"2026","month":12,"day":null,"era":"CE"}"#).unwrap();
        assert_eq!(date, Date::new(2026, 12, 0));
        assert!(serde_json::from_str::<Date>(r#"{"year":"MMXXVI"}"#).is_err());
        assert!(serde_json::from_str::<Date>(r#""2026-12-15""#).is_err());
    }

    #[test]
    fn a_full_date_is_a_naive_date() {
        let day = NaiveDate::from_ymd_opt(2026, 12, 15).unwrap();
        assert_eq!(Date::from(day), Date::new(2026, 12, 15));
        assert_eq!(NaiveDate::try_from(Date::new(2026, 12, 15)), Ok(day));
        for partial in [
            Date::new(0, 12, 15),
            Date::new(2026, 0, 0),
            Date::new(2026, 12, 0),
            Date::new(2026, 2, 30),
            Date::new(2026, -1, 1),
        ] {
            assert!(NaiveDate::try_from(partial).is_err(), "{partial:?}");
        }
    }
}
