use super::Budget;
use crate::failure::Failure;
use flate2::{Decompress, FlushDecompress, Status};
use png::{chunk, DecodeOptions, Decoded, StreamingDecoder};
use std::io::{BufReader, Read, Seek, SeekFrom};

const MAX_METADATA: usize = 64 * 1024;
const MAX_CHUNKS: usize = 1024;
const MAX_DIMENSION: u32 = 4096;
const MAX_PIXELS: u64 = 16_777_216;

fn refused() -> Failure {
    Failure::usage("PNG content does not satisfy the reviewed media profile.")
}

fn strict_options() -> DecodeOptions {
    let mut options = DecodeOptions::default();
    options.set_ignore_adler32(false);
    options.set_ignore_crc(false);
    options.set_skip_ancillary_crc_failures(false);
    options
}

fn check_text(value: &str, total: &mut usize, budget: &mut Budget) -> Result<(), Failure> {
    *total = total.checked_add(value.len()).ok_or_else(refused)?;
    if *total > MAX_METADATA
        || value
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(refused());
    }
    budget.check_text(value)
}

fn inspect_text(
    decoder: &StreamingDecoder,
    kind: chunk::ChunkType,
    total: &mut usize,
    budget: &mut Budget,
) -> Result<(), Failure> {
    let info = decoder.info().ok_or_else(refused)?;
    match kind {
        chunk::tEXt => {
            let text = info.uncompressed_latin1_text.last().ok_or_else(refused)?;
            check_text(&text.keyword, total, budget)?;
            check_text(&text.text, total, budget)?;
        }
        chunk::zTXt => {
            let mut text = info
                .compressed_latin1_text
                .last()
                .ok_or_else(refused)?
                .clone();
            check_text(&text.keyword, total, budget)?;
            text.decompress_text_with_limit(MAX_METADATA - *total)
                .map_err(|_| refused())?;
            check_text(&text.get_text().map_err(|_| refused())?, total, budget)?;
        }
        chunk::iTXt => {
            let mut text = info.utf8_text.last().ok_or_else(refused)?.clone();
            check_text(&text.keyword, total, budget)?;
            check_text(&text.language_tag, total, budget)?;
            check_text(&text.translated_keyword, total, budget)?;
            text.decompress_text_with_limit(MAX_METADATA - *total)
                .map_err(|_| refused())?;
            check_text(&text.get_text().map_err(|_| refused())?, total, budget)?;
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
fn inspect(reader: &mut (impl Read + Seek)) -> Result<(), Failure> {
    inspect_with_budget(reader, &mut Budget::default())
}

pub(super) fn inspect_with_budget(
    reader: &mut (impl Read + Seek),
    budget: &mut Budget,
) -> Result<(), Failure> {
    reader.seek(SeekFrom::Start(0))?;
    let mut decoder = StreamingDecoder::new_with_options(strict_options());
    let mut buffer = [0; 8192];
    let mut offset = 0u64;
    let mut chunks = 0usize;
    let mut metadata = 0usize;
    let mut text_bytes = 0usize;
    let mut unique = Vec::new();
    let mut spans = Vec::new();
    let mut ended = false;
    while !ended {
        let read_limit = if offset < 33 {
            (33 - offset) as usize
        } else {
            buffer.len()
        };
        let count = reader.read(&mut buffer[..read_limit])?;
        if count == 0 {
            return Err(refused());
        }
        let mut consumed = 0;
        while consumed < count {
            let (used, event) = decoder
                .update(&buffer[consumed..count], None)
                .map_err(|_| refused())?;
            consumed += used;
            offset += used as u64;
            match event {
                Decoded::ChunkBegin(length, kind) => {
                    chunks += 1;
                    if chunks > MAX_CHUNKS
                        || !matches!(
                            kind,
                            chunk::IHDR
                                | chunk::PLTE
                                | chunk::IDAT
                                | chunk::IEND
                                | chunk::tEXt
                                | chunk::zTXt
                                | chunk::iTXt
                                | chunk::gAMA
                                | chunk::cHRM
                                | chunk::sRGB
                                | chunk::pHYs
                                | chunk::sBIT
                                | chunk::bKGD
                                | chunk::tRNS
                        )
                    {
                        return Err(refused());
                    }
                    if kind == chunk::IDAT {
                        spans.push((offset, u64::from(length)));
                    } else {
                        metadata = metadata.checked_add(length as usize).ok_or_else(refused)?;
                        if metadata > MAX_METADATA {
                            return Err(refused());
                        }
                        if !matches!(kind, chunk::tEXt | chunk::zTXt | chunk::iTXt) {
                            if unique.contains(&kind) {
                                return Err(refused());
                            }
                            unique.push(kind);
                        }
                    }
                }
                Decoded::ChunkComplete(kind) => {
                    if kind == chunk::IHDR {
                        let info = decoder.info().ok_or_else(refused)?;
                        if info.width > MAX_DIMENSION
                            || info.height > MAX_DIMENSION
                            || u64::from(info.width) * u64::from(info.height) > MAX_PIXELS
                            || info.interlaced
                        {
                            return Err(refused());
                        }
                        let expanded = (info.raw_row_length() as u64)
                            .checked_mul(u64::from(info.height))
                            .ok_or_else(refused)?;
                        budget.decoded_remaining = budget
                            .decoded_remaining
                            .checked_sub(expanded)
                            .ok_or_else(refused)?;
                    }
                    inspect_text(&decoder, kind, &mut text_bytes, budget)?;
                    if kind == chunk::IEND {
                        if consumed != count || reader.read(&mut buffer[..1])? != 0 {
                            return Err(refused());
                        }
                        ended = true;
                        break;
                    }
                }
                Decoded::BadAncillaryChunk(_) | Decoded::SkippedAncillaryChunk(_) => {
                    return Err(refused());
                }
                Decoded::Nothing if used == 0 => return Err(refused()),
                _ => {}
            }
        }
    }
    let info = decoder.info().ok_or_else(refused)?;
    let expected = (info.raw_row_length() as u64)
        .checked_mul(u64::from(info.height))
        .ok_or_else(refused)?;
    drop(decoder);
    inspect_zlib(reader, &spans, expected)?;
    reader.seek(SeekFrom::Start(0))?;
    let mut decoder =
        png::Decoder::new_with_options(BufReader::with_capacity(8192, reader), strict_options());
    decoder.set_limits(png::Limits { bytes: 512 * 1024 });
    let mut image = decoder.read_info().map_err(|_| refused())?;
    while image.next_row().map_err(|_| refused())?.is_some() {}
    image.finish().map_err(|_| refused())
}

fn inspect_zlib(
    reader: &mut (impl Read + Seek),
    spans: &[(u64, u64)],
    expected_output: u64,
) -> Result<(), Failure> {
    let expected_input = spans.iter().try_fold(0u64, |total, (_, length)| {
        total.checked_add(*length).ok_or_else(refused)
    })?;
    let mut stream = Decompress::new(true);
    let mut input = [0; 8192];
    let mut output = [0; 8192];
    for &(offset, length) in spans {
        reader.seek(SeekFrom::Start(offset))?;
        let mut remaining = length;
        while remaining != 0 {
            let count = remaining.min(input.len() as u64) as usize;
            reader.read_exact(&mut input[..count])?;
            remaining -= count as u64;
            let mut used = 0;
            while used < count {
                let before_in = stream.total_in();
                let before_out = stream.total_out();
                let status = stream
                    .decompress(&input[used..count], &mut output, FlushDecompress::None)
                    .map_err(|_| refused())?;
                used += (stream.total_in() - before_in) as usize;
                if stream.total_out() > expected_output {
                    return Err(refused());
                }
                if status == Status::StreamEnd {
                    return if stream.total_in() == expected_input
                        && stream.total_out() == expected_output
                    {
                        Ok(())
                    } else {
                        Err(refused())
                    };
                }
                if before_in == stream.total_in() && before_out == stream.total_out() {
                    return Err(refused());
                }
            }
        }
    }
    loop {
        let before_out = stream.total_out();
        let status = stream
            .decompress(&[], &mut output, FlushDecompress::Finish)
            .map_err(|_| refused())?;
        if stream.total_out() > expected_output {
            return Err(refused());
        }
        if status == Status::StreamEnd {
            return if stream.total_in() == expected_input && stream.total_out() == expected_output {
                Ok(())
            } else {
                Err(refused())
            };
        }
        if before_out == stream.total_out() {
            return Err(refused());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const SAMPLE: &[u8] = include_bytes!("../../../tests/data/artifact-media/sample.png");

    #[test]
    fn policy_permissive_png_retains_sensitive_metadata_without_relaxing_structure() {
        let bytes = image(&[(*b"tEXt", b"Contact\0person@example.com")]);
        let mut output = Vec::new();
        assert!(super::super::copy_checked(
            Cursor::new(&bytes),
            &mut output,
            "image/png",
            bytes.len() as u64
        )
        .is_err());
        let observed = super::super::copy_with_policy_with_budget(
            Cursor::new(&bytes),
            &mut output,
            "image/png",
            bytes.len() as u64,
            &mut Budget::default(),
            super::super::super::policy::CapturePolicy::AllowInsecure,
        )
        .unwrap();
        assert_eq!(output, bytes);
        assert_eq!(
            observed.inspection.sensitivity,
            super::super::super::policy::Sensitivity::Detected
        );
        assert_eq!(
            observed.inspection.coverage,
            super::super::super::policy::Coverage::Partial
        );
        let mut malformed = bytes;
        malformed.extend_from_slice(b"trailing");
        assert!(super::super::copy_with_policy_with_budget(
            Cursor::new(&malformed),
            std::io::sink(),
            "image/png",
            malformed.len() as u64,
            &mut Budget::default(),
            super::super::super::policy::CapturePolicy::AllowInsecure
        )
        .is_err());
    }
    #[test]
    fn refuses_scanline_bomb_before_reading_image_data() {
        struct HeaderOnly(Cursor<Vec<u8>>);

        impl Read for HeaderOnly {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                assert!(self.0.position() < 33, "image data must not be read");
                let count = buffer.len().min(33 - self.0.position() as usize);
                self.0.read(&mut buffer[..count])
            }
        }

        impl Seek for HeaderOnly {
            fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
                self.0.seek(position)
            }
        }

        let mut output = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut output, 4096, 4096);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Sixteen);
            encoder.write_header().unwrap().finish().unwrap();
        }
        assert!(inspect(&mut HeaderOnly(Cursor::new(output))).is_err());
    }

    fn image(chunks: &[([u8; 4], &[u8])]) -> Vec<u8> {
        let mut output = Vec::new();
        {
            let encoder = png::Encoder::new(&mut output, 1, 1);
            let mut writer = encoder.write_header().unwrap();
            for (kind, data) in chunks {
                writer
                    .write_chunk(png::chunk::ChunkType(*kind), data)
                    .unwrap();
            }
            writer.write_image_data(&[0]).unwrap();
            writer.finish().unwrap();
        }
        output
    }

    #[test]
    fn accepts_reviewable_png() {
        inspect(&mut Cursor::new(SAMPLE)).unwrap();
        inspect(&mut Cursor::new(image(&[(
            *b"tEXt",
            b"Title\0A quiet scene",
        )])))
        .unwrap();
        let mut output = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut output, 1, 1);
            encoder
                .add_ztxt_chunk("Title".into(), "A quiet scene".into())
                .unwrap();
            let mut writer = encoder.write_header().unwrap();
            let mut text = png::text_metadata::ITXtChunk::new("Title", "A quiet scene");
            text.compressed = true;
            text.language_tag = "en".into();
            text.translated_keyword = "Title".into();
            writer.write_text_chunk(&text).unwrap();
            writer.write_image_data(&[0]).unwrap();
            writer.finish().unwrap();
        }
        inspect(&mut Cursor::new(output)).unwrap();
    }

    #[test]
    fn refuses_corrupt_truncated_and_trailing_png() {
        let mut corrupt = SAMPLE.to_vec();
        corrupt[29] ^= 1;
        assert!(inspect(&mut Cursor::new(corrupt)).is_err());
        for end in [0, 8, 20, SAMPLE.len() - 1, SAMPLE.len() - 12] {
            assert!(inspect(&mut Cursor::new(&SAMPLE[..end])).is_err());
        }
        let mut trailing = SAMPLE.to_vec();
        trailing.push(0);
        assert!(inspect(&mut Cursor::new(trailing)).is_err());
        let mut ancillary = image(&[(*b"tEXt", b"Title\0A quiet scene")]);
        let location = ancillary
            .windows(13)
            .position(|part| part == b"A quiet scene")
            .unwrap();
        ancillary[location] ^= 1;
        assert!(inspect(&mut Cursor::new(ancillary)).is_err());
    }

    #[test]
    fn refuses_duplicate_metadata_and_interlacing() {
        let gamma = 45455u32.to_be_bytes();
        assert!(inspect(&mut Cursor::new(image(&[
            (*b"gAMA", &gamma),
            (*b"gAMA", &gamma)
        ])))
        .is_err());
        let mut output = Vec::new();
        {
            let mut info = png::Info::with_size(1, 1);
            info.interlaced = true;
            let encoder = png::Encoder::with_info(&mut output, info).unwrap();
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[0]).unwrap();
            writer.finish().unwrap();
        }
        assert!(inspect(&mut Cursor::new(output)).is_err());
    }

    #[test]
    fn checks_image_adler_and_decoded_scanlines() {
        // A zlib stream containing the filter byte and one grayscale pixel.
        let valid = [0x78, 0x9c, 0x63, 0x60, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01];
        for corrupt in [false, true] {
            let mut data = valid;
            if corrupt {
                data[9] ^= 1;
            }
            let mut output = Vec::new();
            {
                let encoder = png::Encoder::new(&mut output, 1, 1);
                let mut writer = encoder.write_header().unwrap();
                writer.write_chunk(png::chunk::IDAT, &data).unwrap();
                writer.finish().unwrap();
            }
            assert_eq!(inspect(&mut Cursor::new(output)).is_err(), corrupt);
        }
        let mut output = Vec::new();
        {
            let encoder = png::Encoder::new(&mut output, 1, 2);
            let mut writer = encoder.write_header().unwrap();
            writer.write_chunk(png::chunk::IDAT, &valid).unwrap();
            writer.finish().unwrap();
        }
        assert!(inspect(&mut Cursor::new(output)).is_err());
    }

    #[test]
    fn refuses_hidden_idat_suffix_and_truncated_zlib_trailer() {
        let valid = [0x78, 0x9c, 0x63, 0x60, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01];
        let mut extra = valid.to_vec();
        extra.extend_from_slice(b"hidden content");
        for data in [extra.as_slice(), &valid[..8]] {
            let mut output = Vec::new();
            {
                let encoder = png::Encoder::new(&mut output, 1, 1);
                let mut writer = encoder.write_header().unwrap();
                writer.write_chunk(png::chunk::IDAT, data).unwrap();
                writer.finish().unwrap();
            }
            assert!(inspect(&mut Cursor::new(output)).is_err());
        }
    }

    #[test]
    fn refuses_unknown_and_sensitive_metadata_chunks() {
        for kind in [
            *b"vpAg", *b"eXIf", *b"iCCP", *b"acTL", *b"fcTL", *b"fdAT", *b"tIME",
        ] {
            assert!(inspect(&mut Cursor::new(image(&[(kind, b"private")]))).is_err());
        }
        for text in [
            b"Title\0person@example.com".as_slice(),
            b"person@example.com\0scene",
            b"Title\0bad\0value",
        ] {
            assert!(inspect(&mut Cursor::new(image(&[(*b"tEXt", text)]))).is_err());
        }
    }

    #[test]
    fn refuses_sensitive_compressed_text_and_expansion() {
        for international in [false, true] {
            for text in ["person@example.com".to_owned(), "a ".repeat(40_000)] {
                let mut output = Vec::new();
                {
                    let mut encoder = png::Encoder::new(&mut output, 1, 1);
                    if international {
                        let mut chunk = png::text_metadata::ITXtChunk::new("Title", text);
                        chunk.compressed = true;
                        let mut writer = encoder.write_header().unwrap();
                        writer.write_text_chunk(&chunk).unwrap();
                        writer.write_image_data(&[0]).unwrap();
                        writer.finish().unwrap();
                    } else {
                        encoder.add_ztxt_chunk("Title".into(), text).unwrap();
                        let mut writer = encoder.write_header().unwrap();
                        writer.write_image_data(&[0]).unwrap();
                        writer.finish().unwrap();
                    }
                }
                assert!(inspect(&mut Cursor::new(output)).is_err());
            }
        }
    }

    #[test]
    fn refuses_sensitive_international_fields() {
        for data in [
            b"Title\0\0\0en\0person@example.com\0scene".as_slice(),
            b"Title\0\0\0person@example.com\0Title\0scene",
            b"Title\0\0\0en\0Title\0person@example.com",
        ] {
            assert!(inspect(&mut Cursor::new(image(&[(*b"iTXt", data)]))).is_err());
        }
    }

    #[test]
    fn refuses_metadata_and_dimension_bombs() {
        let large = vec![b'a'; 65_537];
        assert!(inspect(&mut Cursor::new(image(&[(*b"tEXt", &large)]))).is_err());
        let chunks = vec![(*b"tEXt", b"Title\0scene".as_slice()); 1024];
        assert!(inspect(&mut Cursor::new(image(&chunks))).is_err());
        let mut output = Vec::new();
        {
            let encoder = png::Encoder::new(&mut output, 4097, 1);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&vec![0; 4097]).unwrap();
            writer.finish().unwrap();
        }
        assert!(inspect(&mut Cursor::new(output)).is_err());
    }
}
