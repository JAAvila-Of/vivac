#[path = "linux_creation_record/model.rs"]
mod model;

pub use model::Record;
use model::{invalid, require, MAX_RECORD};
use std::io;

impl Record {
    pub fn parse(bytes: &[u8]) -> io::Result<Self> {
        require(bytes.len() <= MAX_RECORD)?;
        let value: Self = serde_json::from_slice(bytes).map_err(|_| invalid())?;
        require(value.canonical_bytes()? == bytes)?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::super::linux_admission_storage::Metadata;
    use super::*;

    const CANONICAL: &str = concat!(
        "{\"version\":2,",
        "\"installation_id\":\"00000000000000000000000001\",",
        "\"creation_id\":\"00000000000000000000000002\",",
        "\"device_id\":\"00000000000000000000000003\",",
        "\"credential_id\":\"00000000000000000000000004\",",
        "\"public_key\":\"d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a\",",
        "\"ciphertext_sha256\":\"1111111111111111111111111111111111111111111111111111111111111111\",",
        "\"manifest_sha256\":\"2222222222222222222222222222222222222222222222222222222222222222\",",
        "\"ciphertext_len\":128,\"manifest_len\":256,",
        "\"creation_profile_revision\":1,\"secret_policy_revision\":1}"
    );

    fn fixture() -> Record {
        serde_json::from_str(CANONICAL).unwrap()
    }

    fn refused(value: &Record) {
        assert!(value.canonical_bytes().is_err(), "accepted {value:?}");
        let bytes = serde_json::to_vec(value).unwrap();
        assert!(Record::parse(&bytes).is_err(), "parsed {value:?}");
    }

    #[test]
    fn exact_canonical_public_record_roundtrip() {
        let value = fixture();
        assert_eq!(value.canonical_bytes().unwrap(), CANONICAL.as_bytes());
        assert_eq!(Record::parse(CANONICAL.as_bytes()).unwrap(), value);
    }

    #[test]
    fn accepts_package_profile_boundaries_including_large_ciphertext() {
        for (ciphertext_len, manifest_len) in [(1, 1), (8193, 2048), (65536, 2048)] {
            let mut value = fixture();
            value.ciphertext_len = ciphertext_len;
            value.manifest_len = manifest_len;
            let bytes = serde_json::to_vec(&value).unwrap();
            assert_eq!(Record::parse(&bytes).unwrap(), value);
            assert_eq!(value.canonical_bytes().unwrap(), bytes);
        }
    }

    #[test]
    fn experimental_metadata_and_operational_record_are_separate() {
        let old = CANONICAL.replacen("\"version\":2", "\"version\":1", 1);
        let metadata = Metadata::parse(old.as_bytes()).unwrap();
        assert_eq!(metadata.canonical_bytes().unwrap(), old.as_bytes());
        assert!(Record::parse(old.as_bytes()).is_err());
        assert!(Metadata::parse(CANONICAL.as_bytes()).is_err());
        assert_eq!(Record::parse(CANONICAL.as_bytes()).unwrap(), fixture());
    }

    #[test]
    fn refuses_versions_and_profile_revisions() {
        for field in 0..3 {
            for revision in [0, 2, u64::MAX] {
                let mut value = fixture();
                match field {
                    0 => value.version = if revision == 2 { 1 } else { revision },
                    1 => value.creation_profile_revision = revision,
                    _ => value.secret_policy_revision = revision,
                }
                refused(&value);
            }
        }
    }

    #[test]
    fn refuses_noncanonical_json_and_encodings() {
        let escaped = CANONICAL.replacen(
            "\"installation_id\":\"0",
            "\"installation_id\":\"\\u0030",
            1,
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&escaped).unwrap(),
            serde_json::from_str::<serde_json::Value>(CANONICAL).unwrap()
        );
        let cases = [
            CANONICAL.replacen("{", "{\"approved\":true,", 1),
            CANONICAL.replacen("{", "{\"version\":2,", 1),
            CANONICAL.replacen(":2,", ": 2,", 1),
            CANONICAL
                .replace("\"version\":2,", "")
                .replacen("}", ",\"version\":2}", 1),
            format!(" {CANONICAL}"),
            format!("{CANONICAL}\n"),
            format!("\u{feff}{CANONICAL}"),
            format!("{CANONICAL}{{}}"),
            escaped,
        ];
        for bytes in cases {
            assert!(Record::parse(bytes.as_bytes()).is_err(), "accepted {bytes}");
        }
        let mut non_utf8 = CANONICAL.as_bytes().to_vec();
        non_utf8[0] = 0xff;
        assert!(Record::parse(&non_utf8).is_err());
        assert!(Record::parse(&[]).is_err());
        assert!(Record::parse(&vec![b' '; 2049]).is_err());
        let mut oversized = CANONICAL.as_bytes().to_vec();
        oversized.resize(2049, b' ');
        assert!(Record::parse(&oversized).is_err());
    }

    #[test]
    fn refuses_missing_fields_and_wrong_json_types() {
        let object: serde_json::Value = serde_json::from_str(CANONICAL).unwrap();
        for field in object.as_object().unwrap().keys() {
            let mut missing = object.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(Record::parse(&serde_json::to_vec(&missing).unwrap()).is_err());
        }
        for replacement in [
            "null",
            "true",
            "\"128\"",
            "[]",
            "{}",
            "-1",
            "1.0",
            "1e2",
            "18446744073709551616",
        ] {
            let bytes = CANONICAL.replace(
                "\"ciphertext_len\":128",
                &format!("\"ciphertext_len\":{replacement}"),
            );
            assert!(
                Record::parse(bytes.as_bytes()).is_err(),
                "accepted {replacement}"
            );
        }
        for replacement in ["null", "123", "true", "[]", "{}"] {
            let bytes = CANONICAL.replace(
                "\"creation_id\":\"00000000000000000000000002\"",
                &format!("\"creation_id\":{replacement}"),
            );
            assert!(Record::parse(bytes.as_bytes()).is_err());
        }
    }

    #[test]
    fn accepts_canonical_identifier_alphabet_and_highest_first_digit() {
        for id in [
            "00000000000000000000000000",
            "7zzzzzzzzzzzzzzzzzzzzzzzzz",
            "0123456789abcdefghjkmnpqrs",
        ] {
            let mut value = fixture();
            value.installation_id = id.into();
            value.creation_id = id.into();
            value.device_id = id.into();
            value.credential_id = id.into();
            let bytes = serde_json::to_vec(&value).unwrap();
            assert_eq!(Record::parse(&bytes).unwrap(), value);
            assert_eq!(value.canonical_bytes().unwrap(), bytes);
        }
    }

    #[test]
    fn refuses_invalid_identifiers_in_every_slot() {
        for field in 0..4 {
            for id in [
                "",
                "0000000000000000000000000",
                "000000000000000000000000000",
                "80000000000000000000000000",
                "0000000000000000000000000i",
                "0000000000000000000000000l",
                "0000000000000000000000000o",
                "0000000000000000000000000u",
                "0000000000000000000000000A",
                "0000000000000000000000000/",
                "0000000000000000000000000é",
            ] {
                let mut value = fixture();
                match field {
                    0 => value.installation_id = id.into(),
                    1 => value.creation_id = id.into(),
                    2 => value.device_id = id.into(),
                    _ => value.credential_id = id.into(),
                }
                refused(&value);
            }
        }
    }

    #[test]
    fn refuses_noncanonical_hex_and_weak_keys() {
        for field in 0..3 {
            for hex in [
                "AA".repeat(32),
                "0".repeat(63),
                "0".repeat(65),
                "gg".repeat(32),
                "".into(),
            ] {
                let mut value = fixture();
                match field {
                    0 => value.public_key = hex,
                    1 => value.ciphertext_sha256 = hex,
                    _ => value.manifest_sha256 = hex,
                }
                refused(&value);
            }
        }
        for key in ["00".repeat(32), "01".to_owned() + &"00".repeat(31)] {
            let mut value = fixture();
            value.public_key = key;
            refused(&value);
        }
    }

    #[test]
    fn refuses_out_of_profile_lengths_and_overflow_candidates() {
        for (ciphertext_len, manifest_len) in [
            (0, 1),
            (1, 0),
            (65537, 1),
            (1, 2049),
            (u64::MAX, 1),
            (1, u64::MAX),
            (u64::MAX, u64::MAX),
        ] {
            let mut value = fixture();
            value.ciphertext_len = ciphertext_len;
            value.manifest_len = manifest_len;
            refused(&value);
        }
    }
}
