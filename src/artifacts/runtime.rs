use std::io::{Read, Write};

use flate2::{Decompress, FlushDecompress, Status};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::guard;
use crate::failure::Failure;

pub(super) const ID: &str = "sqljs-asm-1.14.2";
pub(super) const PATH: &str = "vivac-sqlite.js";
pub(super) const BYTES: u64 = 1_357_427;
pub(super) const SHA256: &str = "a5709877905821fcae8aeb6789c53302eb0ffc50473f81dc2bfbdd93a16f9b38";
pub(super) const PACKED_LEN: usize = 453_701;

const PACKED_SHA256: &str = "87b4ee801f9d4574e047094065d19ba2b8772a2e7f3f136a4e72179cb5beeb37";
const PACKED: &[u8; PACKED_LEN] =
    include_bytes!("../../vendor/sqljs-asm-1.14.2/vivac-sqlite.js.deflate");
const CHUNK_SIZE: usize = 64 * 1024;
const MATERIALIZATION_FAILED: &str = "Product runtime materialization failed.";
const RETAINED_MISMATCH: &str = "Retained product runtime bytes do not match the engine catalog.";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Descriptor {
    pub(super) id: String,
    pub(super) path: String,
    pub(super) bytes: u64,
    pub(super) sha256: String,
}

pub(super) fn known_descriptor(id: &str) -> Result<Descriptor, Failure> {
    match id {
        ID => Ok(Descriptor {
            id: ID.into(),
            path: PATH.into(),
            bytes: BYTES,
            sha256: SHA256.into(),
        }),
        _ => Err(Failure::usage("Unsupported product runtime identity.")),
    }
}

pub(super) fn descriptor(id: &str) -> Result<Descriptor, Failure> {
    match id {
        ID => known_descriptor(id),
        _ => Err(Failure::usage("Unsupported product runtime identity.")),
    }
}

pub(super) fn validate(descriptor: &Descriptor) -> Result<(), Failure> {
    if known_descriptor(&descriptor.id).is_ok_and(|known| known == *descriptor) {
        Ok(())
    } else {
        Err(Failure::usage(
            "Product runtime metadata does not match the engine catalog.",
        ))
    }
}

pub(super) fn materialize(id: &str, mut writer: impl Write) -> Result<guard::Receipt, Failure> {
    let entry = descriptor(id)?;
    let failed = || Failure::usage(MATERIALIZATION_FAILED);
    if format!("{:x}", Sha256::digest(PACKED)) != PACKED_SHA256 {
        return Err(failed());
    }
    let mut decoder = Decompress::new(false);
    let mut output = [0u8; CHUNK_SIZE];
    let mut digest = Sha256::new();
    loop {
        let before_input = decoder.total_in();
        let before_output = decoder.total_out();
        let limit = ((entry.bytes - before_output + 1) as usize).min(output.len());
        let status = decoder
            .decompress(
                &PACKED[before_input as usize..],
                &mut output[..limit],
                FlushDecompress::None,
            )
            .map_err(|_| failed())?;
        if decoder.total_out() > entry.bytes {
            return Err(failed());
        }
        let count = (decoder.total_out() - before_output) as usize;
        writer.write_all(&output[..count]).map_err(|_| failed())?;
        digest.update(&output[..count]);
        if status == Status::StreamEnd {
            if decoder.total_in() != PACKED_LEN as u64 || decoder.total_out() != entry.bytes {
                return Err(failed());
            }
            break;
        }
        if decoder.total_in() == before_input && decoder.total_out() == before_output {
            return Err(failed());
        }
    }
    let digest = format!("{:x}", digest.finalize());
    if digest != entry.sha256 {
        return Err(failed());
    }
    Ok(guard::Receipt {
        digest,
        size: entry.bytes,
    })
}

pub(super) fn inspect(
    descriptor: &Descriptor,
    reader: impl Read,
) -> Result<guard::Receipt, Failure> {
    validate(descriptor)?;
    let mut reader = reader.take(descriptor.bytes + 1);
    let mut input = [0u8; CHUNK_SIZE];
    let mut digest = Sha256::new();
    let mut size = 0u64;
    loop {
        let count = match reader.read(&mut input) {
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(Failure::usage(RETAINED_MISMATCH)),
        };
        if count == 0 {
            break;
        }
        size += count as u64;
        if size > descriptor.bytes {
            return Err(Failure::usage(RETAINED_MISMATCH));
        }
        digest.update(&input[..count]);
    }
    let digest = format!("{:x}", digest.finalize());
    if size != descriptor.bytes || digest != descriptor.sha256 {
        return Err(Failure::usage(RETAINED_MISMATCH));
    }
    Ok(guard::Receipt { digest, size })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Cursor};

    fn body() -> Vec<u8> {
        let mut bytes = Vec::new();
        materialize(ID, &mut bytes).unwrap();
        bytes
    }

    #[test]
    fn descriptor_matches_the_complete_catalog_entry() {
        let entry = descriptor(ID).unwrap();
        assert_eq!(entry.id, ID);
        assert_eq!(entry.path, PATH);
        assert_eq!(entry.bytes, BYTES);
        assert_eq!(entry.sha256, SHA256);
        assert_eq!(known_descriptor(ID).unwrap(), entry);
        assert!(known_descriptor("unknown").is_err());
        validate(&entry).unwrap();
        assert!(descriptor("unknown").is_err());
        assert!(descriptor(PATH).is_err());
    }

    #[test]
    fn catalog_refuses_changed_metadata_and_origin_labels() {
        let entry = descriptor(ID).unwrap();
        for field in ["id", "path", "bytes", "sha256"] {
            let mut changed = entry.clone();
            match field {
                "id" => changed.id = "unknown".into(),
                "path" => changed.path = "other.js".into(),
                "bytes" => changed.bytes -= 1,
                "sha256" => changed.sha256 = "0".repeat(64),
                _ => unreachable!(),
            }
            assert!(validate(&changed).is_err());
            assert!(inspect(&changed, io::empty()).is_err());
        }
        let mut changed = serde_json::to_value(entry).unwrap();
        changed["origin"] = "product".into();
        assert!(serde_json::from_value::<Descriptor>(changed).is_err());
    }

    #[test]
    fn materialized_program_preserves_license_and_original_runtime() {
        let bytes = body();
        let upstream = include_bytes!("../../vendor/sqljs-asm-1.14.2/sql-asm.js");
        let license = include_bytes!("../../vendor/sqljs-asm-1.14.2/LICENSE");
        let header = b"/*! sql.js 1.14.2. Upstream runtime follows unchanged.\n";
        assert_eq!(&bytes[..header.len()], header);
        assert_eq!(&bytes[header.len()..header.len() + license.len()], license);
        assert_eq!(&bytes[header.len() + license.len()..][..4], b"\n*/\n");
        assert_eq!(&bytes[header.len() + license.len() + 4..], upstream);
        let receipt = inspect(&descriptor(ID).unwrap(), Cursor::new(&bytes)).unwrap();
        assert_eq!(receipt.size, BYTES);
        assert_eq!(receipt.digest, SHA256);
        assert_eq!(
            include_bytes!("../../vendor/sqljs-asm-1.14.2/vivac-sqlite.js.deflate").len(),
            PACKED_LEN
        );
    }

    #[test]
    fn retained_program_refuses_modified_truncated_and_extra_bytes() {
        let entry = descriptor(ID).unwrap();
        let mut bytes = body();
        bytes[100] ^= 1;
        assert!(inspect(&entry, Cursor::new(&bytes)).is_err());
        bytes[100] ^= 1;
        assert!(inspect(&entry, Cursor::new(&bytes[..bytes.len() - 1])).is_err());
        bytes.push(0);
        assert!(inspect(&entry, Cursor::new(&bytes)).is_err());
        assert!(inspect(&entry, io::empty()).is_err());
    }

    struct BrokenWriter;

    impl Write for BrokenWriter {
        fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("writer detail must not escape"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn materialization_refuses_unknown_identity_and_writer_failure() {
        assert!(materialize("unknown", io::sink()).is_err());
        let error = materialize(ID, BrokenWriter).unwrap_err();
        assert!(
            matches!(error, Failure::Usage(message) if message.trim() == "Product runtime materialization failed.")
        );
    }

    #[test]
    fn author_supplied_runtime_keeps_the_normal_content_guard() {
        let bytes = body();
        assert!(guard::copy_checked(Cursor::new(&bytes), io::sink()).is_err());
        let upstream = include_bytes!("../../vendor/sqljs-asm-1.14.2/sql-asm.js");
        assert!(guard::copy_checked(Cursor::new(upstream), io::sink()).is_err());
    }

    struct Unreadable;

    impl Read for Unreadable {
        fn read(&mut self, _bytes: &mut [u8]) -> io::Result<usize> {
            panic!("Untrusted metadata must be rejected before reading bytes.");
        }
    }

    #[test]
    fn metadata_is_checked_before_any_retained_bytes_are_read() {
        let mut entry = descriptor(ID).unwrap();
        entry.sha256 = "0".repeat(64);
        assert!(inspect(&entry, Unreadable).is_err());
        let mut output = Vec::new();
        assert!(materialize(PATH, &mut output).is_err());
        assert!(output.is_empty());
    }

    struct EndlessReader {
        consumed: u64,
    }

    impl Read for EndlessReader {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            assert!(bytes.len() <= 64 * 1024);
            bytes.fill(0);
            self.consumed += bytes.len() as u64;
            Ok(bytes.len())
        }
    }

    #[test]
    fn retained_inspection_stops_after_catalog_size_plus_one() {
        let mut reader = EndlessReader { consumed: 0 };
        assert!(inspect(&descriptor(ID).unwrap(), &mut reader).is_err());
        assert_eq!(reader.consumed, BYTES + 1);
    }

    struct ChunkWriter {
        digest: Sha256,
        size: u64,
    }

    impl Write for ChunkWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            assert!(bytes.len() <= 64 * 1024);
            let count = bytes.len().min(13);
            self.digest.update(&bytes[..count]);
            self.size += count as u64;
            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn materialization_streams_bounded_chunks_and_handles_short_writes() {
        let mut writer = ChunkWriter {
            digest: Sha256::new(),
            size: 0,
        };
        let receipt = materialize(ID, &mut writer).unwrap();
        assert_eq!(writer.size, BYTES);
        assert_eq!(format!("{:x}", writer.digest.finalize()), SHA256);
        assert_eq!(receipt.size, BYTES);
        assert_eq!(receipt.digest, SHA256);
    }
}
