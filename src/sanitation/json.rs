use super::{refused, Result};
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::fmt;

struct Unique(Value);
struct Values;

impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> std::result::Result<Self, D::Error> {
        decoder.deserialize_any(Values)
    }
}

impl<'de> Visitor<'de> for Values {
    type Value = Unique;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("supported JSON")
    }
    fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<Unique, E> {
        Ok(Unique(Value::String(value.into())))
    }
    fn visit_bool<E: de::Error>(self, value: bool) -> std::result::Result<Unique, E> {
        Ok(Unique(value.into()))
    }
    fn visit_i64<E: de::Error>(self, value: i64) -> std::result::Result<Unique, E> {
        Ok(Unique(value.into()))
    }
    fn visit_u64<E: de::Error>(self, value: u64) -> std::result::Result<Unique, E> {
        Ok(Unique(value.into()))
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> std::result::Result<Unique, E> {
        serde_json::Number::from_f64(value)
            .map(|n| Unique(Value::Number(n)))
            .ok_or_else(|| E::custom("unsupported number"))
    }
    fn visit_unit<E: de::Error>(self) -> std::result::Result<Unique, E> {
        Ok(Unique(Value::Null))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut values: A) -> std::result::Result<Unique, A::Error> {
        let mut output = Vec::new();
        while let Some(value) = values.next_element::<Unique>()? {
            output.push(value.0);
        }
        Ok(Unique(Value::Array(output)))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut values: A) -> std::result::Result<Unique, A::Error> {
        let mut output = serde_json::Map::new();
        while let Some(key) = values.next_key::<String>()? {
            if output.contains_key(&key) {
                return Err(de::Error::custom("duplicate field"));
            }
            output.insert(key, values.next_value::<Unique>()?.0);
        }
        Ok(Unique(Value::Object(output)))
    }
}

pub(super) fn decode(bytes: &[u8]) -> Result<Value> {
    serde_json::from_slice::<Unique>(bytes)
        .map(|v| v.0)
        .map_err(|_| refused())
}
