//! The derived index: a binary snapshot of a folded `Tree`, so a command can
//! skip re-reading and re-folding a log it has already folded once.
//!
//! `LOADING.md` §4 is the specification this file exists to satisfy. Five
//! rules from it are not this module's opinion -- they are what makes an
//! index different from a second copy of the truth:
//!
//! - **Deleting it changes no output, ever.** Every `Tree` this module hands
//!   back -- fresh from the index, or from the index plus a tail of new
//!   events -- is exactly what folding the whole log would have produced.
//!   The round-trip and tail-application tests at the bottom of this file
//!   check that directly; the crate's own byte-for-byte harness checked it
//!   against real command output.
//! - **A write never rewrites it.** `allow_persist` is the one knob `load`
//!   takes, and callers that may append to the log pass `false` -- see
//!   `ops::Ctx::load_for_write`.
//! - **A log that folds with an anomaly gets no index.** A `pending`
//!   forward reference or a repeated `num` (`Tree::has_pending`,
//!   `Tree::repeated_nums`) both depend on the fold that produced them to
//!   ever be fixed up or reported again; loading straight from the index
//!   skips that fold, so `persist` refuses to write while either is
//!   non-empty.
//! - **It never fails.** A missing file, the wrong magic, the wrong
//!   version, an offset that points outside the file, the control ULID not
//!   where the header says, or a permission this process does not have are
//!   all answered the same way: fall back to folding the whole log, never
//!   surface as a command failure.
//! - **It is written to a temporary file and renamed.** `write_atomically`
//!   is the only function that ever touches the real path, and only by
//!   renaming a sibling temporary file that already holds the whole thing.
//!
//! ## Format
//!
//! A fixed-width header, then: a node table in ascending `num` order; the
//! spans arena `Node::refs` and `Node::governs` point into; a flat table of
//! flags; the stack; the roots; the vivacs; and the text blob. The header
//! carries every section's own start, so reading one never has to walk the
//! ones before it.
//!
//! ## Staying current
//!
//! The header carries the log's own byte length and modification time. A
//! match means the index is exactly current. A mismatch means the log
//! either grew -- read from the header's own saved offset, once the last
//! event this index ever folded is confirmed still sitting where the header
//! says, by its ULID and its `seq` -- or changed underneath it, in which
//! case there is no tail to apply and the whole log is folded fresh.

use crate::anchor::AnchorRef;
use crate::event::{Event, Flag, Kind, State, VivacKind};
use crate::model::{fold, ArmSpan, Node, Note, RawParts, Span, Tree, Vivac};
use crate::store::Store;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write as IoWrite};
use std::path::Path;

const MAGIC: u64 = u64::from_le_bytes(*b"vivacIDX");
// `t411`/`d441`: a rule's `arms` is a new field on `Node`, and each arm is a
// pair of spans rather than one. `Header::parse` refuses any version but
// this one and `try_load_index` falls back to folding the log, which is
// what the index is derived from -- so bumping this needs no migration and
// no command.
const FORMAT_VERSION: u32 = 4;
const ULID_LEN: usize = 26;
const SPAN_LEN: usize = 8;
const FLAG_RECORD_LEN: usize = 1 + SPAN_LEN;
/// A note's own moment and text, mirroring `FLAG_RECORD_LEN`'s shape: no tag
/// byte, since a note carries no enum the way a flag carries its kind.
const NOTE_RECORD_LEN: usize = SPAN_LEN * 2;
/// An arm is two spans into the text arena -- the folder it runs in and the
/// command itself (`d441`) -- so the flat arms table is shaped like the
/// notes table above it, one record per arm.
const ARM_RECORD_LEN: usize = SPAN_LEN * 2;
const NODE_RECORD_LEN: usize = ULID_LEN
    + 8
    + 1
    + 1
    + 8
    + 1
    + 1
    + SPAN_LEN * 4
    + 1
    + SPAN_LEN
    + SPAN_LEN
    + SPAN_LEN
    + 4
    + 4
    + 4
    + 4
    + 4
    + 4;

/// `LOADING.md` §4 "El umbral, con su número": a stale index is left alone
/// below this many pending events, because applying them in memory is cheap
/// enough to fit inside the write budget, and only a read is ever allowed to
/// pay for rewriting the file itself.
const TAIL_REFRESH_THRESHOLD: usize = 200;

/// Builds the `Tree` a command needs, using the derived index when it can.
///
/// `allow_persist` is `false` for a command that may append to the log: a
/// write must never pay the cost of rewriting the index (`LOADING.md` §4
/// "Cuándo se reescribe"), even though it is free to read a warm or stale
/// one exactly like a read does. The only error this can return is a
/// genuine failure to read `events` itself -- everything the index's own
/// file touches is caught internally and answered by folding the log.
pub fn load(store: &Store, allow_persist: bool) -> std::io::Result<Tree> {
    if let Some(loaded) = try_load_index(store) {
        return Ok(match loaded {
            Loaded::Fresh(tree) => tree,
            Loaded::Grown {
                tree,
                tail_len,
                fold_end_offset,
                last,
            } => {
                if allow_persist && tail_len > TAIL_REFRESH_THRESHOLD {
                    persist(store, &tree, fold_end_offset, last.as_ref());
                }
                tree
            }
        });
    }
    let tail = read_tracked(&store.log(), 0)?;
    let tree = fold(&tail.events, tail.broken);
    if allow_persist {
        persist(store, &tree, tail.end_offset, tail.last.as_ref());
    }
    Ok(tree)
}

enum Loaded {
    Fresh(Tree),
    Grown {
        tree: Tree,
        tail_len: usize,
        fold_end_offset: u64,
        last: Option<LastEvent>,
    },
}

#[derive(Clone)]
struct LastEvent {
    line_offset: u64,
    id: String,
    seq: u64,
}

// ---------------------------------------------------------------------------
// Reading the log, tracking byte offsets. Separate from `Store::read_all`
// on purpose: that function's contract (broken lines counted and skipped,
// invalid UTF-8 propagated) is exercised by the rest of the suite already,
// and this module must reproduce it exactly for a tail applied on top of an
// old index to agree with a fresh fold -- `read_tracked_agrees_with_store_
// read_all` and `a_stale_index_picks_up_the_tail`, below, are what prove it
// does.
// ---------------------------------------------------------------------------

struct Tracked {
    events: Vec<Event>,
    broken: usize,
    /// Byte offset at EOF, counted from the very start of the file
    /// regardless of `from_offset`.
    end_offset: u64,
    last: Option<LastEvent>,
}

fn read_tracked(path: &Path, from_offset: u64) -> std::io::Result<Tracked> {
    let f = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Tracked {
                events: Vec::new(),
                broken: 0,
                end_offset: from_offset,
                last: None,
            })
        }
        Err(e) => return Err(e),
    };
    let mut reader = BufReader::new(f);
    reader.seek(SeekFrom::Start(from_offset))?;
    let mut cursor = from_offset;
    let mut events = Vec::new();
    let mut broken = 0usize;
    let mut last = None;
    let mut raw = Vec::new();
    loop {
        raw.clear();
        let n = reader.read_until(b'\n', &mut raw)?;
        if n == 0 {
            break;
        }
        let line_offset = cursor;
        cursor += n as u64;
        let mut bytes = raw.as_slice();
        if bytes.last() == Some(&b'\n') {
            bytes = &bytes[..bytes.len() - 1];
        }
        if bytes.last() == Some(&b'\r') {
            bytes = &bytes[..bytes.len() - 1];
        }
        let line = String::from_utf8(bytes.to_vec()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "stream did not contain valid UTF-8",
            )
        })?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Event>(&line) {
            Ok(e) => {
                last = Some(LastEvent {
                    line_offset,
                    id: e.id.clone(),
                    seq: e.seq,
                });
                events.push(e);
            }
            Err(_) => broken += 1,
        }
    }
    Ok(Tracked {
        events,
        broken,
        end_offset: cursor,
        last,
    })
}

// ---------------------------------------------------------------------------
// Deciding whether the index on disk is usable.
// ---------------------------------------------------------------------------

fn fingerprint(path: &Path) -> (u64, i64, u32) {
    match fs::metadata(path) {
        Ok(m) => {
            let (secs, nanos) = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| (d.as_secs() as i64, d.subsec_nanos()))
                .unwrap_or((0, 0));
            (m.len(), secs, nanos)
        }
        Err(_) => (0, 0, 0),
    }
}

/// Confirms the last event this index ever folded is still sitting where
/// the header says it is -- the check that tells "the log grew" apart from
/// "the log changed", per `LOADING.md` §4 "Vigencia".
fn control_still_there(log_path: &Path, header: &Header) -> bool {
    if !header.has_last {
        // Nothing was ever folded, so there is nothing to confirm: growing
        // from byte zero needs no control check.
        return header.fold_end_offset == 0;
    }
    let Ok(f) = File::open(log_path) else {
        return false;
    };
    let mut reader = BufReader::new(f);
    if reader
        .seek(SeekFrom::Start(header.last_line_offset))
        .is_err()
    {
        return false;
    }
    let mut raw = Vec::new();
    let n = match reader.read_until(b'\n', &mut raw) {
        Ok(n) => n,
        Err(_) => return false,
    };
    if n == 0 {
        return false;
    }
    let mut bytes = raw.as_slice();
    if bytes.last() == Some(&b'\n') {
        bytes = &bytes[..bytes.len() - 1];
    }
    if bytes.last() == Some(&b'\r') {
        bytes = &bytes[..bytes.len() - 1];
    }
    let Ok(line) = std::str::from_utf8(bytes) else {
        return false;
    };
    let Ok(e) = serde_json::from_str::<Event>(line) else {
        return false;
    };
    e.id == header.last_ulid && e.seq == header.last_seq
}

fn try_load_index(store: &Store) -> Option<Loaded> {
    let bytes = fs::read(store.index_path()).ok()?;
    let header = Header::parse(&bytes)?;
    let log_path = store.log();
    let (cur_len, cur_secs, cur_nanos) = fingerprint(&log_path);

    if header.log_len == cur_len && header.mtime_secs == cur_secs && header.mtime_nanos == cur_nanos
    {
        return build_tree(&bytes, &header).map(Loaded::Fresh);
    }

    if cur_len < header.fold_end_offset || !control_still_there(&log_path, &header) {
        return None;
    }

    let tail = read_tracked(&log_path, header.fold_end_offset).ok()?;
    let mut tree = build_tree(&bytes, &header)?;
    for e in &tail.events {
        tree.apply(e.seq, &e.ts, &e.payload);
    }
    let last = tail.last.clone().or_else(|| {
        header.has_last.then(|| LastEvent {
            line_offset: header.last_line_offset,
            id: header.last_ulid.clone(),
            seq: header.last_seq,
        })
    });
    Some(Loaded::Grown {
        tree,
        tail_len: tail.events.len(),
        fold_end_offset: tail.end_offset,
        last,
    })
}

// ---------------------------------------------------------------------------
// Writing.
// ---------------------------------------------------------------------------

/// Every id this format stores travels in a fixed-width slot: the record
/// layout is what makes the node table directly seekable rather than
/// something a reader has to scan. Every id this binary ever mints
/// (`id::ulid`) is exactly this shape, so the check only ever refuses a
/// hand-edited log -- the same log that a repeated `num` or a pending
/// reference already refuses, and for the same reason: better to keep
/// folding it whole than to store something the format cannot represent
/// without silently truncating it.
fn is_ulid_shaped(s: &str) -> bool {
    s.len() == ULID_LEN && s.is_ascii()
}

/// Best-effort: every failure -- an anomaly in the tree, a read-only
/// directory, a full disk -- is swallowed. `LOADING.md` §4 "Si no se puede
/// escribir, no pasa nada" and "Un log con anomalías no lleva índice".
fn persist(store: &Store, tree: &Tree, fold_end_offset: u64, last: Option<&LastEvent>) {
    if tree.has_pending() || !tree.repeated_nums.is_empty() {
        return;
    }
    let ids_fit = tree.nodes_sorted().iter().all(|n| is_ulid_shaped(&n.id))
        && tree.vivacs.iter().all(|v| is_ulid_shaped(&v.id))
        && last.map_or(true, |l| is_ulid_shaped(&l.id));
    if !ids_fit {
        return;
    }
    let (mtime_secs, mtime_nanos) = mtime_of(&store.log());
    let bytes = encode(tree, fold_end_offset, mtime_secs, mtime_nanos, last);
    let _ = write_atomically(&store.index_path(), &bytes);
}

fn mtime_of(path: &Path) -> (i64, u32) {
    match fs::metadata(path).and_then(|m| m.modified()) {
        Ok(t) => match t.duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => (d.as_secs() as i64, d.subsec_nanos()),
            Err(_) => (0, 0),
        },
        Err(_) => (0, 0),
    }
}

/// Never leaves a half-written index behind: the real path is only ever
/// touched by a `rename` of a sibling temporary file that already holds the
/// whole thing. `LOADING.md` §4, and the same rule `t84`'s own registry
/// uses for the same reason.
fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("index path has no parent"))?;
    let tmp = dir.join(format!("index.tmp.{}", crate::id::ulid()));
    let result = (|| -> std::io::Result<()> {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        drop(f);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

// ---------------------------------------------------------------------------
// The header.
// ---------------------------------------------------------------------------

struct Header {
    seq: u64,
    fold_end_offset: u64,
    has_last: bool,
    last_line_offset: u64,
    last_ulid: String,
    /// The `seq` of the event at `last_line_offset`, kept apart from `seq`
    /// above: `seq` is the tree's own high-water mark, which a hand-edited
    /// log with out-of-order numbers could in principle push past the
    /// physically last line's own value. The control check has to compare
    /// against the line it is actually looking at, not the tree's summary
    /// of every line.
    last_seq: u64,
    log_len: u64,
    mtime_secs: i64,
    mtime_nanos: u32,
    next_num: u64,
    next_vivac_num: u64,
    seq_change: u64,
    seq_vivac: u64,
    seg_new: u64,
    seg_closed: u64,
    seg_notes: u64,
    seg_events: u64,
    broken_lines: u64,
    node_count: u64,
    spans_count: u64,
    flags_count: u64,
    notes_count: u64,
    arms_count: u64,
    roots_count: u64,
    stack_count: u64,
    vivac_count: u64,
    nodes_offset: u64,
    spans_offset: u64,
    flags_offset: u64,
    notes_offset: u64,
    arms_offset: u64,
    roots_offset: u64,
    stack_offset: u64,
    vivacs_offset: u64,
    text_offset: u64,
    text_len: u64,
    file_len: u64,
}

impl Header {
    fn parse(bytes: &[u8]) -> Option<Header> {
        let mut c = Cursor::new(bytes);
        if c.u64()? != MAGIC {
            return None;
        }
        if c.u32()? != FORMAT_VERSION {
            return None;
        }
        let h = Header {
            seq: c.u64()?,
            fold_end_offset: c.u64()?,
            has_last: c.bool_()?,
            last_line_offset: c.u64()?,
            last_ulid: c.fixed_str(ULID_LEN)?,
            last_seq: c.u64()?,
            log_len: c.u64()?,
            mtime_secs: c.i64()?,
            mtime_nanos: c.u32()?,
            next_num: c.u64()?,
            next_vivac_num: c.u64()?,
            seq_change: c.u64()?,
            seq_vivac: c.u64()?,
            seg_new: c.u64()?,
            seg_closed: c.u64()?,
            seg_notes: c.u64()?,
            seg_events: c.u64()?,
            broken_lines: c.u64()?,
            node_count: c.u64()?,
            spans_count: c.u64()?,
            flags_count: c.u64()?,
            notes_count: c.u64()?,
            arms_count: c.u64()?,
            roots_count: c.u64()?,
            stack_count: c.u64()?,
            vivac_count: c.u64()?,
            nodes_offset: c.u64()?,
            spans_offset: c.u64()?,
            flags_offset: c.u64()?,
            notes_offset: c.u64()?,
            arms_offset: c.u64()?,
            roots_offset: c.u64()?,
            stack_offset: c.u64()?,
            vivacs_offset: c.u64()?,
            text_offset: c.u64()?,
            text_len: c.u64()?,
            file_len: c.u64()?,
        };
        if h.file_len as usize != bytes.len() {
            return None;
        }
        h.check_bounds(bytes.len())?;
        Some(h)
    }

    /// Every section has to fit inside the file this header came from --
    /// "un desplazamiento apunta fuera" is one of `LOADING.md` §4's own
    /// named reasons to regenerate rather than trust what is on disk.
    fn check_bounds(&self, len: usize) -> Option<()> {
        let fits = |off: u64, count: u64, width: u64| -> Option<bool> {
            let size = count.checked_mul(width)?;
            let end = off.checked_add(size)?;
            Some(end as usize <= len)
        };
        if !fits(self.nodes_offset, self.node_count, NODE_RECORD_LEN as u64)? {
            return None;
        }
        if !fits(self.spans_offset, self.spans_count, SPAN_LEN as u64)? {
            return None;
        }
        if !fits(self.flags_offset, self.flags_count, FLAG_RECORD_LEN as u64)? {
            return None;
        }
        if !fits(self.notes_offset, self.notes_count, NOTE_RECORD_LEN as u64)? {
            return None;
        }
        if !fits(self.arms_offset, self.arms_count, ARM_RECORD_LEN as u64)? {
            return None;
        }
        if !fits(self.roots_offset, self.roots_count, 8)? {
            return None;
        }
        if !fits(self.stack_offset, self.stack_count, 8)? {
            return None;
        }
        let text_end = self.text_offset.checked_add(self.text_len)?;
        if text_end as usize > len {
            return None;
        }
        if self.vivacs_offset as usize > len {
            return None;
        }
        Some(())
    }
}

#[allow(clippy::too_many_arguments)]
fn write_header(buf: &mut Vec<u8>, h: &Header) {
    write_u64(buf, MAGIC);
    write_u32(buf, FORMAT_VERSION);
    write_u64(buf, h.seq);
    write_u64(buf, h.fold_end_offset);
    write_bool(buf, h.has_last);
    write_u64(buf, h.last_line_offset);
    write_ulid(buf, &h.last_ulid);
    write_u64(buf, h.last_seq);
    write_u64(buf, h.log_len);
    buf.extend_from_slice(&h.mtime_secs.to_le_bytes());
    write_u32(buf, h.mtime_nanos);
    write_u64(buf, h.next_num);
    write_u64(buf, h.next_vivac_num);
    write_u64(buf, h.seq_change);
    write_u64(buf, h.seq_vivac);
    write_u64(buf, h.seg_new);
    write_u64(buf, h.seg_closed);
    write_u64(buf, h.seg_notes);
    write_u64(buf, h.seg_events);
    write_u64(buf, h.broken_lines);
    write_u64(buf, h.node_count);
    write_u64(buf, h.spans_count);
    write_u64(buf, h.flags_count);
    write_u64(buf, h.notes_count);
    write_u64(buf, h.arms_count);
    write_u64(buf, h.roots_count);
    write_u64(buf, h.stack_count);
    write_u64(buf, h.vivac_count);
    write_u64(buf, h.nodes_offset);
    write_u64(buf, h.spans_offset);
    write_u64(buf, h.flags_offset);
    write_u64(buf, h.notes_offset);
    write_u64(buf, h.arms_offset);
    write_u64(buf, h.roots_offset);
    write_u64(buf, h.stack_offset);
    write_u64(buf, h.vivacs_offset);
    write_u64(buf, h.text_offset);
    write_u64(buf, h.text_len);
    write_u64(buf, h.file_len);
}

fn header_len() -> usize {
    let placeholder = Header {
        seq: 0,
        fold_end_offset: 0,
        has_last: false,
        last_line_offset: 0,
        last_ulid: "0".repeat(ULID_LEN),
        last_seq: 0,
        log_len: 0,
        mtime_secs: 0,
        mtime_nanos: 0,
        next_num: 0,
        next_vivac_num: 0,
        seq_change: 0,
        seq_vivac: 0,
        seg_new: 0,
        seg_closed: 0,
        seg_notes: 0,
        seg_events: 0,
        broken_lines: 0,
        node_count: 0,
        spans_count: 0,
        flags_count: 0,
        notes_count: 0,
        arms_count: 0,
        roots_count: 0,
        stack_count: 0,
        vivac_count: 0,
        nodes_offset: 0,
        spans_offset: 0,
        flags_offset: 0,
        notes_offset: 0,
        arms_offset: 0,
        roots_offset: 0,
        stack_offset: 0,
        vivacs_offset: 0,
        text_offset: 0,
        text_len: 0,
        file_len: 0,
    };
    let mut buf = Vec::new();
    write_header(&mut buf, &placeholder);
    buf.len()
}

// ---------------------------------------------------------------------------
// Byte-level helpers.
// ---------------------------------------------------------------------------

struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Cursor<'a> {
        Cursor { buf, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let slice = self.buf.get(self.pos..end)?;
        self.pos = end;
        Some(slice)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    fn bool_(&mut self) -> Option<bool> {
        self.u8().map(|b| b != 0)
    }

    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    }

    fn u64(&mut self) -> Option<u64> {
        self.take(8)
            .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
    }

    fn i64(&mut self) -> Option<i64> {
        self.take(8)
            .map(|b| i64::from_le_bytes(b.try_into().unwrap()))
    }

    fn span(&mut self) -> Option<Span> {
        Some(Span {
            start: self.u32()?,
            len: self.u32()?,
        })
    }

    fn fixed_str(&mut self, n: usize) -> Option<String> {
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }

    fn str(&mut self) -> Option<String> {
        let len = self.u32()? as usize;
        String::from_utf8(self.take(len)?.to_vec()).ok()
    }
}

fn write_u8(buf: &mut Vec<u8>, v: u8) {
    buf.push(v);
}

fn write_bool(buf: &mut Vec<u8>, v: bool) {
    buf.push(v as u8);
}

fn write_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn write_u64(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn write_span(buf: &mut Vec<u8>, s: Span) {
    write_u32(buf, s.start);
    write_u32(buf, s.len);
}

fn write_str(buf: &mut Vec<u8>, s: &str) {
    write_u32(buf, s.len() as u32);
    buf.extend_from_slice(s.as_bytes());
}

fn write_ulid(buf: &mut Vec<u8>, s: &str) {
    debug_assert_eq!(s.len(), ULID_LEN, "a ulid is always {ULID_LEN} bytes");
    buf.extend_from_slice(s.as_bytes());
}

// ---------------------------------------------------------------------------
// Enum <-> byte, by hand: no `repr(u8)` on `event.rs`'s own public enums,
// and no dependency to derive one.
// ---------------------------------------------------------------------------

fn kind_to_u8(k: Kind) -> u8 {
    match k {
        Kind::Goal => 0,
        Kind::Task => 1,
        Kind::Decision => 2,
        Kind::Question => 3,
        Kind::Constraint => 4,
        Kind::Finding => 5,
        Kind::Assumption => 6,
        Kind::Pillar => 7,
        Kind::Rule => 8,
    }
}

fn u8_to_kind(b: u8) -> Option<Kind> {
    Some(match b {
        0 => Kind::Goal,
        1 => Kind::Task,
        2 => Kind::Decision,
        3 => Kind::Question,
        4 => Kind::Constraint,
        5 => Kind::Finding,
        6 => Kind::Assumption,
        7 => Kind::Pillar,
        8 => Kind::Rule,
        _ => return None,
    })
}

fn state_to_u8(s: State) -> u8 {
    match s {
        State::Active => 0,
        State::Done => 1,
        State::Suspended => 2,
        State::Abandoned => 3,
        State::Superseded => 4,
    }
}

fn u8_to_state(b: u8) -> Option<State> {
    Some(match b {
        0 => State::Active,
        1 => State::Done,
        2 => State::Suspended,
        3 => State::Abandoned,
        4 => State::Superseded,
        _ => return None,
    })
}

fn vivac_kind_to_u8(k: VivacKind) -> u8 {
    match k {
        VivacKind::Push => 0,
        VivacKind::Pop => 1,
        VivacKind::Park => 2,
        VivacKind::Manual => 3,
        VivacKind::Auto => 4,
    }
}

fn u8_to_vivac_kind(b: u8) -> Option<VivacKind> {
    Some(match b {
        0 => VivacKind::Push,
        1 => VivacKind::Pop,
        2 => VivacKind::Park,
        3 => VivacKind::Manual,
        4 => VivacKind::Auto,
        _ => return None,
    })
}

fn flag_to_u8(f: Flag) -> u8 {
    match f {
        Flag::Suspect => 0,
        Flag::Review => 1,
        Flag::Stale => 2,
    }
}

fn u8_to_flag(b: u8) -> Option<Flag> {
    Some(match b {
        0 => Flag::Suspect,
        1 => Flag::Review,
        2 => Flag::Stale,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// The node table and the flat flags table it points into.
// ---------------------------------------------------------------------------

struct NodeRaw {
    id: String,
    num: u64,
    kind: Kind,
    state: State,
    parent: Option<u64>,
    blocks: bool,
    forced_close: bool,
    title: Span,
    why: Span,
    outcome: Span,
    opened: Span,
    closed: Option<Span>,
    refs: Span,
    governs: Span,
    flags_offset: u32,
    flags_count: u32,
    notes_offset: u32,
    notes_count: u32,
    arms_offset: u32,
    arms_count: u32,
}

#[allow(clippy::too_many_arguments)]
fn write_node_record(
    buf: &mut Vec<u8>,
    n: &Node,
    flags_buf: &mut Vec<u8>,
    flags_cursor: &mut u32,
    notes_buf: &mut Vec<u8>,
    notes_cursor: &mut u32,
    arms_buf: &mut Vec<u8>,
    arms_cursor: &mut u32,
) {
    let start = buf.len();
    write_ulid(buf, &n.id);
    write_u64(buf, n.num);
    write_u8(buf, kind_to_u8(n.kind));
    write_u8(buf, state_to_u8(n.state));
    write_u64(buf, n.parent.unwrap_or(u64::MAX));
    write_bool(buf, n.blocks);
    write_bool(buf, n.forced_close);
    write_span(buf, n.title);
    write_span(buf, n.why);
    write_span(buf, n.outcome);
    write_span(buf, n.opened);
    match n.closed {
        Some(s) => {
            write_bool(buf, true);
            write_span(buf, s);
        }
        None => {
            write_bool(buf, false);
            write_span(buf, Span::default());
        }
    }
    write_span(buf, n.refs);
    write_span(buf, n.governs);
    let flags_offset = *flags_cursor;
    for (&flag, &span) in &n.flags {
        write_u8(flags_buf, flag_to_u8(flag));
        write_span(flags_buf, span);
    }
    let flags_count = n.flags.len() as u32;
    *flags_cursor += flags_count;
    write_u32(buf, flags_offset);
    write_u32(buf, flags_count);
    let notes_offset = *notes_cursor;
    for note in &n.notes {
        write_span(notes_buf, note.at);
        write_span(notes_buf, note.text);
    }
    let notes_count = n.notes.len() as u32;
    *notes_cursor += notes_count;
    write_u32(buf, notes_offset);
    write_u32(buf, notes_count);
    let arms_offset = *arms_cursor;
    for arm in &n.arms {
        write_span(arms_buf, arm.dir);
        write_span(arms_buf, arm.command);
    }
    let arms_count = n.arms.len() as u32;
    *arms_cursor += arms_count;
    write_u32(buf, arms_offset);
    write_u32(buf, arms_count);
    debug_assert_eq!(buf.len() - start, NODE_RECORD_LEN);
}

fn read_node_record(c: &mut Cursor) -> Option<NodeRaw> {
    let id = c.fixed_str(ULID_LEN)?;
    let num = c.u64()?;
    let kind = u8_to_kind(c.u8()?)?;
    let state = u8_to_state(c.u8()?)?;
    let parent_raw = c.u64()?;
    let parent = (parent_raw != u64::MAX).then_some(parent_raw);
    let blocks = c.bool_()?;
    let forced_close = c.bool_()?;
    let title = c.span()?;
    let why = c.span()?;
    let outcome = c.span()?;
    let opened = c.span()?;
    let closed_present = c.bool_()?;
    let closed_span = c.span()?;
    let closed = closed_present.then_some(closed_span);
    let refs = c.span()?;
    let governs = c.span()?;
    let flags_offset = c.u32()?;
    let flags_count = c.u32()?;
    let notes_offset = c.u32()?;
    let notes_count = c.u32()?;
    let arms_offset = c.u32()?;
    let arms_count = c.u32()?;
    Some(NodeRaw {
        id,
        num,
        kind,
        state,
        parent,
        blocks,
        forced_close,
        title,
        why,
        outcome,
        opened,
        closed,
        refs,
        governs,
        flags_offset,
        flags_count,
        notes_offset,
        notes_count,
        arms_offset,
        arms_count,
    })
}

fn assemble_nodes(
    raw_nodes: Vec<NodeRaw>,
    flags_table: &[(Flag, Span)],
    notes_table: &[Note],
    arms_table: &[ArmSpan],
) -> Option<Vec<Node>> {
    let mut out = Vec::with_capacity(raw_nodes.len());
    for r in raw_nodes {
        let start = r.flags_offset as usize;
        let end = start.checked_add(r.flags_count as usize)?;
        let slice = flags_table.get(start..end)?;
        let mut flags = BTreeMap::new();
        for &(f, s) in slice {
            flags.insert(f, s);
        }
        let notes_start = r.notes_offset as usize;
        let notes_end = notes_start.checked_add(r.notes_count as usize)?;
        let notes = notes_table.get(notes_start..notes_end)?.to_vec();
        let arms_start = r.arms_offset as usize;
        let arms_end = arms_start.checked_add(r.arms_count as usize)?;
        let arms = arms_table.get(arms_start..arms_end)?.to_vec();
        out.push(Node {
            id: r.id,
            num: r.num,
            kind: r.kind,
            title: r.title,
            why: r.why,
            state: r.state,
            parent: r.parent,
            blocks: r.blocks,
            notes,
            outcome: r.outcome,
            refs: r.refs,
            governs: r.governs,
            opened: r.opened,
            closed: r.closed,
            forced_close: r.forced_close,
            flags,
            arms,
        });
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// The vivacs table: variable-length records, self-delimiting via their own
// length prefixes -- `Vivac` never shared `Tree`'s text arena, so there is
// nothing to intern here, only to write down.
// ---------------------------------------------------------------------------

fn write_vivac(buf: &mut Vec<u8>, v: &Vivac) {
    write_ulid(buf, &v.id);
    write_u64(buf, v.num);
    write_u64(buf, v.seq);
    write_u8(buf, vivac_kind_to_u8(v.kind));
    write_str(buf, &v.next_intent);
    write_str(buf, &v.anchor.kind);
    write_str(buf, &v.anchor.id);
    match &v.node_ref {
        Some(s) => {
            write_bool(buf, true);
            write_str(buf, s);
        }
        None => {
            write_bool(buf, false);
            write_str(buf, "");
        }
    }
    write_str(buf, &v.label);
    write_str(buf, &v.ts);
    write_u32(buf, v.stack.len() as u32);
    for (a, b) in &v.stack {
        write_str(buf, a);
        write_str(buf, b);
    }
    write_u32(buf, v.working_set.len() as u32);
    for w in &v.working_set {
        write_str(buf, w);
    }
}

fn parse_vivacs(bytes: &[u8], header: &Header) -> Option<Vec<Vivac>> {
    let mut c = Cursor::new(bytes.get(header.vivacs_offset as usize..)?);
    let mut out = Vec::with_capacity(header.vivac_count as usize);
    for _ in 0..header.vivac_count {
        let id = c.fixed_str(ULID_LEN)?;
        let num = c.u64()?;
        let seq = c.u64()?;
        let kind = u8_to_vivac_kind(c.u8()?)?;
        let next_intent = c.str()?;
        let anchor_kind = c.str()?;
        let anchor_id = c.str()?;
        let node_ref_present = c.bool_()?;
        let node_ref_raw = c.str()?;
        let node_ref = node_ref_present.then_some(node_ref_raw);
        let label = c.str()?;
        let ts = c.str()?;
        let stack_count = c.u32()?;
        let mut stack = Vec::with_capacity(stack_count as usize);
        for _ in 0..stack_count {
            let a = c.str()?;
            let b = c.str()?;
            stack.push((a, b));
        }
        let working_set_count = c.u32()?;
        let mut working_set = Vec::with_capacity(working_set_count as usize);
        for _ in 0..working_set_count {
            working_set.push(c.str()?);
        }
        out.push(Vivac {
            id,
            num,
            seq,
            kind,
            stack,
            working_set,
            next_intent,
            anchor: AnchorRef {
                kind: anchor_kind,
                id: anchor_id,
            },
            node_ref,
            label,
            ts,
        });
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// The remaining fixed-width sections, and putting it all together.
// ---------------------------------------------------------------------------

fn parse_nodes(bytes: &[u8], header: &Header) -> Option<Vec<NodeRaw>> {
    let mut c = Cursor::new(bytes.get(header.nodes_offset as usize..)?);
    let mut out = Vec::with_capacity(header.node_count as usize);
    for _ in 0..header.node_count {
        out.push(read_node_record(&mut c)?);
    }
    Some(out)
}

fn parse_flags(bytes: &[u8], header: &Header) -> Option<Vec<(Flag, Span)>> {
    let mut c = Cursor::new(bytes.get(header.flags_offset as usize..)?);
    let mut out = Vec::with_capacity(header.flags_count as usize);
    for _ in 0..header.flags_count {
        let tag = u8_to_flag(c.u8()?)?;
        let span = c.span()?;
        out.push((tag, span));
    }
    Some(out)
}

fn parse_notes(bytes: &[u8], header: &Header) -> Option<Vec<Note>> {
    let mut c = Cursor::new(bytes.get(header.notes_offset as usize..)?);
    let mut out = Vec::with_capacity(header.notes_count as usize);
    for _ in 0..header.notes_count {
        let at = c.span()?;
        let text = c.span()?;
        out.push(Note { at, text });
    }
    Some(out)
}

fn parse_spans(bytes: &[u8], header: &Header) -> Option<Vec<Span>> {
    let mut c = Cursor::new(bytes.get(header.spans_offset as usize..)?);
    let mut out = Vec::with_capacity(header.spans_count as usize);
    for _ in 0..header.spans_count {
        out.push(c.span()?);
    }
    Some(out)
}

/// The flat arms table: two spans per arm -- folder, then command -- in the
/// same per-node order `write_node_record` wrote them, so
/// `assemble_nodes`'s `[start..end]` slice lands on the right node's own
/// arms.
fn parse_arms(bytes: &[u8], header: &Header) -> Option<Vec<ArmSpan>> {
    let mut c = Cursor::new(bytes.get(header.arms_offset as usize..)?);
    let mut out = Vec::with_capacity(header.arms_count as usize);
    for _ in 0..header.arms_count {
        let dir = c.span()?;
        let command = c.span()?;
        out.push(ArmSpan { dir, command });
    }
    Some(out)
}

fn parse_u64_list(bytes: &[u8], offset: u64, count: u64) -> Option<Vec<u64>> {
    let mut c = Cursor::new(bytes.get(offset as usize..)?);
    let mut out = Vec::with_capacity(count as usize);
    for _ in 0..count {
        out.push(c.u64()?);
    }
    Some(out)
}

fn parse_text(bytes: &[u8], header: &Header) -> Option<String> {
    let start = header.text_offset as usize;
    let end = start.checked_add(header.text_len as usize)?;
    String::from_utf8(bytes.get(start..end)?.to_vec()).ok()
}

fn build_tree(bytes: &[u8], header: &Header) -> Option<Tree> {
    let raw_nodes = parse_nodes(bytes, header)?;
    let flags_table = parse_flags(bytes, header)?;
    let notes_table = parse_notes(bytes, header)?;
    let arms_table = parse_arms(bytes, header)?;
    let nodes = assemble_nodes(raw_nodes, &flags_table, &notes_table, &arms_table)?;
    let spans = parse_spans(bytes, header)?;
    let roots = parse_u64_list(bytes, header.roots_offset, header.roots_count)?;
    let stack = parse_u64_list(bytes, header.stack_offset, header.stack_count)?;
    let vivacs = parse_vivacs(bytes, header)?;
    let text = parse_text(bytes, header)?;
    Some(Tree::from_parts(RawParts {
        text,
        spans,
        nodes,
        roots,
        stack,
        vivacs,
        next_vivac_num: header.next_vivac_num,
        seq: header.seq,
        seq_change: header.seq_change,
        seq_vivac: header.seq_vivac,
        seg_new: header.seg_new,
        seg_closed: header.seg_closed,
        seg_notes: header.seg_notes,
        seg_events: header.seg_events,
        next_num: header.next_num,
        broken_lines: header.broken_lines as usize,
    }))
}

fn encode(
    tree: &Tree,
    fold_end_offset: u64,
    mtime_secs: i64,
    mtime_nanos: u32,
    last: Option<&LastEvent>,
) -> Vec<u8> {
    let nodes = tree.nodes_sorted();
    let mut nodes_buf = Vec::new();
    let mut flags_buf = Vec::new();
    let mut flags_cursor = 0u32;
    let mut notes_buf = Vec::new();
    let mut notes_cursor = 0u32;
    let mut arms_buf = Vec::new();
    let mut arms_cursor = 0u32;
    for n in &nodes {
        write_node_record(
            &mut nodes_buf,
            n,
            &mut flags_buf,
            &mut flags_cursor,
            &mut notes_buf,
            &mut notes_cursor,
            &mut arms_buf,
            &mut arms_cursor,
        );
    }
    let mut spans_buf = Vec::new();
    for &s in tree.raw_spans() {
        write_span(&mut spans_buf, s);
    }
    let mut roots_buf = Vec::new();
    for &r in &tree.roots {
        write_u64(&mut roots_buf, r);
    }
    let mut stack_buf = Vec::new();
    for &s in &tree.stack {
        write_u64(&mut stack_buf, s);
    }
    let mut vivacs_buf = Vec::new();
    for v in &tree.vivacs {
        write_vivac(&mut vivacs_buf, v);
    }
    let text = tree.raw_text();
    let text_bytes = text.as_bytes();

    let header_bytes = header_len() as u64;
    let nodes_offset = header_bytes;
    let spans_offset = nodes_offset + nodes_buf.len() as u64;
    let flags_offset = spans_offset + spans_buf.len() as u64;
    let notes_offset = flags_offset + flags_buf.len() as u64;
    let arms_offset = notes_offset + notes_buf.len() as u64;
    let roots_offset = arms_offset + arms_buf.len() as u64;
    let stack_offset = roots_offset + roots_buf.len() as u64;
    let vivacs_offset = stack_offset + stack_buf.len() as u64;
    let text_offset = vivacs_offset + vivacs_buf.len() as u64;
    let file_len = text_offset + text_bytes.len() as u64;

    let (has_last, last_line_offset, last_ulid, last_seq) = match last {
        Some(l) => (true, l.line_offset, l.id.clone(), l.seq),
        None => (false, 0, "0".repeat(ULID_LEN), 0),
    };

    let header = Header {
        seq: tree.seq,
        fold_end_offset,
        has_last,
        last_line_offset,
        last_ulid,
        last_seq,
        // A persisted index always represents the log up to exactly the
        // byte it finished reading: the two never disagree, or a future
        // freshness match could trust bytes this index never folded.
        log_len: fold_end_offset,
        mtime_secs,
        mtime_nanos,
        next_num: tree.next_num,
        next_vivac_num: tree.next_vivac_num,
        seq_change: tree.seq_change,
        seq_vivac: tree.seq_vivac,
        seg_new: tree.seg_new,
        seg_closed: tree.seg_closed,
        seg_notes: tree.seg_notes,
        seg_events: tree.seg_events,
        broken_lines: tree.broken_lines as u64,
        node_count: nodes.len() as u64,
        spans_count: tree.raw_spans().len() as u64,
        flags_count: (flags_buf.len() / FLAG_RECORD_LEN) as u64,
        notes_count: (notes_buf.len() / NOTE_RECORD_LEN) as u64,
        arms_count: (arms_buf.len() / ARM_RECORD_LEN) as u64,
        roots_count: tree.roots.len() as u64,
        stack_count: tree.stack.len() as u64,
        vivac_count: tree.vivacs.len() as u64,
        nodes_offset,
        spans_offset,
        flags_offset,
        notes_offset,
        arms_offset,
        roots_offset,
        stack_offset,
        vivacs_offset,
        text_offset,
        text_len: text_bytes.len() as u64,
        file_len,
    };

    let mut out = Vec::with_capacity(file_len as usize);
    write_header(&mut out, &header);
    debug_assert_eq!(out.len() as u64, header_bytes);
    out.extend_from_slice(&nodes_buf);
    out.extend_from_slice(&spans_buf);
    out.extend_from_slice(&flags_buf);
    out.extend_from_slice(&notes_buf);
    out.extend_from_slice(&arms_buf);
    out.extend_from_slice(&roots_buf);
    out.extend_from_slice(&stack_buf);
    out.extend_from_slice(&vivacs_buf);
    out.extend_from_slice(text_bytes);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anchor::AnchorRef;
    use crate::event::Body;

    fn tmp_store(name: &str) -> Store {
        let dir = std::env::temp_dir().join(format!(
            "vivac-index-t-{name}-{}-{}",
            std::process::id(),
            crate::id::ulid()
        ));
        Store::create(&dir).unwrap()
    }

    /// A deterministic stand-in for `id::ulid()`: every id this format
    /// stores is fixed-width, so a test fixture needs the same shape a real
    /// one has, not a short mnemonic like `"n1"`.
    fn fixed_id(n: u32) -> String {
        format!("{n:0>26}")
    }

    #[allow(clippy::too_many_arguments)]
    fn created(
        seq: u64,
        ulid: &str,
        num: u64,
        kind: Kind,
        parent: Option<&str>,
        title: &str,
        refs: Vec<String>,
        governs: Vec<String>,
    ) -> Event {
        Event {
            seq,
            id: fixed_id(seq as u32),
            ts: "2026-09-05T10:00:00Z".to_string(),
            actor: "a_test".to_string(),
            lane: "main".to_string(),
            payload: Body::NodeCreated {
                node: ulid.to_string(),
                num,
                kind,
                title: title.to_string(),
                why: "because it is needed".to_string(),
                parent: parent.map(str::to_string),
                blocks: false,
                refs,
                governs,
                arms: vec![],
            },
        }
    }

    /// Like `created`, with a rule's arms: the fixture
    /// `a_rule_round_trips_its_arms_through_the_index` needs, since
    /// `created` above stays the minimal shape every other fixture wants.
    /// `d441`: each arm is a folder and a command, not a bare string.
    #[allow(clippy::too_many_arguments)]
    fn created_with_arms(
        seq: u64,
        ulid: &str,
        num: u64,
        kind: Kind,
        parent: Option<&str>,
        title: &str,
        arms: Vec<crate::event::Arm>,
    ) -> Event {
        Event {
            seq,
            id: fixed_id(seq as u32),
            ts: "2026-09-05T10:00:00Z".to_string(),
            actor: "a_test".to_string(),
            lane: "main".to_string(),
            payload: Body::NodeCreated {
                node: ulid.to_string(),
                num,
                kind,
                title: title.to_string(),
                why: "because it is needed".to_string(),
                parent: parent.map(str::to_string),
                blocks: false,
                refs: vec![],
                governs: vec![],
                arms,
            },
        }
    }

    fn a_note(seq: u64, ulid: &str, note: &str) -> Event {
        a_note_at(seq, ulid, "2026-09-05T10:01:00Z", note)
    }

    /// Like `a_note`, but with its own `ts`: the fixture two distinct notes
    /// on the same node need to prove each keeps the moment it was written.
    fn a_note_at(seq: u64, ulid: &str, ts: &str, note: &str) -> Event {
        Event {
            seq,
            id: fixed_id(seq as u32),
            ts: ts.to_string(),
            actor: "a_test".to_string(),
            lane: "main".to_string(),
            payload: Body::NodeNoted {
                node: ulid.to_string(),
                note: note.to_string(),
            },
        }
    }

    fn a_flag(seq: u64, ulid: &str, flag: Flag, reason: &str) -> Event {
        Event {
            seq,
            id: fixed_id(seq as u32),
            ts: "2026-09-05T10:02:00Z".to_string(),
            actor: "a_test".to_string(),
            lane: "main".to_string(),
            payload: Body::FlagRaised {
                node: ulid.to_string(),
                flag,
                reason: reason.to_string(),
            },
        }
    }

    fn a_close(seq: u64, ulid: &str, outcome: &str) -> Event {
        Event {
            seq,
            id: fixed_id(seq as u32),
            ts: "2026-09-05T10:03:00Z".to_string(),
            actor: "a_test".to_string(),
            lane: "main".to_string(),
            payload: Body::StateChanged {
                node: ulid.to_string(),
                state: State::Done,
                outcome: outcome.to_string(),
                forced: false,
            },
        }
    }

    fn a_vivac(seq: u64, num: u64, root_id: &str) -> Event {
        Event {
            seq,
            id: fixed_id(seq as u32),
            ts: "2026-09-05T10:04:00Z".to_string(),
            actor: "a_test".to_string(),
            lane: "main".to_string(),
            payload: Body::VivacCreated {
                vivac: fixed_id(900 + seq as u32),
                num,
                kind: VivacKind::Manual,
                stack: vec![(root_id.to_string(), "Root".to_string())],
                working_set: vec!["src/lib.rs".to_string()],
                next_intent: "keep going".to_string(),
                anchor: AnchorRef {
                    kind: "git".to_string(),
                    id: "abc123".to_string(),
                },
                node_ref: Some(root_id.to_string()),
                label: "a stop".to_string(),
            },
        }
    }

    /// A dump of everything a command can observe about a `Tree`, so two
    /// trees built two different ways can be compared for equality without
    /// `Tree` itself needing to derive it.
    fn snapshot(tree: &Tree) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "seq={} seq_change={} seq_vivac={} next_num={} next_vivac_num={} broken={} \
             seg_new={} seg_closed={} seg_notes={} seg_events={} total={}\n",
            tree.seq,
            tree.seq_change,
            tree.seq_vivac,
            tree.next_num,
            tree.next_vivac_num,
            tree.broken_lines,
            tree.seg_new,
            tree.seg_closed,
            tree.seg_notes,
            tree.seg_events,
            tree.total(),
        ));
        out.push_str(&format!("roots={:?}\n", tree.roots));
        out.push_str(&format!("stack={:?}\n", tree.stack));
        out.push_str(&format!("repeated_nums={}\n", tree.repeated_nums.len()));
        for n in tree.nodes_sorted() {
            out.push_str(&format!(
                "node num={} id={} kind={:?} state={:?} parent={:?} blocks={} forced={} \
                 title={:?} why={:?} note={:?} outcome={:?} opened={:?} closed={:?} \
                 refs={:?} governs={:?} flags={:?} arms={:?}\n",
                n.num,
                n.id,
                n.kind,
                n.state,
                n.parent,
                n.blocks,
                n.forced_close,
                n.title(tree),
                n.why(tree),
                n.note(tree),
                n.outcome(tree),
                n.opened(tree),
                n.closed(tree),
                n.refs(tree),
                n.governs(tree),
                n.flags
                    .iter()
                    .map(|(f, s)| (f.word(), tree.text(*s)))
                    .collect::<Vec<_>>(),
                n.arms(tree),
            ));
        }
        for v in &tree.vivacs {
            out.push_str(&format!(
                "vivac num={} id={} seq={} kind={:?} stack={:?} working_set={:?} \
                 next_intent={:?} anchor={:?} node_ref={:?} label={:?} ts={:?}\n",
                v.num,
                v.id,
                v.seq,
                v.kind,
                v.stack,
                v.working_set,
                v.next_intent,
                v.anchor,
                v.node_ref,
                v.label,
                v.ts,
            ));
        }
        out
    }

    fn a_varied_event_set() -> Vec<Event> {
        let root_id = fixed_id(1);
        let child_id = fixed_id(2);
        vec![
            created(
                1,
                &root_id,
                1,
                Kind::Goal,
                None,
                "Root goal",
                vec!["ref-a".to_string()],
                vec!["governs-a".to_string()],
            ),
            created(
                2,
                &child_id,
                2,
                Kind::Task,
                Some(&root_id),
                "Child task",
                vec![],
                vec![],
            ),
            a_note(3, &child_id, "a note on the child"),
            a_flag(4, &child_id, Flag::Suspect, "something fell over"),
            a_flag(5, &child_id, Flag::Review, "worth a second look"),
            a_close(6, &child_id, "done for now"),
            a_vivac(7, 1, &root_id),
        ]
    }

    #[test]
    fn read_tracked_agrees_with_store_read_all() {
        let store = tmp_store("agree");
        store.write_raw(&a_varied_event_set()).unwrap();
        let (want_events, want_broken) = store.read_all().unwrap();
        let got = read_tracked(&store.log(), 0).unwrap();
        assert_eq!(got.broken, want_broken);
        assert_eq!(got.events.len(), want_events.len());
        for (a, b) in got.events.iter().zip(want_events.iter()) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.seq, b.seq);
        }
        std::fs::remove_dir_all(&store.root).ok();
    }

    #[test]
    fn round_trip_preserves_everything_a_command_can_observe() {
        let events = a_varied_event_set();
        let fresh = fold(&events, 0);

        let store = tmp_store("roundtrip");
        store.write_raw(&events).unwrap();

        let loaded = load(&store, true).expect("load should succeed");
        assert_eq!(snapshot(&fresh), snapshot(&loaded));
        assert!(
            store.index_path().is_file(),
            "a clean fold should be indexed"
        );

        // And loading again, now purely from the index (no tail to apply),
        // has to agree too.
        let loaded_again = load(&store, false).expect("load should succeed");
        assert_eq!(snapshot(&fresh), snapshot(&loaded_again));

        std::fs::remove_dir_all(&store.root).ok();
    }

    /// `d390`: `note` went from one `Span` to a flat table the node table
    /// points into, the same shape change `flags` already went through.
    ///
    /// This calls `encode`/`build_tree` directly rather than through `load`:
    /// `load` falls back to folding the log whenever the index fails to
    /// parse (`LOADING.md` §4 "nunca falla"), and the log it would fall back
    /// to is sitting right there, untouched, with the same two notes in it.
    /// A `parse`/`write_header` field landing out of step could come back
    /// `None` and hide behind that fallback with the surrounding suite still
    /// green. Going straight at the encoded bytes leaves nowhere to hide.
    #[test]
    fn two_notes_on_one_node_round_trip_through_the_index_alone() {
        let root_id = fixed_id(1);
        let events = vec![
            created(1, &root_id, 1, Kind::Task, None, "Root", vec![], vec![]),
            a_note_at(2, &root_id, "2026-09-01T00:00:00Z", "first note"),
            a_note_at(3, &root_id, "2026-09-02T00:00:00Z", "second note"),
        ];
        let tree = fold(&events, 0);

        let bytes = encode(&tree, 0, 0, 0, None);
        let header = Header::parse(&bytes).expect("the header this test just wrote parses");
        let loaded = build_tree(&bytes, &header).expect("the body this test just wrote parses");

        let n = loaded.node(&root_id).expect("the node is in the index");
        assert_eq!(
            n.notes(&loaded),
            vec![
                ("2026-09-01T00:00:00Z", "first note"),
                ("2026-09-02T00:00:00Z", "second note"),
            ],
            "both notes, oldest first and each with its own date, survive \
             reading the encoded bytes back"
        );
        assert_eq!(n.note(&loaded), "second note");
    }

    /// `t411`: a rule's `arms` is a new field on `Node`, stored the same way
    /// `d390` proved for notes -- a flat table the node record points into.
    /// Same direct `encode`/`build_tree` call as the note round trip above,
    /// and for the same reason: `load`'s fallback to folding the log must
    /// not be the thing that hides a misplaced field. A pillar carries no
    /// field of its own (`d436`), so it rides along here only to prove its
    /// presence changes nothing about the rule beside it.
    #[test]
    fn a_pillar_and_a_rule_round_trip_through_the_index_alone() {
        let pillar_id = fixed_id(1);
        let rule_id = fixed_id(2);
        let events = vec![
            created(
                1,
                &pillar_id,
                1,
                Kind::Pillar,
                None,
                "Security",
                vec![],
                vec![],
            ),
            created_with_arms(
                2,
                &rule_id,
                2,
                Kind::Rule,
                Some(&pillar_id),
                "Never store a secret",
                vec![crate::event::Arm {
                    dir: "vivac".to_string(),
                    command: "cargo test --bin vivac redact::tests".to_string(),
                }],
            ),
        ];
        let tree = fold(&events, 0);

        let bytes = encode(&tree, 0, 0, 0, None);
        let header = Header::parse(&bytes).expect("the header this test just wrote parses");
        let loaded = build_tree(&bytes, &header).expect("the body this test just wrote parses");

        let pillar = loaded.node(&pillar_id).expect("the pillar is in the index");
        assert_eq!(pillar.arms(&loaded), Vec::<(&str, &str)>::new());
        let rule = loaded.node(&rule_id).expect("the rule is in the index");
        assert_eq!(
            rule.arms(&loaded),
            vec![("vivac", "cargo test --bin vivac redact::tests")]
        );
    }

    /// Rewrites just the header of an already-persisted index, keeping the
    /// body untouched, so a single field can be corrupted without knowing
    /// its byte position by hand.
    fn rewrite_header(store: &Store, edit: impl FnOnce(&mut Header)) {
        let bytes = fs::read(store.index_path()).unwrap();
        let mut header = Header::parse(&bytes).expect("the index this test just wrote parses");
        edit(&mut header);
        let mut out = Vec::new();
        write_header(&mut out, &header);
        out.extend_from_slice(&bytes[header_len()..]);
        fs::write(store.index_path(), &out).unwrap();
    }

    #[test]
    fn a_corrupt_index_is_regenerated_rather_than_trusted() {
        let store = tmp_store("corrupt");
        let events = a_varied_event_set();
        store.write_raw(&events).unwrap();
        let want = fold(&events, 0);

        // Bad magic: the very first byte of every valid index.
        load(&store, true).unwrap();
        let mut bytes = fs::read(store.index_path()).unwrap();
        bytes[0] ^= 0xFF;
        fs::write(store.index_path(), &bytes).unwrap();
        assert_eq!(snapshot(&want), snapshot(&load(&store, false).unwrap()));

        // Truncated mid-header.
        load(&store, true).unwrap();
        let bytes = fs::read(store.index_path()).unwrap();
        fs::write(store.index_path(), &bytes[..bytes.len() / 2]).unwrap();
        assert_eq!(snapshot(&want), snapshot(&load(&store, false).unwrap()));

        // Wrong format version: the four bytes right after the eight-byte
        // magic, in every valid index.
        load(&store, true).unwrap();
        let mut bytes = fs::read(store.index_path()).unwrap();
        bytes[8..12].copy_from_slice(&999u32.to_le_bytes());
        fs::write(store.index_path(), &bytes).unwrap();
        assert_eq!(snapshot(&want), snapshot(&load(&store, false).unwrap()));

        // An offset pointing outside the file.
        load(&store, true).unwrap();
        rewrite_header(&store, |h| h.nodes_offset = h.file_len * 100);
        assert_eq!(snapshot(&want), snapshot(&load(&store, false).unwrap()));

        std::fs::remove_dir_all(&store.root).ok();
    }

    #[test]
    fn a_stale_index_picks_up_the_tail() {
        let store = tmp_store("stale");
        let events = a_varied_event_set();
        store.write_raw(&events).unwrap();
        load(&store, true).unwrap();
        assert!(store.index_path().is_file());

        let more = vec![created(
            8,
            &fixed_id(3),
            3,
            Kind::Task,
            Some(&fixed_id(1)),
            "A node born after the index",
            vec![],
            vec![],
        )];
        store.write_raw(&more).unwrap();

        let (all_events, broken) = store.read_all().unwrap();
        let want = fold(&all_events, broken);

        let got = load(&store, false).unwrap();
        assert_eq!(snapshot(&want), snapshot(&got));
        assert_eq!(got.total(), 3);

        std::fs::remove_dir_all(&store.root).ok();
    }

    #[test]
    fn a_log_with_a_repeated_number_is_never_indexed() {
        let store = tmp_store("repeated");
        let events = vec![
            created(
                1,
                &fixed_id(1),
                1,
                Kind::Task,
                None,
                "First",
                vec![],
                vec![],
            ),
            created(
                2,
                &fixed_id(2),
                1,
                Kind::Finding,
                None,
                "Second claims the same num",
                vec![],
                vec![],
            ),
        ];
        store.write_raw(&events).unwrap();

        load(&store, true).unwrap();
        assert!(
            !store.index_path().exists(),
            "a log with a repeated num must not be indexed"
        );
    }

    #[test]
    fn a_log_with_a_pending_reference_is_never_indexed() {
        let store = tmp_store("pending");
        // The child names a parent that never arrives.
        let events = vec![created(
            1,
            &fixed_id(1),
            1,
            Kind::Task,
            Some("ghost-parent-that-never-arrives"),
            "Orphaned child",
            vec![],
            vec![],
        )];
        store.write_raw(&events).unwrap();

        load(&store, true).unwrap();
        assert!(
            !store.index_path().exists(),
            "a log with a pending reference must not be indexed"
        );
    }

    #[test]
    fn deleting_the_index_changes_nothing() {
        let store = tmp_store("delete");
        let events = a_varied_event_set();
        store.write_raw(&events).unwrap();
        load(&store, true).unwrap();
        assert!(store.index_path().is_file());

        let with_index = load(&store, false).unwrap();
        fs::remove_file(store.index_path()).unwrap();
        let without_index = load(&store, false).unwrap();
        assert_eq!(snapshot(&with_index), snapshot(&without_index));

        std::fs::remove_dir_all(&store.root).ok();
    }

    #[test]
    fn a_write_never_persists_the_index() {
        let store = tmp_store("writeonly");
        let events = a_varied_event_set();
        store.write_raw(&events).unwrap();
        load(&store, false).unwrap();
        assert!(
            !store.index_path().exists(),
            "allow_persist=false must never create the index"
        );
        std::fs::remove_dir_all(&store.root).ok();
    }
}
