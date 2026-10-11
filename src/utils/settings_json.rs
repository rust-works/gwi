//! JSON at the settings read/merge/write boundary.
//!
//! Objects are parsed explicitly as ordered source slices, never through Value's
//! arbitrary-precision number protocol. Only scalar leaves use Value. Equality
//! is structural (object order and whitespace do not create import conflicts),
//! while serialization retains member order and numeric precision.

use indexmap::IndexMap;
use serde::{de::IgnoredAny, Serialize, Serializer};
use serde_json::Value;

/// An ordered settings object.
pub(crate) type Object = IndexMap<String, SettingsJson>;

/// Semantic JSON for settings, isolated from arbitrary Value deserialization.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SettingsJson {
    /// Object members in input order.
    Object(Object),
    /// Array elements in input order.
    Array(Vec<Self>),
    /// A null, boolean, string or exact number; never an object or array.
    Scalar(Value),
}

impl SettingsJson {
    /// Parse validated source slices without Value's object/number protocol.
    pub(crate) fn parse(input: &str) -> serde_json::Result<Self> {
        // IgnoredAny checks the whole JSON grammar without interpreting private
        // map keys. It also rejects trailing input before any tree is built.
        serde_json::from_str::<IgnoredAny>(input)?;
        Self::from_slice(input.trim(), 0)
    }

    fn from_slice(input: &str, depth: usize) -> serde_json::Result<Self> {
        // IgnoredAny skips nested values iteratively, so bound our own traversal.
        if depth >= 128 {
            return Err(serde::de::Error::custom(
                "Settings JSON nesting limit exceeded",
            ));
        }
        match input.as_bytes().first() {
            Some(b'{') => {
                let mut members = Object::new();
                let mut remaining = &input[1..input.len() - 1];
                while !remaining.trim().is_empty() {
                    let (key, rest) = take_value(remaining)?;
                    let key: String = serde_json::from_str(key)?;
                    let rest = rest.trim_start().strip_prefix(':').ok_or_else(|| {
                        <serde_json::Error as serde::de::Error>::custom(
                            "Missing settings member colon",
                        )
                    })?;
                    let (value, rest) = take_value(rest)?;
                    members.insert(key, Self::from_slice(value.trim(), depth + 1)?);
                    remaining = after_member(rest)?;
                }
                Ok(Self::Object(members))
            }
            Some(b'[') => {
                let mut elements = Vec::new();
                let mut remaining = &input[1..input.len() - 1];
                while !remaining.trim().is_empty() {
                    let (value, rest) = take_value(remaining)?;
                    elements.push(Self::from_slice(value.trim(), depth + 1)?);
                    remaining = after_member(rest)?;
                }
                Ok(Self::Array(elements))
            }
            _ => serde_json::from_str(input).map(Self::Scalar),
        }
    }

    pub(crate) fn as_object(&self) -> Option<&Object> {
        match self {
            Self::Object(map) => Some(map),
            _ => None,
        }
    }

    pub(crate) fn as_object_mut(&mut self) -> Option<&mut Object> {
        match self {
            Self::Object(map) => Some(map),
            _ => None,
        }
    }

    pub(crate) fn is_object(&self) -> bool {
        matches!(self, Self::Object(_))
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Self::Scalar(value) => value.as_str(),
            _ => None,
        }
    }

    pub(crate) fn get(&self, key: &str) -> Option<&Self> {
        self.as_object()?.get(key)
    }

    pub(crate) fn get_mut(&mut self, key: &str) -> Option<&mut Self> {
        self.as_object_mut()?.get_mut(key)
    }
}

/// Locate a syntactically valid value's exact span using serde_json's scanner.
fn take_value(input: &str) -> serde_json::Result<(&str, &str)> {
    let mut stream = serde_json::Deserializer::from_str(input).into_iter::<IgnoredAny>();
    stream.next().ok_or_else(|| {
        <serde_json::Error as serde::de::Error>::custom("Missing settings JSON value")
    })??;
    Ok(input.split_at(stream.byte_offset()))
}

/// Consume the separator between validated object members or array elements.
fn after_member(input: &str) -> serde_json::Result<&str> {
    let input = input.trim_start();
    if input.is_empty() {
        Ok(input)
    } else {
        input.strip_prefix(',').ok_or_else(|| {
            <serde_json::Error as serde::de::Error>::custom("Missing settings JSON separator")
        })
    }
}

impl Serialize for SettingsJson {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Object(map) => map.serialize(serializer),
            Self::Array(elements) => elements.serialize(serializer),
            Self::Scalar(value) => value.serialize(serializer),
        }
    }
}

/// Convert values constructed by typed settings writers without parsing objects.
impl From<Value> for SettingsJson {
    fn from(value: Value) -> Self {
        match value {
            Value::Object(map) => {
                Self::Object(map.into_iter().map(|(k, v)| (k, v.into())).collect())
            }
            Value::Array(elements) => Self::Array(elements.into_iter().map(Self::from).collect()),
            scalar => Self::Scalar(scalar),
        }
    }
}

#[cfg(test)]
impl std::ops::Index<&str> for SettingsJson {
    type Output = Self;
    fn index(&self, key: &str) -> &Self {
        static NULL: SettingsJson = SettingsJson::Scalar(Value::Null);
        self.get(key).unwrap_or(&NULL)
    }
}

#[cfg(test)]
impl PartialEq<&str> for SettingsJson {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == Some(*other)
    }
}

#[cfg(test)]
impl PartialEq<i32> for SettingsJson {
    fn eq(&self, other: &i32) -> bool {
        self == &Self::from(Value::from(*other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_json_preserves_literal_objects_numbers_and_order() -> serde_json::Result<()> {
        for literal in [
            r#"{"$serde_json::private::Number":"123"}"#,
            r#"{"$serde_json::private::Number":"not-a-number"}"#,
            r#"{"$serde_json::private::Number":"123","other":"keep"}"#,
            r#"{"other":"keep","$serde_json::private::Number":"123"}"#,
            r#"{"z":123456789012345678901234567890,"a":0.12345678901234567890123456789}"#,
            r#"{"quoted\"key:,[":"text \" ] } {","中文":[[],{},true,false,null,-123]}"#,
            r#"{"$serde_json::private::RawValue":"not JSON"}"#,
        ] {
            for input in [
                literal.to_string(),
                format!(r#"{{"nested":[{literal},{{"deep":{literal}}}]}}"#),
            ] {
                let value = SettingsJson::parse(&input)?;
                assert_eq!(serde_json::to_string(&value)?, input);
            }
        }
        Ok(())
    }

    #[test]
    fn settings_json_rejects_excessive_nesting_and_invalid_json() {
        let nested = format!("{}null{}", "[".repeat(200), "]".repeat(200));
        assert!(SettingsJson::parse(&nested).is_err());
        for invalid in ["{", "[1,]", "{} trailing", r#"{"a":01}"#] {
            assert!(SettingsJson::parse(invalid).is_err());
        }
    }

    #[test]
    fn settings_json_equality_ignores_formatting_and_object_order() -> serde_json::Result<()> {
        let a = SettingsJson::parse(
            r#"{"b":[true,null,123],"a":{"$serde_json::private::Number":"123"}}"#,
        )?;
        let b = SettingsJson::parse(
            r#"{ "a" : { "$serde_json::private::Number" : "\u003123" }, "b" : [true, null, 123] }"#,
        )?;
        assert_eq!(a, b);
        let number = SettingsJson::parse("123")?;
        assert_ne!(a["a"], number);
        let different = SettingsJson::parse(
            r#"{"b":[null,true,123],"a":{"$serde_json::private::Number":"123"}}"#,
        )?;
        assert_ne!(a, different);
        Ok(())
    }
}
