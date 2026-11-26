use super::linux_admission_storage::{record_seal, StoredRecord, TypedSnapshot, TypedStorage};
use super::linux_creation_record::Record;
use std::io;

impl record_seal::Sealed for Record {}

impl StoredRecord for Record {
    fn credential_id(&self) -> &str {
        &self.credential_id
    }

    fn canonical_bytes(&self) -> io::Result<Vec<u8>> {
        Record::canonical_bytes(self)
    }

    fn parse(bytes: &[u8]) -> io::Result<Self> {
        Record::parse(bytes)
    }
}

pub type Storage = TypedStorage<Record>;
pub type Snapshot = TypedSnapshot<Record>;

#[cfg(test)]
mod tests {
    use super::super::linux_admission_storage::Metadata;
    use super::*;

    fn fixture() -> Record {
        Record {
            version: 2,
            installation_id: "00000000000000000000000001".into(),
            creation_id: "00000000000000000000000002".into(),
            device_id: "00000000000000000000000003".into(),
            credential_id: "00000000000000000000000004".into(),
            public_key: "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a".into(),
            ciphertext_sha256: "11".repeat(32),
            manifest_sha256: "22".repeat(32),
            ciphertext_len: 65536,
            manifest_len: 2048,
            creation_profile_revision: 1,
            secret_policy_revision: 1,
        }
    }

    #[test]
    fn creation_storage_codec_roundtrips_large_profile_exactly() {
        for (ciphertext_len, manifest_len) in [(1, 1), (8193, 2048), (65536, 2048)] {
            let mut record = fixture();
            record.ciphertext_len = ciphertext_len;
            record.manifest_len = manifest_len;
            let bytes = <Record as StoredRecord>::canonical_bytes(&record).unwrap();
            let decoded = <Record as StoredRecord>::parse(&bytes).unwrap();
            assert_eq!(decoded, record);
            assert_eq!(
                <Record as StoredRecord>::credential_id(&decoded),
                record.credential_id
            );
            assert_eq!(
                <Record as StoredRecord>::canonical_bytes(&decoded).unwrap(),
                bytes
            );
        }
    }

    #[test]
    fn creation_and_fixture_storage_codecs_refuse_each_others_versions() {
        let mut record = fixture();
        record.ciphertext_len = 128;
        record.manifest_len = 256;
        let operational = <Record as StoredRecord>::canonical_bytes(&record).unwrap();
        let fixture_bytes = String::from_utf8(operational.clone())
            .unwrap()
            .replacen("\"version\":2", "\"version\":1", 1)
            .into_bytes();
        let metadata = <Metadata as StoredRecord>::parse(&fixture_bytes).unwrap();
        assert_eq!(
            <Metadata as StoredRecord>::canonical_bytes(&metadata).unwrap(),
            fixture_bytes
        );
        assert!(<Record as StoredRecord>::parse(&fixture_bytes).is_err());
        assert!(<Metadata as StoredRecord>::parse(&operational).is_err());
        assert_eq!(
            <Record as StoredRecord>::parse(&operational).unwrap(),
            record
        );
    }

    #[test]
    fn creation_storage_codec_refuses_noncanonical_records_and_profile_overflow() {
        let record = fixture();
        let bytes = <Record as StoredRecord>::canonical_bytes(&record).unwrap();
        let mut newline = bytes.clone();
        newline.push(b'\n');
        assert!(<Record as StoredRecord>::parse(&newline).is_err());
        let mut object: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        object["admitted"] = true.into();
        assert!(<Record as StoredRecord>::parse(&serde_json::to_vec(&object).unwrap()).is_err());
        for (ciphertext_len, manifest_len) in [(65537, 2048), (65536, 2049), (0, 1), (1, 0)] {
            let mut invalid = record.clone();
            invalid.ciphertext_len = ciphertext_len;
            invalid.manifest_len = manifest_len;
            assert!(<Record as StoredRecord>::canonical_bytes(&invalid).is_err());
            assert!(
                <Record as StoredRecord>::parse(&serde_json::to_vec(&invalid).unwrap()).is_err()
            );
        }
    }
}
