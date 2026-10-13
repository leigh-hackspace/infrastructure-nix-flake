//! Image thumbnails for the icon grid, and the cache that holds them.
//!
//! Thumbnails are generated on demand and cached in temporary storage
//! (`--thumb-cache`, tmpfs-backed on services1).  The cache key is a hash of the
//! file's *identity* — relative path plus the `fingerprint` in fsutil (`mtime +
//! ctime + size + inode`) — and that is what makes a stale thumbnail impossible:
//! any change to the file changes the key, so the entry a request gets is always
//! the one generated from the file as it is now.  Nothing is ever invalidated;
//! entries for a file state that no longer exists are just dead weight until the
//! size cap prunes them.
//!
//! The same identity is in the URL the SPA builds
//! (`/api/thumb?path=…&v=<fingerprint>`), so the *browser* cache is keyed the same
//! way: a response stored under an old fingerprint can never be shown for a file
//! that has changed.  The server recomputes the key from the current stat whatever
//! `v` says, so a hand-built URL with a stale fingerprint regenerates rather than
//! serving the old entry.
//!
//! A file that cannot be thumbnailed (not decodable, or bigger than
//! `--thumb-max-source`) gets a marker entry holding the message, so a broken image
//! is not re-decoded on every grid render.  The marker is keyed the same way, so it
//! is no more stale than a thumbnail.
//!
//! Only formats `image` can rasterise are thumbnailed (the list is
//! `common_preview::thumbnailable`, shared with the SPA so they cannot disagree).
//! SVG is in the preview table but is XML, not a bitmap, and AVIF needs `ravif`, so
//! those rows keep the emoji icon — the full file is still previewable in a browser.

use std::collections::HashSet;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use image::codecs::jpeg::JpegEncoder;
use image::ImageDecoder;

use crate::fsutil::mtime_of;
use crate::server::fmt_limit;

/// Output quality.  82 is visually lossless at icon size and keeps a thumbnail a
/// few KB, which matters when a folder of photos renders at once.
const JPEG_QUALITY: u8 = 82;

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// What a lookup produced: a cached (or just generated) JPEG, or the reason this
/// file cannot be thumbnailed.  `hit` says whether the entry was already there,
/// which is what the `X-Thumb-Cache` response header reports.
#[derive(Debug)]
pub enum Outcome {
    Image { key: String, path: PathBuf, hit: bool },
    Failed { key: String, path: PathBuf, msg: String, hit: bool },
}

pub struct ThumbCache {
    pub dir: PathBuf,
    pub max_side: u32,
    pub max_source: u64,
    pub max_bytes: u64,
    /// Set when the cache directory cannot be created (a dev run pointed at a
    /// directory it has no permission for).  Thumbnails then simply fail and the
    /// UI falls back to the emoji icon.
    pub disabled: bool,
    used: AtomicU64,
    inflight: Mutex<HashSet<String>>,
    seq: AtomicU32,
}

/// One key being generated.  Released when generation finishes, so a second
/// request for the same file state waits instead of decoding it twice.
struct Flight<'a> {
    set: &'a Mutex<HashSet<String>>,
    key: String,
    acquired: bool,
}

impl<'a> Flight<'a> {
    fn new(set: &'a Mutex<HashSet<String>>, key: &str) -> Self {
        let acquired = set.lock().unwrap().insert(key.to_string());
        Self {
            set,
            key: key.to_string(),
            acquired,
        }
    }
}

impl Drop for Flight<'_> {
    fn drop(&mut self) {
        if self.acquired {
            self.set.lock().unwrap().remove(&self.key);
        }
    }
}

impl ThumbCache {
    pub fn open(dir: &Path, max_side: u32, max_source: u64, max_bytes: u64) -> Self {
        let disabled = !dir.is_dir() && fs::create_dir_all(dir).is_err();
        let used = if disabled { 0 } else { total_size(dir) };
        Self {
            dir: dir.to_path_buf(),
            max_side,
            max_source,
            max_bytes,
            disabled,
            used: AtomicU64::new(used),
            inflight: Mutex::new(HashSet::new()),
            seq: AtomicU32::new(0),
        }
    }

    /// The file's identity, hashed.  Two states of the same file never share a key,
    /// which is the whole freshness guarantee: the key is `hash(rel + fingerprint)`,
    /// and the fingerprint the SPA puts in the URL is the same identity.
    pub fn key(&self, rel: &str, meta: &fs::Metadata) -> String {
        format!(
            "{:016x}",
            fnv1a(format!("{rel}|{}", crate::fsutil::fingerprint(meta)).as_bytes())
        )
    }

    /// Sharded by the first two hex digits: a flat directory of a hundred thousand
    /// files is unpleasant, and this keeps each bucket small.
    pub fn path_for(&self, key: &str, ext: &str) -> PathBuf {
        self.dir.join(&key[..2]).join(format!("{key}.{ext}"))
    }

    pub fn entry(&self, abs: &Path, rel: &str, meta: &fs::Metadata) -> Outcome {
        let key = self.key(rel, meta);
        if self.disabled {
            return Outcome::Failed {
                key,
                path: PathBuf::new(),
                msg: "thumbnail cache is disabled".to_string(),
                hit: false,
            };
        }

        let img = self.path_for(&key, "jpg");
        let err = self.path_for(&key, "err");
        if img.exists() {
            return Outcome::Image { key, path: img, hit: true };
        }
        if err.exists() {
            let msg = fs::read_to_string(&err).unwrap_or_default();
            return Outcome::Failed { key, path: err, msg, hit: true };
        }

        let flight = Flight::new(&self.inflight, &key);
        if !flight.acquired {
            // Another request is decoding exactly this file state; give it a
            // moment instead of doing the work twice.  If it does not finish,
            // fall through and do it here.
            for _ in 0..40 {
                if img.exists() {
                    return Outcome::Image {
                        key: key.clone(),
                        path: img.clone(),
                        hit: true,
                    };
                }
                if err.exists() {
                    return Outcome::Failed {
                        key: key.clone(),
                        path: err.clone(),
                        msg: fs::read_to_string(&err).unwrap_or_default(),
                        hit: true,
                    };
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }

        match self.generate(abs, meta) {
            Ok(bytes) => {
                if let Err(e) = self.write(&img, &bytes) {
                    return Outcome::Failed { key, path: img, msg: e, hit: false };
                }
                Outcome::Image { key, path: img, hit: false }
            }
            Err(msg) => {
                // The failure is cached under the same key, so a broken file is
                // not re-decoded on every render.
                let _ = self.write(&err, msg.as_bytes());
                Outcome::Failed { key, path: err, msg, hit: false }
            }
        }
    }

    /// Decode, resize and encode one file.
    fn generate(&self, abs: &Path, meta: &fs::Metadata) -> Result<Vec<u8>, String> {
        if meta.len() > self.max_source {
            return Err(format!(
                "image is larger than the {} thumbnail limit",
                fmt_limit(self.max_source)
            ));
        }
        let bytes = fs::read(abs).map_err(|e| e.to_string())?;
        // Format from the content, not the extension: a JPEG named .png is still a
        // JPEG, and the grid promises a thumbnail for the extension.
        let reader = image::ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|e| e.to_string())?;
        let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
        // `decode` does not apply EXIF orientation, and phone photos are otherwise
        // sideways.
        let orient = decoder.orientation().map_err(|e| e.to_string())?;
        let mut img = image::DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
        img.apply_orientation(orient);
        if img.width() > self.max_side || img.height() > self.max_side {
            img = img.thumbnail(self.max_side, self.max_side);
        }

        // JPEG has no alpha, so composite onto white — the grid's background.
        let (w, h) = (img.width(), img.height());
        let rgba = img.to_rgba8();
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        for p in rgba.pixels() {
            let a = p[3] as u32;
            for c in 0..3 {
                rgb.push(((p[c] as u32 * a + 255 * (255 - a)) / 255) as u8);
            }
        }
        let mut out = Vec::new();
        JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
            .encode(&rgb, w, h, image::ExtendedColorType::Rgb8)
            .map_err(|e| e.to_string())?;
        Ok(out)
    }

    /// Write through a temp name and rename, so a reader never sees a half-written
    /// entry.
    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), String> {
        let dir = path.parent().ok_or("invalid cache path")?;
        let name = path.file_name().ok_or("invalid cache path")?.to_string_lossy();
        // Make room before writing, so the entry just generated is never the one
        // pruned.  A single entry larger than the cap still gets written: the cap
        // bounds the cache, it cannot shrink a thumbnail.
        self.prune(bytes.len() as u64);
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let tmp = dir.join(format!(
            ".{name}-{}-{}",
            std::process::id(),
            self.seq.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
        fs::rename(&tmp, path).map_err(|e| e.to_string())?;
        self.used.fetch_add(bytes.len() as u64, Ordering::Relaxed);
        Ok(())
    }

    /// Evict the oldest entries until `room` more bytes fit.  Entries are never
    /// invalidated — they are only ever dead weight, and this is what bounds the
    /// temporary storage.
    fn prune(&self, room: u64) {
        if self.used.load(Ordering::Relaxed) + room <= self.max_bytes {
            return;
        }
        let mut files: Vec<(PathBuf, i64, u64)> = Vec::new();
        let mut stack = vec![self.dir.clone()];
        while let Some(d) = stack.pop() {
            if let Ok(rd) = fs::read_dir(&d) {
                for e in rd.flatten() {
                    let p = e.path();
                    if let Ok(m) = e.metadata() {
                        if m.is_dir() {
                            stack.push(p);
                        } else {
                            files.push((p, mtime_of(&m), m.len()));
                        }
                    }
                }
            }
        }
        files.sort_by_key(|(_, m, _)| *m);
        for (p, _, size) in files {
            if self.used.load(Ordering::Relaxed) + room <= self.max_bytes {
                break;
            }
            if fs::remove_file(&p).is_ok() {
                self.used.fetch_sub(size, Ordering::Relaxed);
            }
        }
    }

    pub fn used(&self) -> u64 {
        self.used.load(Ordering::Relaxed)
    }
}

fn total_size(dir: &Path) -> u64 {
    let mut n = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(rd) = fs::read_dir(&d) {
            for e in rd.flatten() {
                if let Ok(m) = e.metadata() {
                    if m.is_dir() {
                        stack.push(e.path());
                    } else {
                        n += m.len();
                    }
                }
            }
        }
    }
    n
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    use image::ImageEncoder;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn scratch(name: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "filestore-thumb-{}-{}-{}-{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            n
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A real decodable PNG of `w`x`h`, made with the same encoder the cache uses.
    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::ImageBuffer::from_pixel(w, h, image::Rgb([10, 128, 240]));
        let mut out = Vec::new();
        image::codecs::png::PngEncoder::new(&mut out)
            .write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgb8)
            .unwrap();
        out
    }

    #[test]
    fn key_is_the_file_identity_and_changes_with_any_change_to_it() {
        let c = ThumbCache::open(&scratch("cache"), 120, 1 << 20, 1 << 20);
        let store = scratch("store");
        fs::write(store.join("a.png"), b"hello").unwrap();
        let meta = fs::metadata(store.join("a.png")).unwrap();

        assert_eq!(c.key("a.png", &meta), c.key("a.png", &meta), "same state, same key");

        // Same name, different state.
        fs::write(store.join("a.png"), b"hello!").unwrap();
        let changed = fs::metadata(store.join("a.png")).unwrap();
        assert_ne!(c.key("a.png", &meta), c.key("a.png", &changed), "size moves the key");

        // Same size, different mtime/ctime/inode.
        fs::write(store.join("b.png"), b"hello").unwrap();
        let b = fs::metadata(store.join("b.png")).unwrap();
        assert_ne!(c.key("a.png", &meta), c.key("b.png", &b), "a different file, a different key");

        // The key is content identity, not per-run state: a second cache hashes
        // the same file to the same key.
        let other = ThumbCache::open(&scratch("cache2"), 120, 1 << 20, 1 << 20);
        assert_eq!(c.key("a.png", &meta), other.key("a.png", &meta));
    }

    #[test]
    fn a_thumbnail_is_generated_once_and_then_served_from_the_cache() {
        let c = ThumbCache::open(&scratch("cache"), 120, 1 << 20, 1 << 20);
        let store = scratch("store");
        fs::write(store.join("photo.png"), png(300, 200)).unwrap();
        let abs = store.join("photo.png");
        let meta = fs::metadata(&abs).unwrap();

        let first = c.entry(&abs, "photo.png", &meta);
        assert!(matches!(first, Outcome::Image { hit: false, .. }), "the first lookup generates");
        let path = match &first {
            Outcome::Image { path, .. } => path.clone(),
            Outcome::Failed { msg, .. } => panic!("generation failed: {msg}"),
        };
        assert!(path.exists(), "the entry is on disk");

        // It is actually resized, not just copied.
        let served = image::open(&path).unwrap();
        assert_eq!((served.width(), served.height()), (120, 80));

        let second = c.entry(&abs, "photo.png", &meta);
        assert!(matches!(second, Outcome::Image { hit: true, .. }), "the second is a hit");
    }

    /// The staleness guarantee: change the file and the key moves, so what is
    /// served is generated from the file as it is now.  There is nothing to
    /// invalidate.
    #[test]
    fn a_changed_file_can_never_be_served_the_old_thumbnail() {
        let c = ThumbCache::open(&scratch("cache"), 120, 1 << 20, 1 << 20);
        let store = scratch("store");
        fs::write(store.join("x.png"), png(50, 50)).unwrap();
        let abs = store.join("x.png");

        let before = c.entry(&abs, "x.png", &fs::metadata(&abs).unwrap());
        let old = match &before {
            Outcome::Image { path, .. } => path.clone(),
            Outcome::Failed { msg, .. } => panic!("{msg}"),
        };

        fs::write(store.join("x.png"), png(60, 60)).unwrap();
        let after = c.entry(&abs, "x.png", &fs::metadata(&abs).unwrap());
        let new = match &after {
            Outcome::Image { path, .. } => path.clone(),
            Outcome::Failed { msg, .. } => panic!("{msg}"),
        };
        assert_ne!(old, new, "a changed file gets a different entry");
        let served = image::open(&new).unwrap();
        assert_eq!((served.width(), served.height()), (60, 60), "the served thumbnail is the new file");
    }

    #[test]
    fn a_broken_image_is_cached_as_a_failure_and_not_redecoded() {
        let c = ThumbCache::open(&scratch("cache"), 120, 1 << 20, 1 << 20);
        let store = scratch("store");
        fs::write(store.join("broken.png"), b"not an image at all").unwrap();
        let abs = store.join("broken.png");
        let meta = fs::metadata(&abs).unwrap();

        let first = c.entry(&abs, "broken.png", &meta);
        let marker = match &first {
            Outcome::Failed { path, .. } => path.clone(),
            Outcome::Image { .. } => panic!("garbage decoded as an image"),
        };
        assert!(marker.exists(), "the failure is cached");

        let second = c.entry(&abs, "broken.png", &meta);
        assert!(matches!(second, Outcome::Failed { hit: true, .. }), "the cached failure is reused");
    }

    #[test]
    fn the_cache_stays_under_its_cap() {
        // Cap at about three thumbnails, then generate five.
        let c = ThumbCache::open(&scratch("cache"), 120, 1 << 20, 3000);
        let store = scratch("store");
        for i in 0..5 {
            fs::write(store.join(format!("p{i}.png")), png(200, 200)).unwrap();
            let meta = fs::metadata(store.join(format!("p{i}.png"))).unwrap();
            let out = c.entry(&store.join(format!("p{i}.png")), &format!("p{i}.png"), &meta);
            assert!(matches!(out, Outcome::Image { .. }), "{i}: {out:?}");
        }
        assert!(c.used() <= c.max_bytes, "used {} over cap {}", c.used(), c.max_bytes);
        assert!(c.used() > 0, "nothing was cached at all");
    }

    #[test]
    fn a_source_larger_than_the_limit_is_refused() {
        let c = ThumbCache::open(&scratch("cache"), 120, 100, 1 << 20);
        let store = scratch("store");
        fs::write(store.join("big.png"), vec![0u8; 101]).unwrap();
        let meta = fs::metadata(store.join("big.png")).unwrap();
        match c.entry(&store.join("big.png"), "big.png", &meta) {
            Outcome::Failed { msg, .. } => assert!(msg.contains("limit"), "{msg}"),
            Outcome::Image { .. } => panic!("an oversized file was thumbnailed"),
        }
    }
}
