//! Inspect decoded provenance values without retaining another JSON tree.
use crate::failure::Failure;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::fmt;
use std::io::Read;

struct Checked;

struct Values;

impl<'de> Deserialize<'de> for Checked {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(Values)
    }
}

impl<'de> Visitor<'de> for Values {
    type Value = Checked;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JSON values allowed in provenance")
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<Checked, E> {
        if crate::redact::check_field("backup source", text).is_some() {
            return Err(E::custom("disallowed provenance data"));
        }
        Ok(Checked)
    }

    fn visit_string<E: de::Error>(self, text: String) -> Result<Checked, E> {
        self.visit_str(&text)
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Checked, E> {
        Ok(Checked)
    }

    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Checked, E> {
        Ok(Checked)
    }

    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Checked, E> {
        Ok(Checked)
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Checked, E> {
        Ok(Checked)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Checked, E> {
        Ok(Checked)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut values: A) -> Result<Checked, A::Error> {
        while values.next_element::<Checked>()?.is_some() {}
        Ok(Checked)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut values: A) -> Result<Checked, A::Error> {
        while values.next_key::<Checked>()?.is_some() {
            values.next_value::<Checked>()?;
        }
        Ok(Checked)
    }
}

/// Input framing and schema are checked separately by the caller.
pub(super) fn check_json(reader: impl Read) -> Result<(), Failure> {
    let mut seen = false;
    for value in serde_json::Deserializer::from_reader(reader).into_iter::<Checked>() {
        seen = true;
        if let Err(error) = value {
            if error.is_data() {
                return Err(Failure::Redaction(Box::new(crate::redact::Finding {
                    rule: "sensitive or fenced provenance content",
                    field: "backup source".into(),
                    sample: "[redacted]".into(),
                    advice: "Keep secrets, personal data and file bodies out of provenance.",
                })));
            }
            if let Some(kind) = error.io_error_kind() {
                return Err(Failure::Io(std::io::Error::new(
                    kind,
                    "Backup source could not be read.",
                )));
            }
            return Err(Failure::usage("Backup source contains invalid JSON."));
        }
    }
    if !seen {
        return Err(Failure::usage("Backup source contains no JSON values."));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privacy_checks_decoded_strings_and_unknown_keys_without_echoing_them() {
        let secret = format!("ghp_{}", "k".repeat(32));
        for input in [
            serde_json::json!({"title": secret}).to_string(),
            serde_json::json!({"extra": {secret.clone(): "ordinary text"}}).to_string(),
            serde_json::json!({"unknown": secret})
                .to_string()
                .replace("ghp_", "g\\u0068p_"),
        ] {
            let error = check_json(input.as_bytes()).unwrap_err();
            assert_eq!(error.code(), 3);
            assert!(!error.message().contains(&secret));
        }
        assert_eq!(
            check_json(br#"{"why":"```\nfile body\n```"}"#.as_slice())
                .unwrap_err()
                .code(),
            3
        );
    }

    #[test]
    fn privacy_accepts_technical_metadata_and_multiple_complete_records() {
        let input = format!(
            "{{\"id\":\"01m63whyag8fyahm0emp7515jr\",\"digest\":\"{}\"}}\n{{\"state\":\"active\",\"count\":2,\"safe\":true,\"old\":null}}\n",
            "a".repeat(64)
        );
        check_json(input.as_bytes()).unwrap();
        assert!(check_json(b"{\"title\":".as_slice()).is_err());
    }
}
