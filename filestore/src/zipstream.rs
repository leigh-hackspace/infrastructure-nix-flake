//! Streaming ZIP: compresses a file or directory tree on a background
//! thread while the response body is already on its way to the client.
//!
//! The writer uses the ZIP *data descriptor* form (general-purpose bit 3):
//! per-file sizes/CRC go after the compressed data instead of in the local
//! header, so the archive can be produced on a writer that cannot Seek —
//! which a network response body is.  The central directory is accumulated
//! in memory and written once at the end.  Entries (or offsets) above the
//! 4 GiB limit automatically switch to the ZIP64 forms.
//!
//! A `Sink` bridges synchronous `std::io::Write` (what flate2 and std::fs
//! speak) to an mpsc channel; the axum handler turns the receiver side
//! into a `Stream`.  Backpressure is natural: the channel is bounded and
//! `blocking_send` parks the compression thread until the client drains
//! some chunks.

use std::future::Future;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use crc32fast::Hasher;
use flate2::Compression;
use futures_util::Stream;
use tokio::sync::mpsc;

const BUF: usize = 1 << 16;
const U32MAX: u32 = u32::MAX;

struct Sink {
    tx: mpsc::Sender<Result<Bytes, String>>,
}

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.tx
            .blocking_send(Ok(Bytes::copy_from_slice(buf)))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "client went away"))?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Tracks the total bytes written so the ZIP writer can record entry
/// offsets in the central directory.
struct Tracked<'a, W: Write> {
    w: &'a mut W,
    off: &'a mut u64,
}

impl<W: Write> Write for Tracked<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.w.write(buf)?;
        *self.off += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.w.flush()
    }
}

/// A Stream over the mpsc receiver.
pub(crate) struct ZipStream {
    rx: mpsc::Receiver<Result<Bytes, String>>,
}

impl Stream for ZipStream {
    type Item = Result<Bytes, String>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut fut = Box::pin(self.rx.recv());
        Pin::as_mut(&mut fut).poll(cx)
    }
}

/// Build the stream for one or more targets (files and/or directories,
/// inside the store).  Each target becomes its own top-level entry, so
/// zipping a directory `photos` yields `photos/...` entries.  Top-level
/// name collisions get " (n)" suffixes.
pub fn zip(targets: &[PathBuf]) -> ZipStream {
    let targets = targets.to_vec();
    let (tx, rx) = mpsc::channel::<Result<Bytes, String>>(16);
    std::thread::spawn(move || {
        let mut sink = Sink { tx };
        let mut zw = ZipWriter::new(&mut sink);
        let mut used: Vec<String> = Vec::new();
        for t in &targets {
            let base = t
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "item".to_string());
            let name = unique_name(&base, &mut used);
            if let Err(e) = zw.add_root(t, &name) {
                eprintln!("zip stream error ({name}): {e}");
            }
        }
        if let Err(e) = zw.finish() {
            eprintln!("zip stream error (finish): {e}");
        }
        // Dropping `zw` drops `sink`, which drops the sender and ends the
        // stream.
    });
    ZipStream { rx }
}

fn unique_name(base: &str, used: &mut Vec<String>) -> String {
    if !used.iter().any(|u| u == base) {
        used.push(base.to_string());
        return base.to_string();
    }
    let mut n = 1;
    loop {
        let candidate = format!("{base} ({n})");
        if !used.iter().any(|u| u == &candidate) {
            used.push(candidate.clone());
            return candidate;
        }
        n += 1;
    }
}

// ---------------------------------------------------------------------------
// the writer

#[derive(Clone)]
struct CDEntry {
    name: Vec<u8>,
    crc: u32,
    comp_size: u64,
    uncomp_size: u64,
    mod_time: u16,
    mod_date: u16,
    offset: u64,
    external_attr: u32,
    method: u16, // 0 = stored, 8 = deflate
    /// ZIP64 data descriptor was used (sizes in the descriptor are 8-byte)
    zip64: bool,
}

struct ZipWriter<'a, W: Write> {
    w: &'a mut W,
    /// bytes written through `w` so far
    offset: u64,
    entries: Vec<CDEntry>,
}

impl<'a, W: Write> ZipWriter<'a, W> {
    fn new(w: &'a mut W) -> Self {
        Self {
            w,
            offset: 0,
            entries: Vec::new(),
        }
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.w.write_all(buf)?;
        self.offset += buf.len() as u64;
        Ok(())
    }

    fn u16(&mut self, v: u16) -> io::Result<()> {
        self.write_all(&v.to_le_bytes())
    }
    fn u32(&mut self, v: u32) -> io::Result<()> {
        self.write_all(&v.to_le_bytes())
    }
    fn u64(&mut self, v: u64) -> io::Result<()> {
        self.write_all(&v.to_le_bytes())
    }

    /// Add one root (a file or directory) to the archive under `name`
    /// (not the full path).
    pub fn add_root(&mut self, target: &Path, name: &str) -> io::Result<()> {
        let name = name.to_string();
        if target.is_dir() {
            let mut files: Vec<PathBuf> = Vec::new();
            collect(target, &mut files)?;

            let mtime = target.metadata()?.modified().ok().unwrap_or(UNIX_EPOCH);
            self.add_dir(format!("{name}/"), mtime)?;
            for p in &files {
                let rel = p
                    .strip_prefix(target)
                    .map(|r| r.to_string_lossy().to_string())
                    .unwrap_or_default();
                if p.is_dir() {
                    let mtime = p.metadata()?.modified().ok().unwrap_or(UNIX_EPOCH);
                    self.add_dir(format!("{name}/{rel}/"), mtime)?;
                } else {
                    let mtime = p.metadata()?.modified().ok().unwrap_or(UNIX_EPOCH);
                    self.add_file(format!("{name}/{rel}"), p, mtime, 0o644)?;
                }
            }
        } else {
            let mtime = target.metadata()?.modified().ok().unwrap_or(UNIX_EPOCH);
            self.add_file(name, target, mtime, 0o644)?;
        }
        Ok(())
    }

    /// A directory entry (stored, empty); the trailing slash marks it.
    fn add_dir(&mut self, name: String, mtime: SystemTime) -> io::Result<()> {
        let (mod_time, mod_date) = dos_datetime(mtime);
        let name_b = name.into_bytes();
        let offset = self.offset;
        let zip64 = offset > U32MAX as u64;
        let version = if zip64 { 45 } else { 20 };

        // local file header
        self.u32(0x04034b50)?;
        self.u16(version as u16)?;
        self.u16(0x0008)?; // bit 3: data descriptor follows
        self.u16(0)?; // method: stored
        self.u16(mod_time)?;
        self.u16(mod_date)?;
        self.u32(0)?; // crc (in descriptor)
        self.u32(0)?; // compressed size (in descriptor)
        self.u32(0)?; // uncompressed size (in descriptor)
        self.u16(name_b.len() as u16)?;
        self.u16(0)?; // extra
        self.write_all(&name_b)?;
        // data descriptor
        self.u32(0x08074b50)?;
        self.u32(0)?;
        if zip64 {
            self.u64(0)?;
            self.u64(0)?;
        } else {
            self.u32(0)?;
            self.u32(0)?;
        }

        self.entries.push(CDEntry {
            name: name_b,
            crc: 0,
            comp_size: 0,
            uncomp_size: 0,
            mod_time,
            mod_date,
            offset,
            external_attr: 0o040755u32 << 16, // Unix mode: directory
            method: 0,
            zip64,
        });
        Ok(())
    }

    /// A file entry, deflate-compressed from `path`, streamed in chunks.
    fn add_file(
        &mut self,
        name: String,
        path: &Path,
        mtime: SystemTime,
        unix_mode: u32,
    ) -> io::Result<()> {
        let (mod_time, mod_date) = dos_datetime(mtime);
        let name_b = name.into_bytes();
        let offset = self.offset;
        // Decide ZIP64 up front so the local header is spec-consistent:
        // offset above 4 GiB, or file length near the 4 GiB limit (margin
        // for deflate overhead).
        let len = path.metadata()?.len();
        let zip64 = offset > U32MAX as u64 || len > U32MAX as u64 - 1_048_576;
        let version = if zip64 { 45 } else { 20 };

        // local file header (sizes/CRC unknown until streamed)
        self.u32(0x04034b50)?;
        self.u16(version as u16)?;
        self.u16(0x0008)?;
        self.u16(8)?; // method: deflate
        self.u16(mod_time)?;
        self.u16(mod_date)?;
        self.u32(0)?; // crc (in descriptor)
        self.u32(if zip64 { U32MAX } else { 0 })?; // compressed size
        self.u32(if zip64 { U32MAX } else { 0 })?; // uncompressed size
        self.u16(name_b.len() as u16)?;
        self.u16(0)?; // extra
        self.write_all(&name_b)?;
        let data_start = self.offset;

        // compressed data
        let mut f = std::fs::File::open(path)?;
        let mut hasher = Hasher::new();
        // `tracked` wraps self.w so the deflate output is counted toward
        // self.offset; it borrows self.w/self.offset until dropped below.
        let tracked = Tracked {
            w: &mut *self.w,
            off: &mut self.offset,
        };
        let mut enc = flate2::write::DeflateEncoder::new(tracked, Compression::fast());
        let mut inb = vec![0u8; BUF];
        let mut uncomp_size: u64 = 0;
        loop {
            let n = f.read(&mut inb)?;
            if n == 0 {
                break;
            }
            hasher.update(&inb[..n]);
            enc.write_all(&inb[..n])?;
            uncomp_size += n as u64;
        }
        // finish() flushes the deflate stream and returns the wrapped
        // writer (our `Tracked`); discarding it ends the borrow of self.
        enc.finish()?;
        let comp_size = self.offset - data_start;
        let crc = hasher.finalize();
        let zip64 = zip64 || comp_size > U32MAX as u64;

        // data descriptor
        self.u32(0x08074b50)?;
        self.u32(crc)?;
        if zip64 {
            self.u64(comp_size)?;
            self.u64(uncomp_size)?;
        } else {
            self.u32(comp_size as u32)?;
            self.u32(uncomp_size as u32)?;
        }

        self.entries.push(CDEntry {
            name: name_b,
            crc,
            comp_size,
            uncomp_size,
            mod_time,
            mod_date,
            offset,
            external_attr: unix_mode << 16,
            method: 8,
            zip64,
        });
        Ok(())
    }

    /// Central directory + end-of-central-directory record.
    fn finish(&mut self) -> io::Result<()> {
        let cd_start = self.offset;
        let entries = std::mem::take(&mut self.entries);
        let any_zip64 = entries.iter().any(|e| {
            e.zip64
                || e.comp_size > U32MAX as u64
                || e.uncomp_size > U32MAX as u64
                || e.offset > U32MAX as u64
        }) || entries.len() >= U32MAX as usize;

        for e in &entries {
            let z64 = e.zip64
                || e.comp_size > U32MAX as u64
                || e.uncomp_size > U32MAX as u64
                || e.offset > U32MAX as u64;

            self.u32(0x02014b50)?;
            self.u16(if z64 { 45 } else { 20 } as u16)?; // version made by
            self.u16(if z64 { 45 } else { 20 } as u16)?; // version needed
            self.u16(0x0008)?;
            self.u16(e.method)?;
            self.u16(e.mod_time)?;
            self.u16(e.mod_date)?;
            self.u32(e.crc)?;
            self.u32(if e.comp_size > U32MAX as u64 {
                U32MAX
            } else {
                e.comp_size as u32
            })?;
            self.u32(if e.uncomp_size > U32MAX as u64 {
                U32MAX
            } else {
                e.uncomp_size as u32
            })?;
            self.u16(e.name.len() as u16)?;
            // extra: ZIP64 extra field when needed.  The declared length
            // covers the subfield header (type 2 + size 2) plus the three
            // 8-byte values: 2 + 2 + 24 = 28.
            let extra_len = if z64 { 4 + 24 } else { 0 };
            self.u16(extra_len as u16)?;
            self.u16(0)?; // comment
            self.u16(0)?; // disk start
            self.u16(0)?; // internal attrs
            self.u32(e.external_attr)?;
            self.u32(if e.offset > U32MAX as u64 {
                U32MAX
            } else {
                e.offset as u32
            })?;
            self.write_all(&e.name)?;
            if z64 {
                self.u16(0x0001)?; // ZIP64 extra
                self.u16(24)?;
                self.u64(e.uncomp_size)?;
                self.u64(e.comp_size)?;
                self.u64(e.offset)?;
            }
        }
        let cd_end = self.offset;

        if any_zip64 {
            // zip64 end-of-central-directory record + locator
            let z64_start = self.offset;
            self.u32(0x06064b50)?;
            self.u64(44)?; // size of the rest of this record
            self.u16(45)?;
            self.u16(45)?;
            self.u32(0)?; // disk
            self.u32(0)?; // CD disk
            self.u64(entries.len() as u64)?;
            self.u64(entries.len() as u64)?;
            self.u64(cd_end - cd_start)?;
            self.u64(cd_start)?;
            self.u32(0x07064b50)?; // locator
            self.u32(0)?; // disk number holding the zip64 EOCD (0 = single disk)
            self.u64(z64_start)?;
            self.u32(1)?; // total number of disks
        }

        self.u32(0x06054b50)?; // EOCD
        self.u16(0)?;
        self.u16(0)?;
        self.u16(if entries.len() > U32MAX as usize {
            U32MAX as u16
        } else {
            entries.len() as u16
        })?;
        self.u16(if entries.len() > U32MAX as usize {
            U32MAX as u16
        } else {
            entries.len() as u16
        })?;
        self.u32(if cd_end - cd_start > U32MAX as u64 {
            U32MAX
        } else {
            (cd_end - cd_start) as u32
        })?;
        self.u32(if cd_start > U32MAX as u64 {
            U32MAX
        } else {
            cd_start as u32
        })?;
        self.u16(0)?; // comment length
        self.w.flush()
    }
}

/// Dirs (before their contents) and files under `dir`, sorted by name so
/// the zip is deterministic.
fn collect(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    let rd = std::fs::read_dir(dir)?;
    let mut items: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    items.sort();
    for p in items {
        if p.is_dir() {
            out.push(p.clone());
            collect(&p, out)?;
        } else {
            out.push(p);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// DOS time/date (valid range 1980-2107)

fn dos_datetime(t: SystemTime) -> (u16, u16) {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
        .max(0);
    let days = secs / 86400;
    let tod = secs % 86400;
    let time = (((tod / 3600) as u16) << 11)
        | (((tod % 3600) / 60) as u16) << 5
        | (((tod % 60) / 2) as u16);

    let (year, month, day) = civil_from_days(days);
    if year < 1980 {
        return (time, 0); // pre-DOS-epoch; readers will show 1980-00-00
    }
    let year = (year - 1980).clamp(0, 127) as u16;
    let date = (year << 9) | (month.min(12) as u16) << 5 | day.min(31) as u16;
    (time, date)
}

/// Convert days since 1970-01-01 to (year, month 1-12, day).
/// Howard Hinnant's civil_from_days algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // day of era [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // year of era
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day of year [0, 365]
    let mp = (5 * doy + 2) / 153; // month pointer [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // day [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // month [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}
