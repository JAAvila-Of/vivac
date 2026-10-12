use std::io::{Read, Write};

use sha2::{Digest, Sha256};

use crate::failure::Failure;
use crate::redact::{self, Finding};

const CHUNK_SIZE: usize = 64 * 1024;
const TOKEN_LIMIT: usize = 64 * 1024;
const SIZE_LIMIT: u64 = 10 * 1024 * 1024;

#[derive(Debug)]
pub struct Receipt {
    pub digest: String,
    pub size: u64,
}

pub fn copy_checked(mut reader: impl Read, mut writer: impl Write) -> Result<Receipt, Failure> {
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
        if size > SIZE_LIMIT {
            return Err(Failure::usage("Artifact content exceeds the 10 MiB limit."));
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
                flush_token(&mut token, &mut output)?;
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
    flush_token(&mut token, &mut output)?;
    output.flush()?;
    Ok(Receipt {
        digest: format!("{:x}", output.digest.finalize()),
        size,
    })
}

fn flush_token(token: &mut String, output: &mut CheckedOutput<'_>) -> Result<(), Failure> {
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
    output.append(token.as_bytes())?;
    token.clear();
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
        const LIMIT: usize = 10 * 1024 * 1024;
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
            .contains("10 MiB"));
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
        let mut input = vec![b' '; 10 * 1024 * 1024];
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
