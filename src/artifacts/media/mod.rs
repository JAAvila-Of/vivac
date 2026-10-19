//! Format inspection for explicitly reviewed members of a complete web delivery.
mod mp4;
mod png;

use super::guard::Receipt;
use crate::failure::Failure;
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};

pub(super) const MAX_DECODED_BYTES: u64 = 32 * 1024 * 1024;
pub(super) const MAX_VIDEO_SAMPLES: usize = 8192;
pub(super) const MAX_NAL_UNITS: usize = 16384;

pub(super) struct Budget {
    decoded_remaining: u64,
    sample_remaining: usize,
    nal_remaining: usize,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            decoded_remaining: MAX_DECODED_BYTES,
            sample_remaining: MAX_VIDEO_SAMPLES,
            nal_remaining: MAX_NAL_UNITS,
        }
    }
}

pub(super) fn copy_checked_with_budget(
    mut reader: impl Read + Seek,
    mut writer: impl Write,
    media_type: &str,
    limit: u64,
    budget: &mut Budget,
) -> Result<Receipt, Failure> {
    let size = reader.seek(SeekFrom::End(0))?;
    if size == 0 || size > limit || size > super::MAX_FILE_BYTES {
        return Err(Failure::usage(
            "Media content exceeds the complete revision byte budget or is empty.",
        ));
    }
    reader.seek(SeekFrom::Start(0))?;
    match media_type {
        "image/png" => png::inspect_with_budget(&mut reader, budget)?,
        "video/mp4" => mp4::inspect_with_budget(&mut reader, budget)?,
        _ => return Err(Failure::usage("Unsupported reviewed media type.")),
    }
    reader.seek(SeekFrom::Start(0))?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 8192];
    let mut copied = 0u64;
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        copied += count as u64;
        if copied > size {
            return Err(Failure::usage(
                "The media source changed during inspection.",
            ));
        }
        writer.write_all(&buffer[..count])?;
        digest.update(&buffer[..count]);
    }
    if copied != size {
        return Err(Failure::usage(
            "The media source changed during inspection.",
        ));
    }
    Ok(Receipt {
        digest: format!("{:x}", digest.finalize()),
        size,
    })
}

#[cfg(test)]
pub(super) fn copy_checked(
    reader: impl Read + Seek,
    writer: impl Write,
    media_type: &str,
    limit: u64,
) -> Result<Receipt, Failure> {
    copy_checked_with_budget(reader, writer, media_type, limit, &mut Budget::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn reviewed_video_work_budgets_are_shared_between_files() {
        let bytes = include_bytes!("../../../tests/data/artifact-media/sample.mp4");
        // The fixture has four samples containing five NAL units in total.
        for (samples, nals, accepts_second) in [(7, 10, false), (8, 9, false), (8, 10, true)] {
            let mut budget = Budget {
                sample_remaining: samples,
                nal_remaining: nals,
                ..Budget::default()
            };
            let mut output = Vec::new();
            copy_checked_with_budget(
                Cursor::new(bytes),
                &mut output,
                "video/mp4",
                bytes.len() as u64,
                &mut budget,
            )
            .unwrap();
            assert_eq!(output, bytes);
            output.clear();
            let result = copy_checked_with_budget(
                Cursor::new(bytes),
                &mut output,
                "video/mp4",
                bytes.len() as u64,
                &mut budget,
            );
            assert_eq!(result.is_ok(), accepts_second);
            if accepts_second {
                assert_eq!(output, bytes);
            } else {
                assert!(output.is_empty());
            }
        }
    }

    #[test]
    fn inspected_media_preserves_bytes_and_hashes_without_writing_failed_inputs() {
        for (bytes, media_type) in [
            (
                include_bytes!("../../../tests/data/artifact-media/sample.png").as_slice(),
                "image/png",
            ),
            (
                include_bytes!("../../../tests/data/artifact-media/sample.mp4").as_slice(),
                "video/mp4",
            ),
        ] {
            let mut output = Vec::new();
            let receipt = copy_checked(
                Cursor::new(bytes),
                &mut output,
                media_type,
                bytes.len() as u64,
            )
            .unwrap();
            assert_eq!(output, bytes);
            assert_eq!(receipt.digest, format!("{:x}", Sha256::digest(bytes)));
            assert_eq!(receipt.size, bytes.len() as u64);
            let mut malformed = bytes.to_vec();
            malformed.extend_from_slice(b"unselected bytes");
            output.clear();
            assert!(copy_checked(
                Cursor::new(malformed),
                &mut output,
                media_type,
                super::super::MAX_FILE_BYTES
            )
            .is_err());
            assert!(output.is_empty());
            assert!(copy_checked(
                Cursor::new(bytes),
                &mut output,
                media_type,
                bytes.len() as u64 - 1
            )
            .is_err());
            assert!(output.is_empty());
        }
    }
}
