//! An order-preserving JSON value, used only inside `setup`.
//!
//! `serde_json`'s own `preserve_order` feature would do this, but turning it
//! on changes the key order of every `--json` reader in the crate, because
//! it is a global switch on `serde_json::Value` itself (`t565` §7.5). This
//! type exists so `setup` gets ordering without paying that price: a
//! `Visitor` collects an object's entries in the order the deserializer
//! hands them over -- which is the order they appear in the source, with or
//! without that feature -- into a `Vec` instead of a `Map`.

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(serde_json::Number),
    String(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    pub fn str(s: &str) -> Value {
        Value::String(s.to_string())
    }

    pub fn object(pairs: Vec<(&str, Value)>) -> Value {
        Value::Object(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    pub fn as_object(&self) -> Option<&[(String, Value)]> {
        match self {
            Value::Object(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_object_mut(&mut self) -> Option<&mut Vec<(String, Value)>> {
        match self {
            Value::Object(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_array_mut(&mut self) -> Option<&mut Vec<Value>> {
        match self {
            Value::Array(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_object()?
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
    }

    /// Sets `key` to `value` in an object, in place if it already exists and
    /// appended at the end if it does not -- so touching an existing key
    /// never moves it, which is the ordering guarantee `t565` §7.5 asks for.
    pub fn set(&mut self, key: &str, value: Value) {
        let Some(obj) = self.as_object_mut() else {
            return;
        };
        match obj.iter_mut().find(|(k, _)| k == key) {
            Some(entry) => entry.1 = value,
            None => obj.push((key.to_string(), value)),
        }
    }

    pub fn is_object(&self) -> bool {
        matches!(self, Value::Object(_))
    }
}

impl<'de> Deserialize<'de> for Value {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ValueVisitor;

        impl<'de> Visitor<'de> for ValueVisitor {
            type Value = Value;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a JSON value")
            }

            fn visit_bool<E>(self, v: bool) -> Result<Value, E> {
                Ok(Value::Bool(v))
            }

            fn visit_i64<E>(self, v: i64) -> Result<Value, E> {
                Ok(Value::Number(v.into()))
            }

            fn visit_u64<E>(self, v: u64) -> Result<Value, E> {
                Ok(Value::Number(v.into()))
            }

            fn visit_f64<E>(self, v: f64) -> Result<Value, E>
            where
                E: de::Error,
            {
                Ok(serde_json::Number::from_f64(v)
                    .map(Value::Number)
                    .unwrap_or(Value::Null))
            }

            fn visit_str<E>(self, v: &str) -> Result<Value, E> {
                Ok(Value::String(v.to_string()))
            }

            fn visit_string<E>(self, v: String) -> Result<Value, E> {
                Ok(Value::String(v))
            }

            fn visit_unit<E>(self) -> Result<Value, E> {
                Ok(Value::Null)
            }

            fn visit_none<E>(self) -> Result<Value, E> {
                Ok(Value::Null)
            }

            fn visit_some<D>(self, deserializer: D) -> Result<Value, D::Error>
            where
                D: Deserializer<'de>,
            {
                Deserialize::deserialize(deserializer)
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut v = Vec::new();
                while let Some(item) = seq.next_element()? {
                    v.push(item);
                }
                Ok(Value::Array(v))
            }

            fn visit_map<A>(self, mut map: A) -> Result<Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut v = Vec::new();
                while let Some((k, val)) = map.next_entry::<String, Value>()? {
                    v.push((k, val));
                }
                Ok(Value::Object(v))
            }
        }

        deserializer.deserialize_any(ValueVisitor)
    }
}

impl Serialize for Value {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Value::Null => serializer.serialize_unit(),
            Value::Bool(b) => serializer.serialize_bool(*b),
            Value::Number(n) => n.serialize(serializer),
            Value::String(s) => serializer.serialize_str(s),
            Value::Array(items) => {
                let mut seq = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            Value::Object(pairs) => {
                let mut map = serializer.serialize_map(Some(pairs.len()))?;
                for (k, v) in pairs {
                    map.serialize_entry(k, v)?;
                }
                map.end()
            }
        }
    }
}

/// Parses `raw`, keeping every object's key order. The error is
/// `serde_json`'s own, so a caller can read `.line()` and `.column()` off it.
pub fn parse(raw: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(raw)
}

/// Renders `value` with `indent` at every level, through `serde_json`'s own
/// `PrettyFormatter` -- the same pretty-printer the rest of the crate uses,
/// just fed an indent this file already had instead of the fixed two spaces.
/// The result always ends its lines in `\n`; `finalize` below turns that
/// into the file's own line ending.
pub fn render(value: &Value, indent: &str) -> String {
    let formatter = serde_json::ser::PrettyFormatter::with_indent(indent.as_bytes());
    let mut buf = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut buf, formatter);
    value
        .serialize(&mut serializer)
        .expect("an in-memory Value never fails to serialize");
    String::from_utf8(buf).expect("serde_json only ever writes valid UTF-8")
}

/// The indent unit of `raw`'s first indented line, or two spaces for a file
/// that has none -- a brand new one, or one written all on one line.
pub fn detect_indent(raw: &str) -> String {
    for line in raw.lines() {
        let stripped = line.trim_start_matches([' ', '\t']);
        if stripped.len() != line.len() && !stripped.is_empty() {
            return line[..line.len() - stripped.len()].to_string();
        }
    }
    "  ".to_string()
}

/// `\r\n` if `raw` uses it anywhere, `\n` otherwise. A brand new file gets
/// `\n`.
pub fn detect_eol(raw: &str) -> &'static str {
    if raw.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

pub fn has_trailing_newline(raw: &str) -> bool {
    raw.ends_with('\n')
}

/// Turns `render`'s `\n`-only text into the file's own line ending and
/// trailing newline. None of the strings `setup` ever writes into a value
/// carry a literal newline of their own, so replacing every `\n` is exact
/// and never touches the middle of a line by accident.
pub fn finalize(rendered: &str, eol: &str, trailing_newline: bool) -> String {
    let mut s = if eol == "\r\n" {
        rendered.replace('\n', "\r\n")
    } else {
        rendered.to_string()
    };
    if trailing_newline {
        s.push_str(eol);
    }
    s
}

/// Whether every key path of `original` still holds the same value in
/// `updated`, checked recursively into nested objects; an array's elements
/// in `original` have to be a prefix of the same array in `updated`. `t565`
/// §7.6's second check on `setup`'s own writes: they only ever grow what
/// they touch.
pub fn extends(original: &Value, updated: &Value) -> bool {
    match (original, updated) {
        (Value::Object(o), Value::Object(u)) => o.iter().all(|(k, v)| {
            u.iter()
                .find(|(uk, _)| uk == k)
                .is_some_and(|(_, uv)| extends(v, uv))
        }),
        (Value::Array(o), Value::Array(u)) => {
            o.len() <= u.len() && o.iter().zip(u.iter()).all(|(ov, uv)| extends(ov, uv))
        }
        _ => original == updated,
    }
}

/// The mirror check for `--undo`: every key path of `after` holds the same
/// value somewhere in `before`, and its arrays are an in-order subsequence
/// of `before`'s -- what removing exactly what setup wrote, and nothing
/// else, is allowed to leave behind.
pub fn contained_in(after: &Value, before: &Value) -> bool {
    match (after, before) {
        (Value::Object(a), Value::Object(b)) => a.iter().all(|(k, v)| {
            b.iter()
                .find(|(bk, _)| bk == k)
                .is_some_and(|(_, bv)| contained_in(v, bv))
        }),
        (Value::Array(a), Value::Array(b)) => is_subsequence(a, b),
        _ => after == before,
    }
}

fn is_subsequence(needle: &[Value], haystack: &[Value]) -> bool {
    let mut i = 0;
    'outer: for n in needle {
        while i < haystack.len() {
            let h = &haystack[i];
            i += 1;
            if contained_in(n, h) {
                continue 'outer;
            }
        }
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(pairs: Vec<(&str, Value)>) -> Value {
        Value::object(pairs)
    }

    #[test]
    fn extends_passes_when_only_an_array_grew_at_the_end() {
        let original = obj(vec![("a", Value::Array(vec![Value::str("x")]))]);
        let updated = obj(vec![(
            "a",
            Value::Array(vec![Value::str("x"), Value::str("y")]),
        )]);
        assert!(extends(&original, &updated));
    }

    #[test]
    fn extends_fails_when_a_key_is_missing() {
        let original = obj(vec![("a", Value::str("1")), ("b", Value::str("2"))]);
        let updated = obj(vec![("a", Value::str("1"))]);
        assert!(!extends(&original, &updated));
    }

    #[test]
    fn extends_fails_when_a_value_changed() {
        let original = obj(vec![("a", Value::str("1"))]);
        let updated = obj(vec![("a", Value::str("2"))]);
        assert!(!extends(&original, &updated));
    }

    #[test]
    fn extends_fails_when_an_array_is_reordered() {
        let original = obj(vec![(
            "a",
            Value::Array(vec![Value::str("x"), Value::str("y")]),
        )]);
        let updated = obj(vec![(
            "a",
            Value::Array(vec![Value::str("y"), Value::str("x")]),
        )]);
        assert!(!extends(&original, &updated));
    }

    #[test]
    fn contained_in_passes_when_the_array_lost_an_element_in_order() {
        let after = obj(vec![(
            "a",
            Value::Array(vec![Value::str("x"), Value::str("z")]),
        )]);
        let before = obj(vec![(
            "a",
            Value::Array(vec![Value::str("x"), Value::str("y"), Value::str("z")]),
        )]);
        assert!(contained_in(&after, &before));
    }

    #[test]
    fn contained_in_fails_when_a_value_changed() {
        let after = obj(vec![("a", Value::str("1"))]);
        let before = obj(vec![("a", Value::str("2"))]);
        assert!(!contained_in(&after, &before));
    }

    #[test]
    fn contained_in_fails_when_the_remaining_array_is_out_of_order() {
        let after = obj(vec![(
            "a",
            Value::Array(vec![Value::str("z"), Value::str("x")]),
        )]);
        let before = obj(vec![(
            "a",
            Value::Array(vec![Value::str("x"), Value::str("y"), Value::str("z")]),
        )]);
        assert!(!contained_in(&after, &before));
    }

    #[test]
    fn object_order_survives_a_round_trip() {
        let raw = r#"{"z":1,"a":2,"m":3}"#;
        let v = parse(raw).unwrap();
        let keys: Vec<&str> = v
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(keys, ["z", "a", "m"]);
    }

    #[test]
    fn set_overwrites_in_place_and_appends_when_new() {
        let mut v = Value::object(vec![("a", Value::str("1")), ("b", Value::str("2"))]);
        v.set("a", Value::str("changed"));
        v.set("c", Value::str("3"));
        let keys: Vec<&str> = v
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(keys, ["a", "b", "c"]);
        assert_eq!(v.get("a").unwrap().as_str(), Some("changed"));
    }

    #[test]
    fn render_uses_the_given_indent() {
        let v = Value::object(vec![("a", Value::object(vec![("b", Value::str("c"))]))]);
        let out = render(&v, "    ");
        assert!(out.contains("\n    \"a\": {\n        \"b\""), "{out}");
    }

    #[test]
    fn indent_is_detected_from_the_first_indented_line() {
        assert_eq!(detect_indent("{\n    \"a\": 1\n}"), "    ");
        assert_eq!(detect_indent("{\"a\":1}"), "  ");
    }

    #[test]
    fn eol_is_detected_and_finalize_applies_it() {
        assert_eq!(detect_eol("a\r\nb"), "\r\n");
        assert_eq!(detect_eol("a\nb"), "\n");
        assert_eq!(finalize("a\nb", "\r\n", true), "a\r\nb\r\n");
        assert_eq!(finalize("a\nb", "\n", false), "a\nb");
    }
}
