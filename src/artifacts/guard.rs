use std::io::{Read, Write};

use sha2::{Digest, Sha256};

use super::policy::{CapturePolicy, Inspection, Sensitivity};
use crate::failure::Failure;
use crate::redact::{self, Finding};

const CHUNK_SIZE: usize = 64 * 1024;
const TOKEN_LIMIT: usize = 64 * 1024;

#[derive(Debug)]
pub struct Receipt {
    pub digest: String,
    pub size: u64,
}

#[derive(Debug)]
pub(super) struct ObservedReceipt {
    pub receipt: Receipt,
    pub inspection: Inspection,
}

pub(super) fn copy_observed(
    reader: impl Read,
    writer: impl Write,
    limit: u64,
) -> Result<ObservedReceipt, Failure> {
    copy_text(
        reader,
        writer,
        limit.min(super::MAX_FILE_BYTES),
        Inspection::permissive(),
    )
}

pub(super) fn check_text_observed(text: &str) -> Result<Inspection, Failure> {
    let mut inspection = Inspection::permissive();
    for token in text.split(redact::is_token_separator) {
        if token.len() > TOKEN_LIMIT {
            return Err(Failure::usage(
                "Artifact token exceeds the 64 KiB inspection limit.",
            ));
        }
        inspect_token(token, &mut inspection)?;
    }
    Ok(inspection)
}

pub fn copy_checked(reader: impl Read, writer: impl Write) -> Result<Receipt, Failure> {
    copy_text(reader, writer, super::MAX_FILE_BYTES, Inspection::strict())
        .map(|observed| observed.receipt)
}

fn copy_text(
    mut reader: impl Read,
    mut writer: impl Write,
    limit: u64,
    mut inspection: Inspection,
) -> Result<ObservedReceipt, Failure> {
    let mut input = [0u8; CHUNK_SIZE + 3];
    let mut pending = 0;
    let mut token = String::with_capacity(TOKEN_LIMIT);
    let mut output = CheckedOutput {
        writer: &mut writer,
        bytes: Vec::with_capacity(CHUNK_SIZE),
        digest: Sha256::new(),
    };
    let mut size = 0u64;
    loop {
        let count = match reader.read(&mut input[pending..pending + CHUNK_SIZE]) {
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        };
        if count == 0 {
            if pending != 0 {
                return Err(Failure::usage("Artifact content must be valid UTF-8."));
            }
            break;
        }
        size += count as u64;
        if size > limit {
            return Err(Failure::usage(format!(
                "Artifact content exceeds the {} byte limit.",
                limit
            )));
        }
        let available = pending + count;
        let valid = match std::str::from_utf8(&input[..available]) {
            Ok(text) => text.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(_) => return Err(Failure::usage("Artifact content must be valid UTF-8.")),
        };
        let text = std::str::from_utf8(&input[..valid])
            .map_err(|_| Failure::usage("Artifact content must be valid UTF-8."))?;
        for character in text.chars() {
            if redact::is_token_separator(character) {
                flush_token(&mut token, &mut output, &mut inspection)?;
                let mut encoded = [0u8; 4];
                output.append(character.encode_utf8(&mut encoded).as_bytes())?;
            } else {
                if token.len() + character.len_utf8() > TOKEN_LIMIT {
                    return Err(Failure::usage(
                        "Artifact token exceeds the 64 KiB inspection limit.",
                    ));
                }
                token.push(character);
            }
        }
        pending = available - valid;
        input.copy_within(valid..available, 0);
    }
    flush_token(&mut token, &mut output, &mut inspection)?;
    output.flush()?;
    Ok(ObservedReceipt {
        receipt: Receipt {
            digest: format!("{:x}", output.digest.finalize()),
            size,
        },
        inspection,
    })
}

pub(super) fn copy_checked_limit(
    reader: impl Read,
    writer: impl Write,
    limit: u64,
) -> Result<Receipt, Failure> {
    let receipt = copy_checked(reader.take(limit.saturating_add(1)), writer)?;
    if receipt.size > limit {
        return Err(Failure::usage(
            "The complete bundle exceeds its byte budget.",
        ));
    }
    Ok(receipt)
}

fn flush_token(
    token: &mut String,
    output: &mut CheckedOutput<'_>,
    inspection: &mut Inspection,
) -> Result<(), Failure> {
    inspect_token(token, inspection)?;
    output.append(token.as_bytes())?;
    token.clear();
    Ok(())
}

fn inspect_token(token: &str, inspection: &mut Inspection) -> Result<(), Failure> {
    match check_token(token) {
        Err(Failure::Redaction(_)) if inspection.policy == CapturePolicy::AllowInsecure => {
            inspection.sensitivity = Sensitivity::Detected;
            Ok(())
        }
        result => result,
    }
}

pub(super) fn check_text(text: &str) -> Result<(), Failure> {
    for token in text.split(redact::is_token_separator) {
        if token.len() > TOKEN_LIMIT {
            return Err(Failure::usage(
                "Artifact token exceeds the 64 KiB inspection limit.",
            ));
        }
        check_token(token)?;
    }
    Ok(())
}

fn check_token(token: &str) -> Result<(), Failure> {
    if token.contains("-----BEGIN") {
        return Err(Failure::Redaction(Box::new(Finding {
            rule: "PEM header",
            field: "artifact content".into(),
            sample: "content omitted".into(),
            advice: "Keep credentials and personal data out of artifacts.",
        })));
    }
    if let Some(mut finding) =
        redact::tokens(token).find_map(|value| redact::check_token("artifact content", value))
    {
        finding.rule = "sensitive artifact token";
        finding.sample = "content omitted".into();
        finding.advice = "Replace sensitive values with references before capturing this artifact.";
        return Err(Failure::Redaction(Box::new(finding)));
    }
    Ok(())
}

struct CheckedOutput<'a> {
    writer: &'a mut dyn Write,
    bytes: Vec<u8>,
    digest: Sha256,
}

impl CheckedOutput<'_> {
    fn append(&mut self, bytes: &[u8]) -> Result<(), Failure> {
        if self.bytes.len() + bytes.len() > CHUNK_SIZE {
            self.flush()?;
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    fn flush(&mut self) -> Result<(), Failure> {
        self.writer.write_all(&self.bytes)?;
        self.digest.update(&self.bytes);
        self.bytes.clear();
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::io::{self, Cursor};

    struct TinyReader<'a> {
        remaining: &'a [u8],
        width: usize,
    }

    impl Read for TinyReader<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let count = buffer.len().min(self.width).min(self.remaining.len());
            buffer[..count].copy_from_slice(&self.remaining[..count]);
            self.remaining = &self.remaining[count..];
            Ok(count)
        }
    }

    fn validation_result(result: Result<(), Failure>) -> Option<(i32, String)> {
        result
            .err()
            .map(|error| (error.code(), format!("{error:?}")))
    }

    #[test]
    fn observed_preserves_sensitive_bytes_and_hash_across_read_boundaries() {
        use super::super::policy::{CapturePolicy, Coverage, Sensitivity};
        let input = "café safe (person@example.com) -----BEGIN PRIVATE KEY-----";
        for width in [1, 2, 7, CHUNK_SIZE] {
            let mut output = Vec::new();
            let observed = copy_observed(
                TinyReader {
                    remaining: input.as_bytes(),
                    width,
                },
                &mut output,
                input.len() as u64,
            )
            .unwrap();
            assert_eq!(output, input.as_bytes());
            assert_eq!(observed.receipt.size, input.len() as u64);
            assert_eq!(
                observed.receipt.digest,
                format!("{:x}", Sha256::digest(input.as_bytes()))
            );
            assert_eq!(observed.inspection.policy, CapturePolicy::AllowInsecure);
            assert_eq!(observed.inspection.sensitivity, Sensitivity::Detected);
            assert_eq!(observed.inspection.coverage, Coverage::Complete);
            assert!(observed.inspection.review_required);
            observed.inspection.validate().unwrap();
        }
    }

    #[test]
    fn observed_text_records_sensitive_content_without_changing_technical_bounds() {
        use super::super::policy::{Coverage, Sensitivity};
        let inspection = check_text_observed("safe person@example.com").unwrap();
        assert_eq!(inspection.sensitivity, Sensitivity::Detected);
        assert_eq!(inspection.coverage, Coverage::Complete);
        assert!(inspection.review_required);
        assert_eq!(
            check_text_observed(&"a".repeat(TOKEN_LIMIT + 1))
                .unwrap_err()
                .code(),
            2
        );
        let oversized = format!("person@example.com {}", "a".repeat(TOKEN_LIMIT + 1));
        assert_eq!(check_text_observed(&oversized).unwrap_err().code(), 2);
        assert_eq!(
            copy_observed(
                Cursor::new(oversized.as_bytes()),
                io::sink(),
                oversized.len() as u64
            )
            .unwrap_err()
            .code(),
            2
        );
    }

    #[test]
    fn observed_large_input_detects_the_final_token_with_bounded_memory() {
        use super::super::policy::{Coverage, Sensitivity};
        let mut input = vec![b' '; super::super::MAX_FILE_BYTES as usize];
        assert_eq!(input.len(), 100 * 1024 * 1024);
        let sensitive = b"person@example.com";
        let start = input.len() - sensitive.len();
        input[start..].copy_from_slice(sensitive);
        let expected_digest = format!("{:x}", Sha256::digest(&input));
        let (result, peak) = crate::test_alloc::measure(|| {
            copy_observed(Cursor::new(&input), io::sink(), input.len() as u64)
        });
        let observed = result.unwrap();
        assert_eq!(observed.receipt.size, input.len() as u64);
        assert_eq!(observed.receipt.digest, expected_digest);
        assert_eq!(observed.inspection.sensitivity, Sensitivity::Detected);
        assert_eq!(observed.inspection.coverage, Coverage::Complete);
        assert!(peak + 128 * 1024 <= 2 * 1024 * 1024, "peak: {peak}");
    }

    #[test]
    fn observed_matches_strict_sensitive_rules_across_full_chunk_boundaries() {
        for sensitive in [
            "person@example.com",
            "-----BEGIN PRIVATE KEY-----",
            "AKIAIOSFODNN7EXAMPLE",
        ] {
            for offset in 1..sensitive.len() {
                let mut input = vec![b' '; CHUNK_SIZE - offset];
                input.extend_from_slice(sensitive.as_bytes());
                let mut output = Vec::new();
                let observed =
                    copy_observed(Cursor::new(&input), &mut output, input.len() as u64).unwrap();
                assert_eq!(output, input);
                assert_eq!(
                    observed.inspection.sensitivity,
                    super::super::policy::Sensitivity::Detected
                );
            }
        }
    }

    #[test]
    fn observed_preserves_utf8_profile_bounds() {
        for input in [
            b"safe \xff person@example.com".as_slice(),
            b"safe \xed\xa0\x80 person@example.com",
            b"safe \xf0\x9f\xa6",
        ] {
            for width in [1, 2, CHUNK_SIZE] {
                let mut output = Vec::new();
                let error = copy_observed(
                    TinyReader {
                        remaining: input,
                        width,
                    },
                    &mut output,
                    input.len() as u64,
                )
                .unwrap_err();
                assert_eq!(error.code(), 2);
                assert!(error.message().contains("UTF-8"));
                assert!(!error.message().contains("person@example.com"));
            }
        }
    }

    #[test]
    fn observed_preserves_token_bounds_and_bounded_memory() {
        let input = vec![b'a'; 4 * TOKEN_LIMIT];
        let (result, peak) = crate::test_alloc::measure(|| {
            copy_observed(Cursor::new(&input), io::sink(), input.len() as u64)
        });
        let error = result.unwrap_err();
        assert_eq!(error.code(), 2);
        assert!(error.message().contains("64 KiB"));
        assert!(peak + 128 * 1024 < 2 * 1024 * 1024, "peak: {peak}");
        let mut input = vec![b'a'; TOKEN_LIMIT];
        input.extend_from_slice(b" person@example.com");
        let mut output = Vec::new();
        let observed = copy_observed(Cursor::new(&input), &mut output, input.len() as u64).unwrap();
        assert_eq!(output, input);
        assert_eq!(
            observed.inspection.sensitivity,
            super::super::policy::Sensitivity::Detected
        );
        assert_eq!(
            observed.inspection.coverage,
            super::super::policy::Coverage::Complete
        );
    }

    #[test]
    fn observed_enforces_limits_without_truncation_or_io_suppression() {
        let input = b"selected bytes";
        for limit in [0, input.len() as u64 - 1] {
            assert_eq!(
                copy_observed(Cursor::new(input), io::sink(), limit)
                    .unwrap_err()
                    .code(),
                2
            );
        }
        assert_eq!(
            copy_observed(Cursor::new(input), io::sink(), input.len() as u64)
                .unwrap()
                .receipt
                .size,
            input.len() as u64
        );
        assert_eq!(
            copy_observed(Cursor::new([]), io::sink(), 0)
                .unwrap()
                .receipt
                .size,
            0
        );
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("reader failed"))
            }
        }
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("writer failed"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        assert_eq!(
            copy_observed(Broken, io::sink(), 100).unwrap_err().code(),
            5
        );
        assert_eq!(
            copy_observed(Cursor::new(input), Broken, 100)
                .unwrap_err()
                .code(),
            5
        );
    }

    fn assert_text_parity(text: &str, expected: Option<i32>) {
        let direct = validation_result(check_text(text));
        assert_eq!(direct.as_ref().map(|(code, _)| *code), expected);
        for width in [1, 7, CHUNK_SIZE] {
            let streamed = validation_result(
                copy_checked(
                    TinyReader {
                        remaining: text.as_bytes(),
                        width,
                    },
                    io::sink(),
                )
                .map(|_| ()),
            );
            assert_eq!(streamed, direct, "read width {width}");
        }
    }

    #[test]
    fn bounded_text_validation_matches_streaming_for_safe_unicode_and_separators() {
        for text in [
            "",
            "Selected application data.",
            "\u{feff}Example: caf\u{e9} \u{4e2d} \u{1f980}.\r\n```rust\nlet result = 42;\n```\n",
            "one,two;three\"four'five(six)[seven]{eight}<nine>`ten`\t\r\n\u{2003}\u{2028}last",
            "src/artifacts/guard.rs:42 https://example.org/path",
            "MeetingV2PolicyMaskCalculator",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "...safe!? :reference:",
        ] {
            assert_text_parity(text, None);
        }
    }

    #[test]
    fn bounded_text_validation_preserves_sensitive_rules_and_edge_signs() {
        for sensitive in [
            "-----BEGIN CERTIFICATE-----",
            "prefix-----BEGINsuffix",
            "person@example.com",
            "C:\\Users\\someone\\project",
            "/home/someone/project",
            "ghp_16C7e42F292c6912E7710c838347Ae178B4a",
            "AKIAIOSFODNN7EXAMPLE",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVPmB92K27uhbUJU1p1r",
            "Xk7fQ2mZp9RtLw4sVb8NcJ3hGd6y",
        ] {
            assert_text_parity(sensitive, Some(3));
            assert_text_parity(&format!("safe [:.{sensitive}!?] tail"), Some(3));
        }
    }

    #[test]
    fn bounded_text_validation_counts_raw_token_bytes_before_trimming_edges() {
        for length in [TOKEN_LIMIT - 1, TOKEN_LIMIT, TOKEN_LIMIT + 1] {
            let expected = (length > TOKEN_LIMIT).then_some(2);
            assert_text_parity(&"a".repeat(length), expected);
            assert_text_parity(&".".repeat(length), expected);
            let edged = format!(".{}!", "a".repeat(length - 2));
            assert_text_parity(&edged, expected);
            let unicode = format!("{}{}", "\u{e9}".repeat(length / 2), "a".repeat(length % 2));
            assert_eq!(unicode.len(), length);
            assert_text_parity(&unicode, expected);
        }
        let separated = format!("{};{}", "a".repeat(TOKEN_LIMIT), ".".repeat(TOKEN_LIMIT));
        assert_text_parity(&separated, None);
    }

    #[test]
    fn bounded_text_validation_avoids_per_value_streaming_allocations() {
        let text = "Synthetic SQLite application fixture data. ".repeat(6);
        let (result, peak) = crate::test_alloc::measure(|| check_text(&text));
        result.unwrap();
        assert!(
            peak < 8 * 1024,
            "bounded value validation allocated {peak} bytes"
        );
    }

    #[test]
    fn preserves_original_bytes_and_hash_across_tiny_reads() {
        let input = "\u{feff}Example: café 中 🦀.\r\n```rust\nlet result = 42;\n```\n";
        for width in 1..=7 {
            let mut output = Vec::new();
            let receipt = copy_checked(
                TinyReader {
                    remaining: input.as_bytes(),
                    width,
                },
                &mut output,
            )
            .unwrap();
            assert_eq!(output, input.as_bytes());
            assert_eq!(receipt.size, input.len() as u64);
            assert_eq!(
                receipt.digest,
                format!("{:x}", Sha256::digest(input.as_bytes()))
            );
        }
        assert!(crate::redact::check_field("note", input).is_some());
    }

    #[test]
    fn never_writes_sensitive_tokens_across_any_read_boundary() {
        for sensitive in [
            "ghp_16C7e42F292c6912E7710c838347Ae178B4a",
            "person@example.com",
            "C:\\Users\\someone\\project",
            "/home/someone/project",
            "AKIAIOSFODNN7EXAMPLE",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVPmB92K27uhbUJU1p1r",
            "Xk7fQ2mZp9RtLw4sVb8NcJ3hGd6y",
        ] {
            let input = format!("safe (.{sensitive}!) tail");
            for width in 1..=input.len() {
                let mut output = Vec::new();
                let failure = copy_checked(
                    TinyReader {
                        remaining: input.as_bytes(),
                        width,
                    },
                    &mut output,
                )
                .unwrap_err();
                assert_eq!(failure.code(), 3);
                assert!(!failure.message().contains(&sensitive[..4]));
                assert!(!failure.message().contains("********"));
                assert!(b"safe (".starts_with(&output));
            }
        }
    }

    #[test]
    fn rejects_all_pem_headers_before_writing_them() {
        for header in ["-----BEGIN CERTIFICATE-----", "-----BEGIN PRIVATE KEY-----"] {
            let input = format!("safe {header}\nbody");
            for width in 1..=input.len() {
                let mut output = Vec::new();
                assert_eq!(
                    copy_checked(
                        TinyReader {
                            remaining: input.as_bytes(),
                            width
                        },
                        &mut output,
                    )
                    .unwrap_err()
                    .code(),
                    3
                );
                assert!(b"safe ".starts_with(&output));
            }
        }
    }

    #[test]
    fn rejects_invalid_or_unfinished_utf8() {
        for bytes in [
            b"safe \xff".as_slice(),
            b"safe \xf0\x9f\xa6",
            b"\xed\xa0\x80",
            b"\xc0\xaf",
        ] {
            for width in 1..=bytes.len() {
                let mut output = Vec::new();
                let error = copy_checked(
                    TinyReader {
                        remaining: bytes,
                        width,
                    },
                    &mut output,
                )
                .unwrap_err();
                assert!(error.message().contains("UTF-8"));
                assert!(std::str::from_utf8(&output).is_ok());
            }
        }
    }

    #[test]
    fn enforces_exact_size_and_token_limits() {
        const LIMIT: usize = super::super::MAX_FILE_BYTES as usize;
        let input = vec![b' '; LIMIT];
        let mut output = Vec::new();
        assert_eq!(
            copy_checked(Cursor::new(&input), &mut output).unwrap().size,
            LIMIT as u64
        );
        assert_eq!(output, input);
        let oversized = vec![b' '; LIMIT + 1];
        assert!(copy_checked(Cursor::new(oversized), io::sink())
            .unwrap_err()
            .message()
            .contains(&format!("{LIMIT} byte limit")));
        let token = vec![b'a'; 64 * 1024];
        let mut output = Vec::new();
        copy_checked(Cursor::new(&token), &mut output).unwrap();
        assert_eq!(output, token);
        let token = vec![b'a'; 64 * 1024 + 1];
        let mut output = Vec::new();
        assert!(copy_checked(Cursor::new(token), &mut output)
            .unwrap_err()
            .message()
            .contains("token"));
        assert!(output.is_empty());
    }

    #[test]
    fn holds_sensitive_tokens_across_full_chunk_boundaries() {
        let sensitive = "person@example.com";
        for offset in 1..sensitive.len() {
            let mut input = vec![b' '; 64 * 1024 - offset];
            input.extend_from_slice(sensitive.as_bytes());
            input.push(b' ');
            let mut output = Vec::new();
            assert_eq!(
                copy_checked(Cursor::new(&input), &mut output)
                    .unwrap_err()
                    .code(),
                3
            );
            assert!(output.iter().all(|byte| *byte == b' '));
        }
        for text in ["é", "中", "🦀"] {
            for offset in 1..text.len() {
                let mut input = vec![b' '; 64 * 1024 - offset];
                input.extend_from_slice(text.as_bytes());
                let mut output = Vec::new();
                copy_checked(Cursor::new(&input), &mut output).unwrap();
                assert_eq!(output, input);
            }
        }
    }
    #[test]
    fn propagates_io_failures() {
        struct BrokenReader;
        impl Read for BrokenReader {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("reader failed"))
            }
        }
        struct BrokenWriter;
        impl Write for BrokenWriter {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("writer failed"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        assert_eq!(
            copy_checked(BrokenReader, io::sink()).unwrap_err().code(),
            5
        );
        assert_eq!(
            copy_checked(Cursor::new(b"safe"), BrokenWriter)
                .unwrap_err()
                .code(),
            5
        );
    }

    #[test]
    fn maximum_input_stays_within_the_auxiliary_memory_budget() {
        let mut input = vec![b' '; super::super::MAX_FILE_BYTES as usize];
        assert_eq!(input.len(), 100 * 1024 * 1024);
        input[..64 * 1024].fill(b'a');
        for (offset, text) in [
            (2 * 64 * 1024 - 1, "🦀"),
            (3 * 64 * 1024 - 2, "中"),
            (4 * 64 * 1024 - 1, "é"),
        ] {
            input[offset..offset + text.len()].copy_from_slice(text.as_bytes());
        }
        let expected_digest = format!("{:x}", Sha256::digest(&input));
        let (result, peak) =
            crate::test_alloc::measure(|| copy_checked(Cursor::new(&input), io::sink()));
        let receipt = result.unwrap();
        assert_eq!(receipt.size, input.len() as u64);
        assert_eq!(receipt.digest, expected_digest);
        eprintln!(
            "Artifact guard peak auxiliary heap: {peak} bytes; stack allowance: 131072 bytes."
        );
        assert!(
            peak + 128 * 1024 <= 2 * 1024 * 1024,
            "auxiliary allocation peak: {peak} bytes"
        );
    }
}
