//! Explicit JSON boundaries for selected API payloads and Sheets metadata.
//!
//! Raw capture distinguishes literal objects from serde_json's private number
//! protocol. Objects are constructed explicitly; primitives retain arbitrary
//! precision. This module does not change other Value consumers.

use std::fmt;

use serde::{de::Error, Deserialize, Deserializer};
use serde_json::{value::RawValue, Value};

const MAX_PAYLOAD_DEPTH: usize = 128;

/// Holds a payload decoded without the private-number object protocol.
pub(crate) struct Payload(pub(crate) Value);

impl<'de> Deserialize<'de> for Payload {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Box::<RawValue>::deserialize(deserializer)?;
        parse(raw.get(), 0).map(Self).map_err(D::Error::custom)
    }
}

/// Deserializes an optional selected payload, retaining null as None.
pub(crate) fn optional<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Value>, D::Error> {
    Option::<Payload>::deserialize(deserializer).map(|value| value.map(|payload| payload.0))
}

/// Deserializes a ragged cell matrix with a raw boundary for each cell.
pub(crate) fn cells<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<Vec<Value>>, D::Error> {
    Vec::<Vec<Payload>>::deserialize(deserializer).map(|rows| {
        rows.into_iter()
            .map(|row| row.into_iter().map(|cell| cell.0).collect())
            .collect()
    })
}

struct Object(Vec<(String, Box<RawValue>)>);

impl<'de> Deserialize<'de> for Object {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ObjectVisitor;
        impl<'de> serde::de::Visitor<'de> for ObjectVisitor {
            type Value = Object;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a JSON object")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Object, M::Error> {
                let mut entries = Vec::new();
                while let Some(entry) = map.next_entry()? {
                    entries.push(entry);
                }
                Ok(Object(entries))
            }
        }
        deserializer.deserialize_map(ObjectVisitor)
    }
}

fn parse(text: &str, depth: usize) -> serde_json::Result<Value> {
    // RawValue capture itself does not enforce the usual Value recursion limit.
    if depth >= MAX_PAYLOAD_DEPTH {
        return Err(serde_json::Error::custom(
            "JSON payload recursion limit exceeded",
        ));
    }
    match text.trim_start().as_bytes().first() {
        Some(b'{') => {
            let Object(entries) = serde_json::from_str(text)?;
            let mut object = serde_json::Map::new();
            for (key, raw) in entries {
                object.insert(key, parse(raw.get(), depth + 1)?);
            }
            Ok(Value::Object(object))
        }
        Some(b'[') => {
            let items: Vec<Box<RawValue>> = serde_json::from_str(text)?;
            items
                .into_iter()
                .map(|raw| parse(raw.get(), depth + 1))
                .collect::<serde_json::Result<Vec<_>>>()
                .map(Value::Array)
        }
        _ => serde_json::from_str(text),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Tests assert successful parsing of fixed fixtures.
mod tests {
    use super::*;

    #[test]
    fn private_number_payload_shapes_precision_and_order() {
        for (literal, expected) in crate::test_support::preserved_private_number_cases() {
            let payload: Payload = serde_json::from_str(&literal).unwrap();
            assert_eq!(payload.0, expected);
            let wire = serde_json::to_string(&payload.0).unwrap();
            assert_eq!(serde_json::from_str::<Payload>(&wire).unwrap().0, expected);
            assert_eq!(
                serde_json::from_slice::<Payload>(wire.as_bytes())
                    .unwrap()
                    .0,
                expected
            );
            assert_eq!(
                serde_json::from_value::<Payload>(expected.clone())
                    .unwrap()
                    .0,
                expected
            );
        }
        let text = r#"{"z":184467440737095516160,"a":0.12345678901234567890123456789}"#;
        let payload: Payload = serde_json::from_str(text).unwrap();
        assert_eq!(serde_json::to_string(&payload.0).unwrap(), text);
    }

    #[test]
    fn payload_rejects_malformed_and_deep_json() {
        assert!(serde_json::from_str::<Payload>("{bad}").is_err());
        // A value that starts like an object but is not one reaches the
        // object visitor's `expecting` message.
        let message = serde_json::from_value::<Object>(Value::Bool(true))
            .err()
            .unwrap()
            .to_string();
        assert!(message.contains("a JSON object"), "{message}");
        let deep = format!("{}null{}", "[".repeat(129), "]".repeat(129));
        assert!(serde_json::from_str::<Payload>(&deep).is_err());
    }
    #[test]
    fn selected_slots_keep_missing_null_and_scalar_behavior() {
        #[derive(Deserialize)]
        struct Slots {
            #[serde(default, deserialize_with = "optional")]
            payload: Option<Value>,
            #[serde(default, deserialize_with = "cells")]
            values: Vec<Vec<Value>>,
        }
        for text in ["{}", r#"{"payload":null}"#] {
            let slots: Slots = serde_json::from_str(text).unwrap();
            assert!(slots.payload.is_none());
            assert!(slots.values.is_empty());
        }
        let slots: Slots =
            serde_json::from_str(r#"{"payload":{},"values":[[null,true,42,"x"],[]]}"#).unwrap();
        assert_eq!(slots.payload, Some(serde_json::json!({})));
        assert_eq!(
            slots.values,
            vec![
                vec![
                    Value::Null,
                    Value::Bool(true),
                    serde_json::json!(42),
                    serde_json::json!("x")
                ],
                vec![]
            ]
        );
        assert!(serde_json::from_str::<Slots>(r#"{"values":[1]}"#).is_err());
        assert!(serde_json::from_str::<Slots>(r#"{"values":null}"#).is_err());
    }
}
