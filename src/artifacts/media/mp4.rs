use std::io::{Read, Seek, SeekFrom};

use h264_reader::avcc::AvcDecoderConfigurationRecord;
use h264_reader::nal::{pps::PicParameterSet, sps::SeqParameterSet, Nal, RefNal};
use h264_reader::rbsp::BitRead;
use mp4_atom::{Any, Decode, DecodeAtom, Header, ReadFrom};

use super::{Budget, MAX_VIDEO_SAMPLES};
use crate::failure::Failure;

const MAX_METADATA: usize = 64 * 1024;

fn unsupported() -> Failure {
    Failure::usage("Unsupported or invalid reviewed MP4 structure.")
}

fn require(condition: bool) -> Result<(), Failure> {
    if condition {
        Ok(())
    } else {
        Err(unsupported())
    }
}

fn word(bytes: &[u8], offset: usize) -> Result<u32, Failure> {
    let value = bytes.get(offset..offset + 4).ok_or_else(unsupported)?;
    Ok(u32::from_be_bytes(
        value.try_into().map_err(|_| unsupported())?,
    ))
}

fn long(bytes: &[u8], offset: usize) -> Result<u64, Failure> {
    let value = bytes.get(offset..offset + 8).ok_or_else(unsupported)?;
    Ok(u64::from_be_bytes(
        value.try_into().map_err(|_| unsupported())?,
    ))
}

fn text(bytes: &[u8]) -> Result<(), Failure> {
    let value = std::str::from_utf8(bytes).map_err(|_| unsupported())?;
    value
        .split('\0')
        .try_for_each(super::super::guard::check_text)
}

fn typed(header: &Header, mut bytes: &[u8]) -> Result<Any, Failure> {
    Any::decode_atom(header, &mut bytes).map_err(|_| unsupported())
}

#[derive(Default)]
struct Movie<'a> {
    boxes: usize,
    sizes: Option<&'a [u8]>,
    chunks: Option<(&'a [u8], bool)>,
    mapping: Option<&'a [u8]>,
    timing: Option<&'a [u8]>,
    composition: Option<&'a [u8]>,
    sync: Option<&'a [u8]>,
    timescale: u32,
    movie_timescale: u32,
    dimensions: (u32, u32),
}

#[cfg(test)]
fn inspect(reader: &mut (impl Read + Seek)) -> Result<(), Failure> {
    inspect_with_budget(reader, &mut Budget::default())
}

pub(super) fn inspect_with_budget(
    reader: &mut (impl Read + Seek),
    budget: &mut Budget,
) -> Result<(), Failure> {
    let length = reader.seek(SeekFrom::End(0))?;
    require(length > 0 && length <= 10 * 1024 * 1024)?;
    reader.seek(SeekFrom::Start(0))?;
    let mut metadata = None;
    let mut media = None;
    let mut brands = false;
    let mut count = 0;
    while reader.stream_position()? < length {
        count += 1;
        require(count <= 16)?;
        let header_start = reader.stream_position()?;
        let header = Header::read_from(reader).map_err(|_| unsupported())?;
        let size = header.size.ok_or_else(unsupported)?;
        let start = reader.stream_position()?;
        require(start - header_start == 8)?;
        let end = start.checked_add(size as u64).ok_or_else(unsupported)?;
        require(end <= length)?;
        match <[u8; 4]>::from(header.kind) {
            kind if kind == *b"ftyp" => {
                require(!brands && start <= 16 && (8..=256).contains(&size) && size % 4 == 0)?;
                let mut bytes = vec![0; size];
                reader.read_exact(&mut bytes)?;
                for brand in std::iter::once(&bytes[..4]).chain(bytes[8..].chunks_exact(4)) {
                    require(matches!(
                        brand,
                        b"isom" | b"iso2" | b"avc1" | b"mp41" | b"mp42"
                    ))?;
                }
                typed(&header, &bytes)?;
                brands = true;
            }
            kind if kind == *b"moov" => {
                require(metadata.is_none() && size <= MAX_METADATA)?;
                let mut bytes = vec![0; size];
                reader.read_exact(&mut bytes)?;
                metadata = Some(bytes);
            }
            kind if kind == *b"mdat" => {
                require(media.is_none() && size > 0)?;
                media = Some((start, end));
                reader.seek(SeekFrom::Start(end))?;
            }
            kind if kind == *b"free" => {
                require(size <= MAX_METADATA)?;
                let mut buffer = [0; 1024];
                let mut remaining = size;
                while remaining > 0 {
                    let size = remaining.min(buffer.len());
                    reader.read_exact(&mut buffer[..size])?;
                    require(buffer[..size].iter().all(|byte| *byte == 0))?;
                    remaining -= size;
                }
            }
            _ => return Err(unsupported()),
        }
    }
    require(brands)?;
    let metadata = metadata.ok_or_else(unsupported)?;
    let mut movie = Movie::default();
    movie.walk(*b"moov", &metadata, 0, budget)?;
    movie.samples(reader, media.ok_or_else(unsupported)?, budget)
}

impl<'a> Movie<'a> {
    fn walk(
        &mut self,
        parent: [u8; 4],
        mut bytes: &'a [u8],
        depth: usize,
        budget: &mut Budget,
    ) -> Result<(), Failure> {
        require(depth <= 12)?;
        let mut seen = Vec::new();
        while !bytes.is_empty() {
            let header_start = bytes.len();
            let header = Header::decode(&mut bytes).map_err(|_| unsupported())?;
            require(header_start - bytes.len() == 8)?;
            let size = header.size.ok_or_else(unsupported)?;
            require(size <= bytes.len())?;
            let body = &bytes[..size];
            bytes = &bytes[size..];
            self.boxes += 1;
            require(self.boxes <= 1024)?;
            let kind: [u8; 4] = header.kind.into();
            require(!seen.contains(&kind))?;
            seen.push(kind);
            let allowed: &[&[u8; 4]] = match &parent {
                b"moov" => &[b"mvhd", b"trak", b"udta"],
                b"trak" => &[b"tkhd", b"edts", b"mdia"],
                b"mdia" => &[b"mdhd", b"hdlr", b"minf"],
                b"minf" => &[b"vmhd", b"dinf", b"stbl"],
                b"stbl" => &[
                    b"stsd", b"stts", b"ctts", b"stss", b"stsc", b"stsz", b"stco", b"co64",
                ],
                b"edts" => &[b"elst"],
                b"dinf" => &[b"dref"],
                b"dref" => &[b"url "],
                b"stsd" => &[b"avc1"],
                b"avc1" => &[b"avcC", b"pasp", b"btrt", b"colr"],
                b"udta" => &[b"meta"],
                b"meta" => &[b"hdlr", b"ilst"],
                b"ilst" => &[b"\xa9too"],
                b"\xa9too" => &[b"data"],
                _ => return Err(unsupported()),
            };
            let name = allowed
                .iter()
                .find(|name| kind == ***name)
                .ok_or_else(unsupported)?;
            match *name {
                name @ (b"trak" | b"mdia" | b"minf" | b"stbl" | b"edts" | b"dinf" | b"udta"
                | b"ilst" | b"\xa9too") => {
                    self.walk(*name, body, depth + 1, budget)?;
                }
                b"meta" => {
                    require(word(body, 0)? == 0)?;
                    self.walk(*b"meta", &body[4..], depth + 1, budget)?;
                }
                name @ (b"dref" | b"stsd") => {
                    require(word(body, 0)? == 0 && word(body, 4)? == 1)?;
                    self.walk(*name, &body[8..], depth + 1, budget)?;
                }
                b"avc1" => {
                    require(body.len() >= 78 && body[..6] == [0; 6] && body[6..8] == [0, 1])?;
                    require(body[8..24] == [0; 16] && body[36..40] == [0; 4])?;
                    let width = u16::from_be_bytes([body[24], body[25]]) as u32;
                    let height = u16::from_be_bytes([body[26], body[27]]) as u32;
                    dimensions(width, height)?;
                    self.dimensions = (width, height);
                    require(body[40..42] == [0, 1] && body[74..78] == [0, 24, 255, 255])?;
                    let length = body[42] as usize;
                    require(length <= 31 && body[43 + length..74].iter().all(|byte| *byte == 0))?;
                    text(&body[43..43 + length])?;
                    let mut visual = &body[..78];
                    mp4_atom::Visual::decode(&mut visual).map_err(|_| unsupported())?;
                    self.walk(*b"avc1", &body[78..], depth + 1, budget)?;
                }
                b"hdlr" => {
                    require(body.len() >= 24 && word(body, 0)? == 0 && word(body, 4)? == 0)?;
                    let handler = if parent == *b"mdia" { b"vide" } else { b"mdir" };
                    require(&body[8..12] == handler)?;
                    let reserved = &body[12..24];
                    require(if parent == *b"mdia" {
                        reserved == [0; 12]
                    } else {
                        reserved[..4] == *b"appl" && reserved[4..] == [0; 8]
                    })?;
                    let name = &body[24..];
                    let end = name
                        .iter()
                        .position(|byte| *byte == 0)
                        .ok_or_else(unsupported)?;
                    require(name[end..].iter().all(|byte| *byte == 0))?;
                    text(&name[..end])?;
                    typed(&header, body)?;
                }
                b"data" => {
                    require(word(body, 0)? == 1 && word(body, 4)? == 0)?;
                    text(&body[8..])?;
                }
                b"url " => require(body == [0, 0, 0, 1])?,
                b"avcC" => self.avcc(&header, body)?,
                b"colr" => {
                    require(
                        (body.len() == 11 && &body[..4] == b"nclx")
                            || (body.len() == 10 && &body[..4] == b"nclc"),
                    )?;
                    typed(&header, body)?;
                }
                b"stsz" => {
                    require(word(body, 0)? == 0)?;
                    let count = word(body, 8)? as usize;
                    require((1..=MAX_VIDEO_SAMPLES).contains(&count))?;
                    budget.sample_remaining = budget
                        .sample_remaining
                        .checked_sub(count)
                        .ok_or_else(unsupported)?;
                    let expected = if word(body, 4)? == 0 {
                        12 + count * 4
                    } else {
                        12
                    };
                    require(body.len() == expected)?;
                    self.sizes = Some(body);
                    typed(&header, body)?;
                }
                name @ (b"stco" | b"co64" | b"stsc" | b"stts" | b"ctts" | b"stss" | b"elst") => {
                    let flags = word(body, 0)?;
                    require(flags == 0 || (*name == *b"ctts" && flags == 0x01000000))?;
                    let count = word(body, 4)? as usize;
                    require(count <= MAX_VIDEO_SAMPLES)?;
                    let stride = match name {
                        b"co64" | b"stts" | b"ctts" => 8,
                        b"stsc" | b"elst" => 12,
                        _ => 4,
                    };
                    require(body.len() == 8 + count * stride)?;
                    match name {
                        b"stco" | b"co64" => {
                            require(self.chunks.is_none())?;
                            self.chunks = Some((body, *name == *b"co64"));
                        }
                        b"stsc" => self.mapping = Some(body),
                        b"stts" => self.timing = Some(body),
                        b"ctts" => self.composition = Some(body),
                        b"stss" => self.sync = Some(body),
                        b"elst" => {
                            require(count <= 2)?;
                            for entry in body[8..].chunks_exact(12) {
                                require(
                                    word(entry, 0)? as u64 <= u64::from(self.movie_timescale) * 300
                                        && word(entry, 8)? == 0x10000,
                                )?;
                            }
                        }
                        _ => return Err(unsupported()),
                    }
                    typed(&header, body)?;
                }
                b"mvhd" => {
                    require(body.len() == 100 && word(body, 0)? == 0)?;
                    require(body[26..36] == [0; 10] && body[72..96] == [0; 24])?;
                    self.movie_timescale = word(body, 12)?;
                    require(
                        self.movie_timescale > 0
                            && word(body, 16)? as u64 <= u64::from(self.movie_timescale) * 300,
                    )?;
                    typed(&header, body)?;
                }
                b"mdhd" => {
                    require(body.len() == 24 && word(body, 0)? == 0 && body[22..24] == [0; 2])?;
                    self.timescale = word(body, 12)?;
                    require(
                        self.timescale > 0
                            && word(body, 16)? as u64 <= u64::from(self.timescale) * 300,
                    )?;
                    typed(&header, body)?;
                }
                b"tkhd" => {
                    require(body.len() == 84 && word(body, 0)? & !7 == 0)?;
                    require(
                        body[16..20] == [0; 4] && body[24..32] == [0; 8] && body[38..40] == [0; 2],
                    )?;
                    dimensions(word(body, 76)? >> 16, word(body, 80)? >> 16)?;
                    typed(&header, body)?;
                }
                b"vmhd" => {
                    require(body.len() == 12 && word(body, 0)? == 1 && body[4..] == [0; 8])?;
                    typed(&header, body)?;
                }
                b"pasp" => {
                    require(body.len() == 8 && word(body, 0)? > 0 && word(body, 4)? > 0)?;
                    typed(&header, body)?;
                }
                b"btrt" => {
                    require(body.len() == 12)?;
                    typed(&header, body)?;
                }
                _ => return Err(unsupported()),
            }
        }
        let required: &[&[u8; 4]] = match &parent {
            b"moov" => &[b"mvhd", b"trak"],
            b"trak" => &[b"tkhd", b"mdia"],
            b"mdia" => &[b"mdhd", b"hdlr", b"minf"],
            b"minf" => &[b"vmhd", b"dinf", b"stbl"],
            b"stbl" => &[b"stsd", b"stsz", b"stsc", b"stts"],
            b"dinf" => &[b"dref"],
            b"dref" => &[b"url "],
            b"stsd" => &[b"avc1"],
            b"avc1" => &[b"avcC"],
            b"edts" => &[b"elst"],
            b"udta" => &[b"meta"],
            b"meta" => &[b"hdlr", b"ilst"],
            b"ilst" => &[b"\xa9too"],
            b"\xa9too" => &[b"data"],
            _ => &[],
        };
        require(
            required
                .iter()
                .all(|name| seen.iter().any(|kind| *kind == **name)),
        )
    }

    fn avcc(&self, header: &Header, body: &[u8]) -> Result<(), Failure> {
        let record = AvcDecoderConfigurationRecord::try_from(body).map_err(|_| unsupported())?;
        require(
            record.length_size_minus_one() == 3 && record.num_of_sequence_parameter_sets() == 1,
        )?;
        require(body[4] == 255 && body[5] == 225)?;
        let mut context = h264_reader::Context::new();
        let sps = record
            .sequence_parameter_sets()
            .next()
            .ok_or_else(unsupported)?
            .map_err(|_| unsupported())?;
        let nal = RefNal::new(sps, &[], true);
        let sps =
            SeqParameterSet::from_bits(BoundedBits(nal.rbsp_bits())).map_err(|_| unsupported())?;
        require(sps.pic_width_in_mbs_minus1 <= 255 && sps.pic_height_in_map_units_minus1 <= 255)?;
        let (width, height) = sps.pixel_dimensions().map_err(|_| unsupported())?;
        dimensions(width, height)?;
        require((width, height) == self.dimensions)?;
        context.put_seq_param_set(sps);
        let mut sets = record.picture_parameter_sets();
        let pps = sets
            .next()
            .ok_or_else(unsupported)?
            .map_err(|_| unsupported())?;
        require(
            sets.next().is_none() && record.sequence_parameter_set_extensions().next().is_none(),
        )?;
        let nal = RefNal::new(pps, &[], true);
        let mut bits = nal.rbsp_bits();
        bits.read_ue("pic_parameter_set_id")
            .map_err(|_| unsupported())?;
        bits.read_ue("seq_parameter_set_id")
            .map_err(|_| unsupported())?;
        bits.read_bit("entropy_coding_mode_flag")
            .map_err(|_| unsupported())?;
        bits.read_bit("bottom_field_pic_order_in_frame_present_flag")
            .map_err(|_| unsupported())?;
        require(
            bits.read_ue("num_slice_groups_minus1")
                .map_err(|_| unsupported())?
                == 0,
        )?;
        PicParameterSet::from_bits(&context, BoundedBits(nal.rbsp_bits()))
            .map_err(|_| unsupported())?;
        typed(header, body)?;
        Ok(())
    }

    fn samples(
        &self,
        reader: &mut (impl Read + Seek),
        media: (u64, u64),
        budget: &mut Budget,
    ) -> Result<(), Failure> {
        let sizes = self.sizes.ok_or_else(unsupported)?;
        let count = word(sizes, 8)? as usize;
        let fixed = word(sizes, 4)?;
        let timing = self.timing.ok_or_else(unsupported)?;
        let mut samples = 0u64;
        let mut ticks = 0u64;
        for entry in timing[8..].chunks_exact(8) {
            let n = word(entry, 0)?;
            let delta = word(entry, 4)?;
            require(n > 0 && delta > 0)?;
            samples += u64::from(n);
            ticks = ticks
                .checked_add(u64::from(n) * u64::from(delta))
                .ok_or_else(unsupported)?;
        }
        require(samples == count as u64 && ticks <= u64::from(self.timescale) * 300)?;
        if let Some(composition) = self.composition {
            let mut total = 0u64;
            for entry in composition[8..].chunks_exact(8) {
                let n = word(entry, 0)?;
                require(n > 0)?;
                total += u64::from(n);
                let offset = word(entry, 4)?;
                let offset = if composition[0] == 1 {
                    i64::from(offset as i32).unsigned_abs()
                } else {
                    u64::from(offset)
                };
                require(offset <= u64::from(self.timescale) * 300)?;
            }
            require(total == count as u64)?;
        }
        if let Some(sync) = self.sync {
            let mut previous = 0;
            for entry in sync[8..].chunks_exact(4) {
                let index = word(entry, 0)?;
                require(index > previous && index <= count as u32)?;
                previous = index;
            }
        }
        let (chunks, wide) = self.chunks.ok_or_else(unsupported)?;
        let chunk_count = word(chunks, 4)? as usize;
        let mapping = self.mapping.ok_or_else(unsupported)?;
        let entries = &mapping[8..];
        require(!entries.is_empty() && word(entries, 0)? == 1 && chunk_count > 0)?;
        let mut previous = 0;
        for entry in entries.chunks_exact(12) {
            let chunk = word(entry, 0)?;
            require(
                chunk > previous
                    && chunk as usize <= chunk_count
                    && word(entry, 4)? > 0
                    && word(entry, 4)? as usize <= count
                    && word(entry, 8)? == 1,
            )?;
            previous = chunk;
        }
        let mut intervals = Vec::with_capacity(count);
        let mut map = 0;
        for chunk in 1..=chunk_count {
            while map + 12 < entries.len() && word(entries, map + 12)? as usize <= chunk {
                map += 12;
            }
            let mut offset = if wide {
                long(chunks, 8 + (chunk - 1) * 8)?
            } else {
                word(chunks, 8 + (chunk - 1) * 4)? as u64
            };
            for _ in 0..word(entries, map + 4)? {
                require(intervals.len() < count)?;
                let size = if fixed == 0 {
                    word(sizes, 12 + intervals.len() * 4)?
                } else {
                    fixed
                };
                require(size > 0)?;
                let end = offset
                    .checked_add(u64::from(size))
                    .ok_or_else(unsupported)?;
                require(offset >= media.0 && end <= media.1)?;
                intervals.push((offset, end));
                offset = end;
            }
        }
        require(intervals.len() == count)?;
        intervals.sort_unstable();
        let mut end = media.0;
        for &(start, next) in &intervals {
            require(start == end)?;
            end = next;
        }
        require(end == media.1)?;
        for (start, end) in intervals {
            inspect_sample_with_budget(reader, start, end, budget)?;
        }
        Ok(())
    }
}

fn dimensions(width: u32, height: u32) -> Result<(), Failure> {
    require(
        width > 0
            && height > 0
            && width <= 4096
            && height <= 4096
            && u64::from(width) * u64::from(height) <= 16_000_000,
    )
}

#[cfg(test)]
fn inspect_sample(reader: &mut (impl Read + Seek), start: u64, end: u64) -> Result<(), Failure> {
    inspect_sample_with_budget(reader, start, end, &mut Budget::default())
}

fn inspect_sample_with_budget(
    reader: &mut (impl Read + Seek),
    start: u64,
    end: u64,
    budget: &mut Budget,
) -> Result<(), Failure> {
    reader.seek(SeekFrom::Start(start))?;
    let mut image = false;
    while reader.stream_position()? < end {
        budget.nal_remaining = budget
            .nal_remaining
            .checked_sub(1)
            .ok_or_else(unsupported)?;
        let position = reader.stream_position()?;
        require(end - position >= 5)?;
        let mut prefix = [0; 4];
        reader.read_exact(&mut prefix)?;
        let size = u32::from_be_bytes(prefix) as usize;
        require(size > 0 && size as u64 <= end - position - 4)?;
        let mut header = [0];
        reader.read_exact(&mut header)?;
        require(header[0] & 0x80 == 0)?;
        match header[0] & 31 {
            1 | 5 => {
                require(size > 1 && (header[0] & 31 != 5 || header[0] & 0x60 != 0))?;
                image = true;
                reader.seek(SeekFrom::Current((size - 1) as i64))?;
            }
            6 => {
                require(size <= MAX_METADATA && header[0] == 6)?;
                let mut nal = vec![0; size];
                nal[0] = header[0];
                reader.read_exact(&mut nal[1..])?;
                inspect_sei(&nal)?;
            }
            9 => {
                require(size == 2 && header[0] == 9)?;
                let mut byte = [0];
                reader.read_exact(&mut byte)?;
                require(byte[0] & 31 == 16)?;
            }
            _ => return Err(unsupported()),
        }
    }
    require(image)
}

fn inspect_sei(nal: &[u8]) -> Result<(), Failure> {
    use h264_reader::nal::sei::{HeaderType, SeiReader};
    require(nal.len() <= MAX_METADATA)?;
    let rbsp = h264_reader::rbsp::decode_nal(nal).map_err(|_| unsupported())?;
    let mut scratch = Vec::new();
    let mut reader = SeiReader::from_rbsp_bytes(rbsp.as_ref(), &mut scratch);
    let mut count = 0;
    while let Some(message) = reader.next().map_err(|_| unsupported())? {
        require(
            message.payload_type == HeaderType::UserDataUnregistered && message.payload.len() >= 17,
        )?;
        super::super::guard::check_text(&String::from_utf8_lossy(&message.payload[..16]))?;
        let value = &message.payload[16..];
        require(value.last() == Some(&0) && !value[..value.len() - 1].contains(&0))?;
        text(&value[..value.len() - 1])?;
        count += 1;
        require(count <= 1024)?;
    }
    require(count > 0)
}

// Bound dimensions before the codec parser derives macroblock counts.
struct BoundedBits<R>(R);
impl<R: BitRead> BitRead for BoundedBits<R> {
    fn read_ue(&mut self, name: &'static str) -> Result<u32, h264_reader::rbsp::BitReaderError> {
        let value = self.0.read_ue(name)?;
        if value > 8192 {
            return Err(h264_reader::rbsp::BitReaderError::ReaderError(
                name,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "H.264 integer exceeds the supported profile.",
                ),
            ));
        }
        if matches!(
            name,
            "pic_width_in_mbs_minus1" | "pic_height_in_map_units_minus1"
        ) && value > 255
        {
            return Err(h264_reader::rbsp::BitReaderError::ReaderError(
                name,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "MP4 dimensions exceed the supported profile.",
                ),
            ));
        }
        Ok(value)
    }
    fn read_se(&mut self, name: &'static str) -> Result<i32, h264_reader::rbsp::BitReaderError> {
        let value = self.0.read_se(name)?;
        if !(-8192..=8192).contains(&value) {
            return Err(h264_reader::rbsp::BitReaderError::ReaderError(
                name,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "H.264 integer exceeds the supported profile.",
                ),
            ));
        }
        Ok(value)
    }
    fn read_bit(&mut self, name: &'static str) -> Result<bool, h264_reader::rbsp::BitReaderError> {
        self.0.read_bit(name)
    }
    fn read<const BITS: u32, I: h264_reader::rbsp::Integer>(
        &mut self,
        name: &'static str,
    ) -> Result<I, h264_reader::rbsp::BitReaderError> {
        self.0.read::<BITS, I>(name)
    }
    fn read_var<I: h264_reader::rbsp::Integer>(
        &mut self,
        count: u32,
        name: &'static str,
    ) -> Result<I, h264_reader::rbsp::BitReaderError> {
        self.0.read_var(count, name)
    }
    fn read_to<V: h264_reader::rbsp::Primitive>(
        &mut self,
        name: &'static str,
    ) -> Result<V, h264_reader::rbsp::BitReaderError> {
        self.0.read_to(name)
    }
    fn skip(
        &mut self,
        count: u32,
        name: &'static str,
    ) -> Result<(), h264_reader::rbsp::BitReaderError> {
        self.0.skip(count, name)
    }
    fn byte_aligned(&self) -> bool {
        self.0.byte_aligned()
    }
    fn has_more_rbsp_data(
        &mut self,
        name: &'static str,
    ) -> Result<bool, h264_reader::rbsp::BitReaderError> {
        self.0.has_more_rbsp_data(name)
    }
    fn finish_rbsp(self) -> Result<(), h264_reader::rbsp::BitReaderError> {
        self.0.finish_rbsp()
    }
    fn finish_sei_payload(self) -> Result<(), h264_reader::rbsp::BitReaderError> {
        self.0.finish_sei_payload()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    const SAMPLE: &[u8] = include_bytes!("../../../tests/data/artifact-media/sample.mp4");
    #[test]
    fn accepts_reviewed_video_profile() {
        inspect(&mut Cursor::new(SAMPLE)).unwrap();
    }
    #[test]
    fn refuses_excessive_nal_units_in_one_sample() {
        let bytes = [0, 0, 0, 2, 1, 128].repeat(16_385);
        let end = bytes.len() as u64;
        let mut reader = Cursor::new(bytes);
        assert!(inspect_sample(&mut reader, 0, end).is_err());
        assert_eq!(reader.position(), 16_384 * 6);
    }
    #[test]
    fn refuses_truncation_and_trailing_bytes() {
        assert!(inspect(&mut Cursor::new(&SAMPLE[..SAMPLE.len() - 1])).is_err());
        let mut bytes = SAMPLE.to_vec();
        bytes.extend_from_slice(b"garbage");
        assert!(inspect(&mut Cursor::new(bytes)).is_err());
    }
    #[test]
    fn refuses_unknown_top_level_and_nested_boxes() {
        for (old, new) in [
            (b"free", b"uuid"),
            (b"pasp", b"uuid"),
            (b"moov", b"moof"),
            (b"vide", b"soun"),
        ] {
            let mut bytes = SAMPLE.to_vec();
            let position = bytes.windows(4).position(|value| value == old).unwrap();
            bytes[position..position + 4].copy_from_slice(new);
            assert!(inspect(&mut Cursor::new(bytes)).is_err());
        }
    }
    #[test]
    fn refuses_sensitive_text_and_sei() {
        let mut bytes = SAMPLE.to_vec();
        let pos = bytes.windows(4).position(|value| value == b"Lavf").unwrap();
        bytes[pos..pos + 13].copy_from_slice(b"a@b.test     ");
        assert!(inspect(&mut Cursor::new(bytes)).is_err());
        let mut sei = b"\x06\x05\x19abcdefghijklmnop".to_vec();
        sei.extend_from_slice(b"a@b.test\x00\x80");
        assert!(matches!(inspect_sei(&sei), Err(Failure::Redaction(_))));
    }
    #[test]
    fn refuses_sensitive_text_in_opaque_sei_identifiers() {
        let mut bytes = SAMPLE.to_vec();
        let marker = b"x264 - core";
        let offset = bytes
            .windows(marker.len())
            .position(|value| value == marker)
            .unwrap();
        bytes[offset - 16..offset].copy_from_slice(b"person@test.test");
        assert!(matches!(
            inspect(&mut Cursor::new(bytes)),
            Err(Failure::Redaction(_))
        ));
    }
    #[test]
    fn inspects_sei_payloads_and_refuses_unknown_or_oversized_messages() {
        let mut sei = b"\x06\x05\x1babcdefghijklmnop".to_vec();
        sei.extend_from_slice(b"Demo clip.\0\x80");
        inspect_sei(&sei).unwrap();
        sei[1] = 4;
        assert!(inspect_sei(&sei).is_err());
        sei[1] = 5;
        sei.pop();
        assert!(inspect_sei(&sei).is_err());
        assert!(inspect_sei(&vec![0; MAX_METADATA + 1]).is_err());
    }
    #[test]
    fn refuses_open_ended_sizes_and_external_references() {
        let mut bytes = SAMPLE.to_vec();
        bytes[..4].fill(0);
        assert!(inspect(&mut Cursor::new(bytes)).is_err());
        let mut bytes = SAMPLE.to_vec();
        bytes[..4].copy_from_slice(&1u32.to_be_bytes());
        bytes.splice(8..8, 40u64.to_be_bytes());
        assert!(inspect(&mut Cursor::new(bytes)).is_err());
        let mut bytes = SAMPLE.to_vec();
        let pos = bytes.windows(4).position(|value| value == b"url ").unwrap();
        bytes[pos + 7] = 0;
        assert!(inspect(&mut Cursor::new(bytes)).is_err());
    }
    #[test]
    fn refuses_bounds_and_unreferenced_media() {
        for (kind, offset, value) in [
            (b"stsz", 12, 8193u32),
            (b"stco", 12, 915u32),
            (b"mdhd", 20, u32::MAX),
        ] {
            let mut bytes = SAMPLE.to_vec();
            let pos = bytes.windows(4).position(|value| value == kind).unwrap() + offset;
            bytes[pos..pos + 4].copy_from_slice(&value.to_be_bytes());
            assert!(inspect(&mut Cursor::new(bytes)).is_err());
        }
    }
}
