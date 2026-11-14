//! Durable on-disk buffer for measured values.
//!
//! Layout inside the spool directory:
//!
//! - `NNNNNNNN.jsonl`: numbered segment files, one JSON `MetricValue` per
//!   line. Only newline-terminated lines count; a trailing fragment left by a
//!   crash in the middle of a write is ignored and never extended.
//! - `cursor`: text `"<segment> <line>\n"`, the position of the oldest
//!   unacknowledged line. It is replaced atomically (temp file, sync, rename).
//!
//! The spool is synchronous; share it as `Arc<std::sync::Mutex<Spool>>`.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use cerbrix_common::MetricValue;

/// A new segment is started once the current one has reached this size.
pub const SEGMENT_BYTES: u64 = 1024 * 1024;

const CURSOR_FILE: &str = "cursor";
const CURSOR_TMP_FILE: &str = "cursor.tmp";
const SEGMENT_SUFFIX: &str = ".jsonl";

/// Position of a line: segment number and zero-based line index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Pos {
    segment: u64,
    line: usize,
}

#[derive(Debug)]
struct SegmentInfo {
    size: u64,
    /// Number of complete lines. Always known for the newest segment, looked
    /// up lazily for older (immutable) ones.
    lines: Option<usize>,
}

#[derive(Debug)]
struct Writer {
    segment: u64,
    file: File,
}

/// Durable FIFO of metric values.
#[derive(Debug)]
pub struct Spool {
    dir: PathBuf,
    max_bytes: u64,
    segments: BTreeMap<u64, SegmentInfo>,
    cursor: Pos,
    writer: Option<Writer>,
    /// Number for the next segment that gets created.
    next_segment: u64,
    /// For each value of the last peek that is not yet acknowledged: the
    /// position right after it.
    pending: Option<Vec<Pos>>,
}

impl Spool {
    /// Opens (and creates, if needed) the spool in `dir`.
    pub fn open(dir: &Path, max_bytes: u64) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        match fs::remove_file(dir.join(CURSOR_TMP_FILE)) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }

        let mut segments = BTreeMap::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let Some(number) = entry.file_name().to_str().and_then(parse_segment_name) else {
                continue;
            };
            let meta = entry.metadata()?;
            if meta.is_file() {
                segments.insert(
                    number,
                    SegmentInfo {
                        size: meta.len(),
                        lines: None,
                    },
                );
            }
        }

        let loaded = read_cursor(dir);
        let mut spool = Spool {
            dir: dir.to_owned(),
            max_bytes,
            segments,
            cursor: Pos {
                segment: 1,
                line: 0,
            },
            writer: None,
            next_segment: 1,
            pending: None,
        };

        let Some(&oldest) = spool.segments.keys().next() else {
            return Ok(spool);
        };
        let newest = *spool.segments.keys().next_back().expect("not empty");

        // Count the lines of the newest segment and decide whether to append.
        let (lines, complete) = count_lines(&spool.segment_path(newest))?;
        spool.segments.get_mut(&newest).expect("exists").lines = Some(lines);
        if complete {
            let file = OpenOptions::new()
                .append(true)
                .open(spool.segment_path(newest))?;
            spool.writer = Some(Writer {
                segment: newest,
                file,
            });
            spool.next_segment = newest + 1;
        } else {
            tracing::warn!(
                segment = newest,
                "spool segment ends in an incomplete line; starting a new segment"
            );
            spool.next_segment = newest + 1;
        }

        let start = match loaded {
            Cursor::Valid(pos) if spool.segments.contains_key(&pos.segment) => pos,
            Cursor::Valid(pos) => {
                tracing::warn!(
                    segment = pos.segment,
                    "spool cursor points at a missing segment; starting at the oldest one"
                );
                Pos {
                    segment: oldest,
                    line: 0,
                }
            }
            Cursor::Missing => Pos {
                segment: oldest,
                line: 0,
            },
            Cursor::Corrupt => {
                tracing::warn!("spool cursor is corrupt; starting at the oldest segment");
                Pos {
                    segment: oldest,
                    line: 0,
                }
            }
        };
        spool.cursor = start;
        spool.normalise_cursor()?;
        if let Cursor::Valid(pos) = loaded
            && pos != spool.cursor
        {
            spool.write_cursor()?;
        }
        spool.delete_consumed_segments();
        Ok(spool)
    }

    /// Appends values. They are in the OS buffers (not fsynced) on return.
    pub fn push(&mut self, values: &[MetricValue]) -> io::Result<()> {
        if values.is_empty() {
            return Ok(());
        }
        let mut buf = Vec::with_capacity(values.len() * 128);
        for value in values {
            serde_json::to_writer(&mut buf, value).map_err(io::Error::other)?;
            buf.push(b'\n');
        }

        if self
            .writer
            .as_ref()
            .is_some_and(|w| self.segments[&w.segment].size >= SEGMENT_BYTES)
        {
            self.writer = None; // closes the file
        }
        let writer = match self.writer.take() {
            Some(writer) => writer,
            None => self.create_segment()?,
        };
        self.writer = Some(writer);
        let writer = self.writer.as_mut().expect("just set");
        let segment = writer.segment;

        let result = writer
            .file
            .write_all(&buf)
            .and_then(|()| writer.file.flush());
        match result {
            Ok(()) => {
                let info = self.segments.get_mut(&segment).expect("writer segment");
                info.size += buf.len() as u64;
                if let Some(lines) = &mut info.lines {
                    *lines += values.len();
                }
            }
            Err(e) => {
                // A partial line may be on disk: never append to this segment again.
                self.writer = None;
                self.next_segment = segment + 1;
                let path = self.segment_path(segment);
                let info = self.segments.get_mut(&segment).expect("writer segment");
                if let Ok(meta) = fs::metadata(&path) {
                    info.size = meta.len();
                }
                info.lines = count_lines(&path).ok().map(|(lines, _)| lines);
                return Err(e);
            }
        }
        self.enforce_limit()
    }

    /// Returns up to `max` of the oldest unacknowledged values without
    /// removing them. Lines that are not valid JSON values are skipped.
    pub fn peek(&mut self, max: usize) -> io::Result<Vec<MetricValue>> {
        let mut values = Vec::new();
        let mut positions = Vec::new();
        'segments: for (&number, _) in self.segments.range(self.cursor.segment..) {
            if values.len() >= max {
                break;
            }
            let data = match fs::read(self.segment_path(number)) {
                Ok(data) => data,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            };
            let skip = if number == self.cursor.segment {
                self.cursor.line
            } else {
                0
            };
            // Everything after the last newline is an incomplete line.
            let Some(last_newline) = data.iter().rposition(|&b| b == b'\n') else {
                continue;
            };
            for (index, line) in data[..last_newline]
                .split(|&b| b == b'\n')
                .enumerate()
                .skip(skip)
            {
                if values.len() >= max {
                    break 'segments;
                }
                match serde_json::from_slice::<MetricValue>(line) {
                    Ok(value) => {
                        values.push(value);
                        positions.push(Pos {
                            segment: number,
                            line: index + 1,
                        });
                    }
                    Err(e) => {
                        tracing::warn!(segment = number, line = index, error = %e,
                            "skipping unparseable spool line");
                    }
                }
            }
        }
        self.pending = Some(positions);
        Ok(values)
    }

    /// Acknowledges the `count` oldest values of the last [`peek`](Self::peek).
    ///
    /// Fails with `InvalidInput`, changing nothing, if there is no pending
    /// peek or it has fewer than `count` values left. Once it returns, the
    /// values stay gone across restarts.
    pub fn ack(&mut self, count: usize) -> io::Result<()> {
        if count == 0 {
            return Ok(());
        }
        let available = self.pending.as_ref().map_or(0, Vec::len);
        if count > available {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("ack of {count} values but the last peek has {available} left"),
            ));
        }
        let new_cursor = self.pending.as_ref().expect("checked above")[count - 1];
        let old_cursor = self.cursor;

        self.cursor = new_cursor;
        let result = self.normalise_cursor().and_then(|()| self.write_cursor());
        if let Err(e) = result {
            // Not durable: keep the values pending.
            self.cursor = old_cursor;
            return Err(e);
        }
        if let Some(pending) = &mut self.pending {
            pending.drain(..count);
        }
        self.delete_consumed_segments();
        Ok(())
    }

    /// Total size of all segment files.
    pub fn size_bytes(&self) -> u64 {
        self.segments.values().map(|s| s.size).sum()
    }

    /// True if there is no unacknowledged value.
    pub fn is_empty(&self) -> bool {
        let Some((&newest, info)) = self.segments.iter().next_back() else {
            return true;
        };
        self.cursor.segment == newest && info.lines.is_some_and(|lines| self.cursor.line >= lines)
    }

    fn segment_path(&self, number: u64) -> PathBuf {
        self.dir.join(segment_name(number))
    }

    fn create_segment(&mut self) -> io::Result<Writer> {
        let number = self.next_segment;
        let file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(self.segment_path(number))?;
        self.next_segment = number + 1;
        if self.segments.is_empty() {
            self.cursor = Pos {
                segment: number,
                line: 0,
            };
        }
        self.segments.insert(
            number,
            SegmentInfo {
                size: 0,
                lines: Some(0),
            },
        );
        Ok(Writer {
            segment: number,
            file,
        })
    }

    /// Line count of a segment, read from disk if not known yet.
    fn lines_of(&mut self, number: u64) -> io::Result<usize> {
        if let Some(lines) = self.segments[&number].lines {
            return Ok(lines);
        }
        let (lines, _) = count_lines(&self.segment_path(number))?;
        self.segments.get_mut(&number).expect("exists").lines = Some(lines);
        Ok(lines)
    }

    /// Moves the cursor past fully consumed segments (all but the newest one)
    /// and keeps it inside the newest segment.
    fn normalise_cursor(&mut self) -> io::Result<()> {
        loop {
            let Some(&newest) = self.segments.keys().next_back() else {
                return Ok(());
            };
            let lines = self.lines_of(self.cursor.segment)?;
            if self.cursor.segment == newest {
                self.cursor.line = self.cursor.line.min(lines);
                return Ok(());
            }
            if self.cursor.line < lines {
                return Ok(());
            }
            let next = *self
                .segments
                .range(self.cursor.segment + 1..)
                .next()
                .expect("a newer segment exists")
                .0;
            self.cursor = Pos {
                segment: next,
                line: 0,
            };
        }
    }

    /// Deletes every segment older than the cursor's. Failures are logged and
    /// retried on the next call.
    fn delete_consumed_segments(&mut self) {
        let consumed: Vec<u64> = self
            .segments
            .range(..self.cursor.segment)
            .map(|(&n, _)| n)
            .collect();
        for number in consumed {
            match fs::remove_file(self.segment_path(number)) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => {
                    tracing::warn!(segment = number, error = %e, "cannot delete spool segment");
                    continue;
                }
            }
            self.segments.remove(&number);
        }
    }

    /// Persists the cursor: temp file, flush, sync, rename.
    fn write_cursor(&self) -> io::Result<()> {
        let tmp = self.dir.join(CURSOR_TMP_FILE);
        let mut file = File::create(&tmp)?;
        file.write_all(format!("{} {}\n", self.cursor.segment, self.cursor.line).as_bytes())?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, self.dir.join(CURSOR_FILE))?;
        sync_dir(&self.dir);
        Ok(())
    }

    /// Deletes the oldest segments while the spool is over its limit. The
    /// newest segment is never deleted.
    fn enforce_limit(&mut self) -> io::Result<()> {
        let mut dropped_values = 0usize;
        let mut dropped_segments = 0usize;
        while self.size_bytes() > self.max_bytes && self.segments.len() > 1 {
            let oldest = *self.segments.keys().next().expect("not empty");
            let lines = self.lines_of(oldest).unwrap_or(0);
            match fs::remove_file(self.segment_path(oldest)) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => {
                    tracing::warn!(segment = oldest, error = %e, "cannot delete spool segment");
                    break;
                }
            }
            self.segments.remove(&oldest);
            dropped_segments += 1;
            dropped_values += if oldest == self.cursor.segment {
                lines.saturating_sub(self.cursor.line)
            } else {
                lines
            };
        }
        if dropped_segments == 0 {
            return Ok(());
        }
        tracing::warn!(
            dropped_values,
            dropped_segments,
            "spool over its size limit; dropped the oldest values"
        );
        self.pending = None;
        let oldest = *self.segments.keys().next().expect("newest is kept");
        if self.cursor.segment < oldest {
            self.cursor = Pos {
                segment: oldest,
                line: 0,
            };
            self.write_cursor()?;
        }
        Ok(())
    }
}

enum Cursor {
    Valid(Pos),
    Missing,
    Corrupt,
}

fn read_cursor(dir: &Path) -> Cursor {
    let data = match fs::read(dir.join(CURSOR_FILE)) {
        Ok(data) => data,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Cursor::Missing,
        Err(e) => {
            tracing::warn!(error = %e, "cannot read spool cursor");
            return Cursor::Corrupt;
        }
    };
    let parsed = std::str::from_utf8(&data).ok().and_then(|text| {
        let mut parts = text.split_whitespace();
        let segment = parts.next()?.parse().ok()?;
        let line = parts.next()?.parse().ok()?;
        parts.next().is_none().then_some(Pos { segment, line })
    });
    parsed.map_or(Cursor::Corrupt, Cursor::Valid)
}

fn segment_name(number: u64) -> String {
    format!("{number:08}{SEGMENT_SUFFIX}")
}

/// Parses `NNNNNNNN.jsonl` (exactly eight digits).
fn parse_segment_name(name: &str) -> Option<u64> {
    let digits = name.strip_suffix(SEGMENT_SUFFIX)?;
    if digits.len() == 8 && digits.bytes().all(|b| b.is_ascii_digit()) {
        digits.parse().ok()
    } else {
        None
    }
}

/// Returns the number of complete lines and whether the file is empty or ends
/// with a newline.
fn count_lines(path: &Path) -> io::Result<(usize, bool)> {
    let data = fs::read(path)?;
    let lines = data.iter().filter(|&&b| b == b'\n').count();
    Ok((lines, data.last().is_none_or(|&b| b == b'\n')))
}

#[cfg(unix)]
fn sync_dir(dir: &Path) {
    if let Ok(file) = File::open(dir) {
        let _ = file.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;
    use std::io::{ErrorKind, Write};

    fn value(n: usize) -> MetricValue {
        MetricValue {
            key: "test.value".into(),
            params: BTreeMap::new(),
            ts_ms: 1_700_000_000_000 + n as i64,
            value: n as f64,
        }
    }

    fn fat_value(n: usize) -> MetricValue {
        let mut v = value(n);
        v.params.insert("mount".into(), "x".repeat(250));
        v
    }

    fn values(range: std::ops::Range<usize>) -> Vec<MetricValue> {
        range.map(value).collect()
    }

    fn fat_values(range: std::ops::Range<usize>) -> Vec<MetricValue> {
        range.map(fat_value).collect()
    }

    fn segment_files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.ends_with(".jsonl"))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn push_peek_ack_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let mut spool = Spool::open(dir.path(), 10 * SEGMENT_BYTES).unwrap();
        assert!(spool.is_empty());
        spool.push(&values(0..5)).unwrap();
        assert!(!spool.is_empty());
        assert_eq!(spool.peek(3).unwrap(), values(0..3));
        spool.ack(3).unwrap();
        assert_eq!(spool.peek(10).unwrap(), values(3..5));
        spool.ack(2).unwrap();
        assert!(spool.is_empty());
        assert!(spool.peek(10).unwrap().is_empty());
    }

    #[test]
    fn peek_without_ack_returns_the_same_values() {
        let dir = tempfile::tempdir().unwrap();
        let mut spool = Spool::open(dir.path(), 10 * SEGMENT_BYTES).unwrap();
        spool.push(&values(0..4)).unwrap();
        let first = spool.peek(4).unwrap();
        assert_eq!(spool.peek(4).unwrap(), first);
        assert_eq!(first, values(0..4));
    }

    #[test]
    fn survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut spool = Spool::open(dir.path(), 10 * SEGMENT_BYTES).unwrap();
            spool.push(&values(0..5)).unwrap();
            spool.peek(2).unwrap();
            spool.ack(2).unwrap();
        }
        let mut spool = Spool::open(dir.path(), 10 * SEGMENT_BYTES).unwrap();
        assert_eq!(spool.peek(10).unwrap(), values(2..5));
        // New values go after the old ones.
        spool.push(&values(5..6)).unwrap();
        assert_eq!(spool.peek(10).unwrap(), values(2..6));
    }

    #[test]
    fn truncated_last_line_is_skipped_on_reopen() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut spool = Spool::open(dir.path(), 10 * SEGMENT_BYTES).unwrap();
            spool.push(&values(0..3)).unwrap();
        }
        let path = dir.path().join("00000001.jsonl");
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(br#"{"key":"test.val"#).unwrap();
        drop(file);

        let mut spool = Spool::open(dir.path(), 10 * SEGMENT_BYTES).unwrap();
        assert_eq!(spool.peek(10).unwrap(), values(0..3));
        // A new value is not glued to the fragment.
        spool.push(&values(3..4)).unwrap();
        assert_eq!(spool.peek(10).unwrap(), values(0..4));
        drop(spool);
        let mut spool = Spool::open(dir.path(), 10 * SEGMENT_BYTES).unwrap();
        assert_eq!(spool.peek(10).unwrap(), values(0..4));
    }

    #[test]
    fn unparseable_lines_are_skipped_and_consumed_by_ack() {
        let dir = tempfile::tempdir().unwrap();
        let good = |n: usize| serde_json::to_string(&value(n)).unwrap();
        let content = format!("{}\nnot json\n{}\n", good(0), good(1));
        fs::write(dir.path().join("00000001.jsonl"), content).unwrap();
        let mut spool = Spool::open(dir.path(), 10 * SEGMENT_BYTES).unwrap();
        assert_eq!(spool.peek(10).unwrap(), values(0..2));
        spool.ack(2).unwrap();
        assert!(spool.is_empty());
    }

    #[test]
    fn rotates_segments_and_drops_oldest_over_limit() {
        let dir = tempfile::tempdir().unwrap();
        let max = 3 * SEGMENT_BYTES;
        let mut spool = Spool::open(dir.path(), max).unwrap();
        // About 300 bytes per value: 17 batches of 1000 values are roughly 5 MiB.
        let mut n = 0;
        while n < 17_000 {
            spool.push(&fat_values(n..n + 1000)).unwrap();
            n += 1000;
            assert!(spool.size_bytes() <= max);
        }
        assert!(segment_files(dir.path()).len() <= 3);
        let first = spool.peek(1).unwrap();
        assert!(first[0].value > 0.0, "oldest values must have been dropped");
        // What is still there is contiguous and ends with the newest value.
        let all = spool.peek(usize::MAX).unwrap();
        assert_eq!(all.last().unwrap().value, (n - 1) as f64);
        for pair in all.windows(2) {
            assert_eq!(pair[1].value, pair[0].value + 1.0);
        }
    }

    #[test]
    fn acknowledged_segments_are_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let mut spool = Spool::open(dir.path(), 100 * SEGMENT_BYTES).unwrap();
        spool.push(&fat_values(0..4000)).unwrap(); // more than 1 MiB
        spool.push(&[fat_value(4000)]).unwrap(); // goes to segment 2
        assert_eq!(segment_files(dir.path()).len(), 2);
        assert_eq!(spool.peek(4000).unwrap().len(), 4000);
        spool.ack(4000).unwrap();
        assert_eq!(segment_files(dir.path()), vec!["00000002.jsonl"]);
        assert_eq!(spool.peek(10).unwrap(), vec![fat_value(4000)]);
        // Same after a reopen.
        drop(spool);
        let mut spool = Spool::open(dir.path(), 100 * SEGMENT_BYTES).unwrap();
        assert_eq!(spool.peek(10).unwrap(), vec![fat_value(4000)]);
    }

    #[test]
    fn ack_more_than_peeked_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut spool = Spool::open(dir.path(), 10 * SEGMENT_BYTES).unwrap();
        spool.push(&values(0..5)).unwrap();
        spool.peek(2).unwrap();
        let err = spool.ack(3).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        // Nothing changed.
        assert_eq!(spool.peek(10).unwrap(), values(0..5));
        spool.ack(0).unwrap();
        assert_eq!(spool.peek(10).unwrap(), values(0..5));
    }

    #[test]
    fn ack_without_peek_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut spool = Spool::open(dir.path(), 10 * SEGMENT_BYTES).unwrap();
        spool.push(&values(0..2)).unwrap();
        assert_eq!(spool.ack(1).unwrap_err().kind(), ErrorKind::InvalidInput);
        spool.peek(2).unwrap();
        spool.ack(2).unwrap();
        // The peek is used up.
        assert_eq!(spool.ack(1).unwrap_err().kind(), ErrorKind::InvalidInput);
    }

    #[test]
    fn push_between_peek_and_ack_keeps_the_peek_valid() {
        let dir = tempfile::tempdir().unwrap();
        let mut spool = Spool::open(dir.path(), 10 * SEGMENT_BYTES).unwrap();
        spool.push(&values(0..3)).unwrap();
        assert_eq!(spool.peek(3).unwrap(), values(0..3));
        spool.push(&values(3..5)).unwrap();
        spool.ack(3).unwrap();
        assert_eq!(spool.peek(10).unwrap(), values(3..5));
    }

    #[test]
    fn peek_crosses_segment_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let mut spool = Spool::open(dir.path(), 100 * SEGMENT_BYTES).unwrap();
        spool.push(&fat_values(0..4000)).unwrap();
        spool.push(&fat_values(4000..4002)).unwrap();
        assert_eq!(segment_files(dir.path()).len(), 2);
        let got = spool.peek(4002).unwrap();
        assert_eq!(got.len(), 4002);
        assert_eq!(got[3999], fat_value(3999));
        assert_eq!(got[4000], fat_value(4000));
        spool.ack(4001).unwrap();
        assert_eq!(spool.peek(10).unwrap(), vec![fat_value(4001)]);
    }

    #[test]
    fn corrupt_cursor_starts_at_oldest_segment() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut spool = Spool::open(dir.path(), 10 * SEGMENT_BYTES).unwrap();
            spool.push(&values(0..3)).unwrap();
            spool.peek(2).unwrap();
            spool.ack(2).unwrap();
        }
        fs::write(dir.path().join("cursor"), b"\xff garbage").unwrap();
        let mut spool = Spool::open(dir.path(), 10 * SEGMENT_BYTES).unwrap();
        assert_eq!(spool.peek(10).unwrap(), values(0..3));
    }

    #[test]
    fn cursor_pointing_at_a_missing_segment_starts_at_oldest() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut spool = Spool::open(dir.path(), 10 * SEGMENT_BYTES).unwrap();
            spool.push(&values(0..3)).unwrap();
        }
        fs::write(dir.path().join("cursor"), b"7 2\n").unwrap();
        let mut spool = Spool::open(dir.path(), 10 * SEGMENT_BYTES).unwrap();
        assert_eq!(spool.peek(10).unwrap(), values(0..3));
    }

    #[test]
    fn unrelated_files_are_ignored_and_stale_temp_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("notes.txt"), b"hello").unwrap();
        fs::write(dir.path().join("1.jsonl"), b"junk\n").unwrap();
        fs::write(dir.path().join("cursor.tmp"), b"1 1\n").unwrap();
        let mut spool = Spool::open(dir.path(), 10 * SEGMENT_BYTES).unwrap();
        assert!(spool.is_empty());
        assert_eq!(spool.size_bytes(), 0);
        spool.push(&values(0..1)).unwrap();
        assert_eq!(spool.peek(10).unwrap(), values(0..1));
        assert!(!dir.path().join("cursor.tmp").exists());
        assert!(dir.path().join("notes.txt").exists());
    }

    #[test]
    fn creates_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b");
        let mut spool = Spool::open(&nested, SEGMENT_BYTES).unwrap();
        spool.push(&values(0..1)).unwrap();
        assert_eq!(spool.peek(1).unwrap(), values(0..1));
    }

    #[test]
    fn dropping_segments_invalidates_a_pending_peek() {
        let dir = tempfile::tempdir().unwrap();
        let max = 3 * SEGMENT_BYTES;
        let mut spool = Spool::open(dir.path(), max).unwrap();
        spool.push(&fat_values(0..4000)).unwrap(); // segment 1
        spool.push(&fat_values(4000..8000)).unwrap(); // segment 2
        let peeked = spool.peek(10).unwrap();
        assert_eq!(peeked[0], fat_value(0));
        spool.push(&fat_values(8000..12000)).unwrap(); // segment 3: segment 1 is dropped
        assert!(spool.size_bytes() <= max);
        assert_eq!(spool.ack(10).unwrap_err().kind(), ErrorKind::InvalidInput);
        // The cursor moved to the oldest remaining segment.
        assert_eq!(spool.peek(1).unwrap(), vec![fat_value(4000)]);
        drop(spool);
        let mut spool = Spool::open(dir.path(), max).unwrap();
        assert_eq!(spool.peek(1).unwrap(), vec![fat_value(4000)]);
    }
}
