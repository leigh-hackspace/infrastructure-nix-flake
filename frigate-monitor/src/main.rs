//! frigate-monitor — persistent scene-change detector for an RTSP stream.
//!
//! Every `--interval` seconds a JPEG snapshot is grabbed from an RTSP stream
//! (via ffmpeg) and compared against a slowly adapting background model. A
//! change is recorded as an *event* only once the same regions keep
//! differing from the background for `--persist` consecutive snapshots.
//!
//! Why that shape: a human walking through the frame produces differences
//! that appear, move and disappear, so no region stays "differing" long
//! enough to trip the persistence threshold. A pair of scissors left on a
//! table, a scissors taken off it, or a chair moved to a new spot produces a
//! difference that stays put, and is recorded as an event shortly after.
//!
//! The background is an exponential moving average (small alpha) that only
//! updates at pixels that currently match it, so:
//!   - objects present at startup are absorbed into the background over a
//!     few minutes;
//!   - while an object sits still, the background does not update underneath
//!     it, so removing it later still produces a persistent diff;
//!   - a whole-scene change (lights on/off, camera reset) is detected by the
//!     global-difference guard and re-seeds the background instead of
//!     logging a garbage event.
//!
//! Each event stores, under `<data-dir>/events/<id>/`:
//!   thumb.jpg    small "after" thumbnail (with outlines)
//!   before.jpg   full-resolution "before" with changed regions outlined
//!   after.jpg    full-resolution "after"  with changed regions outlined
//!   before_z.jpg zoomed crop of the largest changed region (before)
//!   after_z.jpg  same crop (after)
//!   meta.json    id, timestamp, region boxes (full-resolution pixels)
//!
//! Web UI (single page, no build step):
//!   GET  /                  SPA
//!   GET  /api/status        {frames, last_frame_ts, scene_resets, events}
//!   GET  /api/events        [meta, meta, ...] newest first
//!   GET  /api/events/<id>   one meta
//!   GET  /files/<id>/<f>    one of the event JPEGs
//!   GET  /api/live          latest raw snapshot

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_BIND: &str = "0.0.0.0";
const DEFAULT_PORT: &str = "8090";
const DEFAULT_RTSP: &str = "rtsp://10.3.1.20:8554/main_space";
const DEFAULT_FFMPEG: &str = "ffmpeg";
const DEFAULT_INTERVAL: f64 = 10.0;
const DEFAULT_WIDTH: u32 = 640; // detection resolution (width)

const DIFF_THRESHOLD: f32 = 32.0; // max-channel |frame - bg| that counts as "changed"
const BG_ALPHA: f32 = 0.02; // background adaptation rate (per snapshot)
const BLOCK: u32 = 16; // block size at detection resolution
const BLOCK_DIFF_FRAC: f32 = 0.25; // fraction of a block that must differ
const GLOBAL_RESET_FRAC: f32 = 0.30; // > this fraction differing => re-seed background
const ACK_CLEAR_FRAMES: u32 = 10; // clean frames before an acked block can re-trigger
const MIN_AREA_FRAC: f64 = 1.0 / 2500.0; // min full-res region area (relative)
const OUTLINE_STAMP: i32 = 3; // outline = 7px thick at full resolution
const THUMB_WIDTH: u32 = 320;
const RING_MAX: usize = 30; // snapshots kept for "before" images (~5 min at 10 s)
const FFMPEG_TIMEOUT: Duration = Duration::from_secs(20);

const EVENT_FILES: [&str; 5] = ["thumb.jpg", "before.jpg", "after.jpg", "before_z.jpg", "after_z.jpg"];

struct Config {
    bind: String,
    port: u16,
    rtsp: String,
    ffmpeg: String,
    interval: f64,
    width: u32,
    persist: u32,
    data_dir: PathBuf,
}

fn parse_args() -> Config {
    let mut cfg = Config {
        bind: DEFAULT_BIND.to_string(),
        port: DEFAULT_PORT.parse().unwrap(),
        rtsp: DEFAULT_RTSP.to_string(),
        ffmpeg: DEFAULT_FFMPEG.to_string(),
        interval: DEFAULT_INTERVAL,
        width: DEFAULT_WIDTH,
        persist: 4,
        data_dir: PathBuf::from("/var/lib/frigate-monitor"),
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut next = || args.next().expect("missing value for flag");
        match a.as_str() {
            "--bind" => cfg.bind = next(),
            "--port" => cfg.port = next().parse().expect("bad port"),
            "--rtsp" => cfg.rtsp = next(),
            "--ffmpeg" => cfg.ffmpeg = next(),
            "--interval" => cfg.interval = next().parse().expect("bad interval"),
            "--width" => cfg.width = next().parse().expect("bad width"),
            "--persist" => cfg.persist = next().parse().expect("bad persist"),
            "--data-dir" => cfg.data_dir = PathBuf::from(next()),
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }
    cfg
}

/// Unix seconds -> "YYYY-MM-DDTHH:MM:SS" (UTC; no chrono, civil-from-days).
fn fmt_ts(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (h, m, s) = (tod / 3600, (tod % 3600) / 60, tod % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    // The civil calendar year starts in March; Jan/Feb belong to y + 1.
    let y = y + if mo <= 2 { 1 } else { 0 };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}")
}

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Grab one JPEG frame from the RTSP stream with a hard timeout.
fn grab_frame(ffmpeg: &str, rtsp: &str, out: &Path) -> Result<(), String> {
    let _ = std::fs::remove_file(out);
    let mut child = Command::new(ffmpeg)
        .args([
            "-hide_banner", "-loglevel", "error",
            "-rtsp_transport", "tcp",
            "-i", rtsp,
            "-frames:v", "1", "-q:v", "3",
            "-y",
        ])
        .arg(out)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn ffmpeg: {e}"))?;
    let deadline = Instant::now() + FFMPEG_TIMEOUT;
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => {
                if status.success() && out.exists() {
                    return Ok(());
                }
                return Err(format!("ffmpeg exited {status}"));
            }
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("ffmpeg timed out".into());
                }
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

fn decode_rgb(bytes: &[u8]) -> Result<image::RgbImage, String> {
    image::load_from_memory(bytes)
        .map_err(|e| format!("decode: {e}"))
        .map(|d| d.to_rgb8())
}

fn encode_jpeg(img: &image::RgbImage, quality: u8) -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::new();
    {
        use image::codecs::jpeg::JpegEncoder;
        use image::ExtendedColorType;
        let mut enc = JpegEncoder::new_with_quality(&mut buf, quality);
        enc.encode(img.as_raw(), img.width(), img.height(), ExtendedColorType::Rgb8)
            .expect("jpeg encode");
    }
    buf
}

/// Per-block detection state. The background model plus a grid of blocks
/// each carrying a "consecutive differing frames" counter and an ack flag.
struct Detector {
    w: u32,
    h: u32,
    bg: Vec<f32>, // w*h*3
    have_bg: bool,
    bw: u32,
    bh: u32,
    diff_count: Vec<u32>, // per block
    acked: Vec<bool>,
    clean_count: Vec<u32>,
    full_w: u32,
    full_h: u32,
    persist: u32,
}

impl Detector {
    fn new(width: u32, persist: u32) -> Self {
        Detector {
            w: width,
            h: 0,
            bg: Vec::new(),
            have_bg: false,
            bw: 0,
            bh: 0,
            diff_count: Vec::new(),
            acked: Vec::new(),
            clean_count: Vec::new(),
            full_w: 0,
            full_h: 0,
            persist,
        }
    }

    /// Seed the background from the first frame.
    fn seed(&mut self, full: &image::RgbImage) {
        let small = downscale(full, self.w);
        self.h = small.height();
        self.full_w = full.width();
        self.full_h = full.height();
        self.bg = small.pixels().flat_map(|p| [p[0] as f32, p[1] as f32, p[2] as f32]).collect();
        self.bw = self.w.div_ceil(BLOCK);
        self.bh = self.h.div_ceil(BLOCK);
        let n = (self.bw * self.bh) as usize;
        self.diff_count = vec![0; n];
        self.acked = vec![false; n];
        self.clean_count = vec![0; n];
        self.have_bg = true;
    }

    /// Compare a frame against the background; update the model and block
    /// counters. Returns true when a whole-scene reset happened (background
    /// re-seeded) and, otherwise, the list of blocks that just crossed the
    /// persistence threshold.
    fn step(&mut self, full: &image::RgbImage) -> (bool, Vec<usize>) {
        if !self.have_bg || full.width() != self.full_w || full.height() != self.full_h {
            self.seed(full);
            return (true, Vec::new());
        }
        let small = downscale(full, self.w);
        let (w, h) = (self.w, self.h);
        let n = (w * h) as usize;

        // Per-pixel mask: any channel differs from the background by more
        // than the threshold.
        let mut mask = vec![false; n];
        let mut masked = 0usize;
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) as usize;
                let f = small.get_pixel(x, y);
                let b = &self.bg[i * 3..i * 3 + 3];
                let d = (f[0] as f32 - b[0]).abs().max((f[1] as f32 - b[1]).abs()).max((f[2] as f32 - b[2]).abs());
                if d > DIFF_THRESHOLD {
                    mask[i] = true;
                    masked += 1;
                }
            }
        }

        // Global guard: a big scene change (lights, camera reset) re-seeds
        // the background instead of producing a garbage event.
        if masked as f32 > GLOBAL_RESET_FRAC * n as f32 {
            self.seed(full);
            return (true, Vec::new());
        }

        // Adapt the background, but only where it currently matches.
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) as usize;
                if mask[i] {
                    continue;
                }
                let f = small.get_pixel(x, y);
                let b = &mut self.bg[i * 3..i * 3 + 3];
                b[0] += (f[0] as f32 - b[0]) * BG_ALPHA;
                b[1] += (f[1] as f32 - b[1]) * BG_ALPHA;
                b[2] += (f[2] as f32 - b[2]) * BG_ALPHA;
            }
        }

        // Block counters.
        let mut triggered = Vec::new();
        for by in 0..self.bh {
            for bx in 0..self.bw {
                let bi = (by * self.bw + bx) as usize;
                let mut diff_px = 0usize;
                let mut total = 0usize;
                for y in by * BLOCK..(by * BLOCK + BLOCK).min(h) {
                    for x in bx * BLOCK..(bx * BLOCK + BLOCK).min(w) {
                        total += 1;
                        if mask[(y * w + x) as usize] {
                            diff_px += 1;
                        }
                    }
                }
                let diffing = total > 0 && diff_px as f32 >= BLOCK_DIFF_FRAC * total as f32;
                if diffing {
                    if !self.acked[bi] {
                        self.diff_count[bi] += 1;
                        if self.diff_count[bi] == self.persist {
                            triggered.push(bi);
                        }
                    }
                    self.clean_count[bi] = 0;
                } else {
                    self.diff_count[bi] = 0;
                    if self.acked[bi] {
                        self.clean_count[bi] += 1;
                        if self.clean_count[bi] >= ACK_CLEAR_FRAMES {
                            self.acked[bi] = false;
                            self.clean_count[bi] = 0;
                        }
                    }
                }
            }
        }
        (false, triggered)
    }
}

fn downscale(full: &image::RgbImage, width: u32) -> image::RgbImage {
    let scale = width as f32 / full.width() as f32;
    let height = (full.height() as f32 * scale).round().max(1.0) as u32;
    image::imageops::resize(full, width, height, image::imageops::FilterType::Triangle)
}

/// Full-resolution difference mask between two same-size images.
fn diff_mask(a: &image::RgbImage, b: &image::RgbImage, threshold: f32) -> Vec<bool> {
    let (w, h) = (a.width(), a.height());
    let n = (w * h) as usize;
    let mut mask = vec![false; n];
    let pa = a.as_raw();
    let pb = b.as_raw();
    for i in 0..n {
        let d = (pa[i * 3] as f32 - pb[i * 3] as f32).abs()
            .max((pa[i * 3 + 1] as f32 - pb[i * 3 + 1] as f32).abs())
            .max((pa[i * 3 + 2] as f32 - pb[i * 3 + 2] as f32).abs());
        if d > threshold {
            mask[i] = true;
        }
    }
    mask
}

struct Region {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    area: u32,
}

/// Connected components (4-connected) of the mask; returns regions sorted by
/// area descending, keeping only those at least `min_area` pixels.
fn find_regions(mask: &[bool], w: u32, h: u32, min_area: u32) -> Vec<Region> {
    let n = (w * h) as usize;
    let mut visited = vec![false; n];
    let mut stack: Vec<usize> = Vec::with_capacity(1024);
    let mut regions = Vec::new();
    for start in 0..n {
        if !mask[start] || visited[start] {
            continue;
        }
        let mut minx = w;
        let mut miny = h;
        let mut maxx = 0u32;
        let mut maxy = 0u32;
        let mut area = 0u32;
        stack.clear();
        stack.push(start);
        visited[start] = true;
        while let Some(i) = stack.pop() {
            let x = (i % w as usize) as u32;
            let y = (i / w as usize) as u32;
            if x < minx { minx = x; }
            if y < miny { miny = y; }
            if x > maxx { maxx = x; }
            if y > maxy { maxy = y; }
            area += 1;
            // Neighbours (4-connected).
            if x > 0 {
                let j = i - 1;
                if mask[j] && !visited[j] { visited[j] = true; stack.push(j); }
            }
            if x + 1 < w {
                let j = i + 1;
                if mask[j] && !visited[j] { visited[j] = true; stack.push(j); }
            }
            if y > 0 {
                let j = i - w as usize;
                if mask[j] && !visited[j] { visited[j] = true; stack.push(j); }
            }
            if y + 1 < h {
                let j = i + w as usize;
                if mask[j] && !visited[j] { visited[j] = true; stack.push(j); }
            }
        }
        if area >= min_area {
            regions.push(Region { x: minx, y: miny, w: maxx - minx + 1, h: maxy - miny + 1, area });
        }
    }
    regions.sort_by(|a, b| b.area.cmp(&a.area));
    regions
}

/// Stamp the outline (boundary pixels of the mask, thickened) in red.
fn draw_outline(img: &mut image::RgbImage, mask: &[bool]) {
    let (w, h) = (img.width(), img.height());
    let red = image::Rgb([255u8, 40, 40]);
    let mut stamp = |x: i32, y: i32| {
        if x < 0 || y < 0 {
            return;
        }
        let (x, y) = (x as u32, y as u32);
        if x >= w || y >= h {
            return;
        }
        for dy in 0..=OUTLINE_STAMP * 2 {
            for dx in 0..=OUTLINE_STAMP * 2 {
                let (nx, ny) = (x as i64 + dx as i64 - OUTLINE_STAMP as i64, y as i64 + dy as i64 - OUTLINE_STAMP as i64);
                if nx >= 0 && ny >= 0 && (nx as u32) < w && (ny as u32) < h {
                    *img.get_pixel_mut(nx as u32, ny as u32) = red;
                }
            }
        }
    };
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            if !mask[i] {
                continue;
            }
            // Boundary: any of the 8 neighbours is outside the mask.
            let mut boundary = false;
            'outer: for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        boundary = true;
                        break 'outer;
                    }
                    if !mask[(ny as u32 * w + nx as u32) as usize] {
                        boundary = true;
                        break 'outer;
                    }
                }
            }
            if boundary {
                stamp(x as i32, y as i32);
            }
        }
    }
}

fn crop(img: &image::RgbImage, r: &Region) -> image::RgbImage {
    let pad_x = (r.w as f64 * 0.35) as u32;
    let pad_y = (r.h as f64 * 0.35) as u32;
    let x0 = r.x.saturating_sub(pad_x);
    let y0 = r.y.saturating_sub(pad_y);
    let x1 = (r.x + r.w + pad_x).min(img.width());
    let y1 = (r.y + r.h + pad_y).min(img.height());
    let cw = (x1 - x0).max(8);
    let ch = (y1 - y0).max(8);
    let mut tmp = img.clone();
    image::imageops::crop_imm(&mut tmp, x0, y0, cw, ch).to_image()
}

/// Record an event: before/after with outlines, zoomed crops, thumbnail,
/// meta. `before_bytes` is the oldest snapshot in the ring.
fn record_event(
    data_dir: &Path,
    before_bytes: &[u8],
    after_full: &image::RgbImage,
    ts: i64,
) -> std::io::Result<Option<String>> {
    let before_img = match decode_rgb(before_bytes) {
        Ok(img) if img.width() == after_full.width() && img.height() == after_full.height() => img,
        // Before image undecodable or size changed (camera restart): fall
        // back to a solid placeholder so the event is still recorded with
        // the "after" image.
        _ => image::RgbImage::from_pixel(after_full.width(), after_full.height(), image::Rgb([80, 80, 80])),
    };

    let min_area = ((after_full.width() as f64 * after_full.height() as f64) * MIN_AREA_FRAC) as u32;
    let mask = diff_mask(&before_img, after_full, DIFF_THRESHOLD);
    let regions = find_regions(&mask, after_full.width(), after_full.height(), min_area);
    if regions.is_empty() {
        return Ok(None); // nothing real to show; caller acks the blocks
    }

    // Unique id from wall clock (seconds).
    let events_dir = data_dir.join("events");
    std::fs::create_dir_all(&events_dir)?;
    let mut id = ts;
    let mut dir = events_dir.join(id.to_string());
    while dir.exists() {
        id += 1;
        dir = events_dir.join(id.to_string());
    }
    std::fs::create_dir_all(&dir)?;

    let mut before_marked = before_img;
    let mut after_marked = after_full.clone();
    draw_outline(&mut before_marked, &mask);
    draw_outline(&mut after_marked, &mask);

    let biggest = &regions[0];
    let before_z = crop(&before_marked, biggest);
    let after_z = crop(&after_marked, biggest);
    let thumb = downscale(&after_marked, THUMB_WIDTH);

    std::fs::write(dir.join("thumb.jpg"), encode_jpeg(&thumb, 85))?;
    std::fs::write(dir.join("before.jpg"), encode_jpeg(&before_marked, 85))?;
    std::fs::write(dir.join("after.jpg"), encode_jpeg(&after_marked, 85))?;
    std::fs::write(dir.join("before_z.jpg"), encode_jpeg(&before_z, 88))?;
    std::fs::write(dir.join("after_z.jpg"), encode_jpeg(&after_z, 88))?;

    let regions_json: Vec<String> = regions
        .iter()
        .map(|r| format!("{{\"x\":{},\"y\":{},\"w\":{},\"h\":{},\"area\":{}}}", r.x, r.y, r.w, r.h, r.area))
        .collect();
    let meta = format!(
        "{{\"id\":{},\"ts\":\"{}\",\"width\":{},\"height\":{},\"regions\":[{}]}}",
        id,
        fmt_ts(ts),
        after_full.width(),
        after_full.height(),
        regions_json.join(",")
    );
    std::fs::write(dir.join("meta.json"), meta)?;
    eprintln!("event {id}: {} region(s), largest {}x{} at ({},{}))",
        regions.len(), biggest.w, biggest.h, biggest.x, biggest.y);
    Ok(Some(id.to_string()))
}

// ---------------------------------------------------------------------------
// Shared state between the capture loop and the HTTP server.
// ---------------------------------------------------------------------------

struct Shared {
    data_dir: PathBuf,
    frames: u64,
    last_frame_ts: i64,
    scene_resets: u64,
    events: u64,
    last_error: String,
    latest: Option<Vec<u8>>,
}

// ---------------------------------------------------------------------------
// Capture loop
// ---------------------------------------------------------------------------

fn run_capture(cfg: Arc<Config>, shared: Arc<RwLock<Shared>>) {
    let mut detector = Detector::new(cfg.width, cfg.persist);
    let mut ring: VecDeque<(i64, Vec<u8>)> = VecDeque::with_capacity(RING_MAX);
    let tmp = cfg.data_dir.join("frame.jpg");
    let _ = std::fs::create_dir_all(&cfg.data_dir);

    loop {
        let t0 = Instant::now();
        let ts = now_secs();
        let res = grab_frame(&cfg.ffmpeg, &cfg.rtsp, &tmp).and_then(|()| {
            std::fs::read(&tmp).map_err(|e| e.to_string())
        });
        match res {
            Ok(jpeg) => match decode_rgb(&jpeg) {
                Ok(full) => {
                    let (reset, triggered) = detector.step(&full);
                    if reset {
                        // New background baseline: drop old snapshots so
                        // "before" images are never from before a scene reset.
                        ring.clear();
                        ring.push_back((ts, jpeg.clone()));
                        shared.write().unwrap().scene_resets += 1;
                    } else if !triggered.is_empty() {
                        let before = ring.front().map(|(_, b)| b.clone());
                        if let Some(before_bytes) = before {
                            match record_event(&cfg.data_dir, &before_bytes, &full, ts) {
                                Ok(Some(id)) => {
                                    shared.write().unwrap().events += 1;
                                    // Ack the triggered blocks so the same
                                    // change is not re-recorded while it persists.
                                    ack_blocks(&mut detector, &triggered);
                                    let _ = id;
                                }
                                Ok(None) => ack_blocks(&mut detector, &triggered),
                                Err(e) => eprintln!("event record failed: {e}"),
                            }
                        }
                    }
                    ring.push_back((ts, jpeg.clone()));
                    while ring.len() > RING_MAX {
                        ring.pop_front();
                    }
                    let mut s = shared.write().unwrap();
                    s.frames += 1;
                    s.last_frame_ts = ts;
                    s.last_error.clear();
                    s.latest = Some(jpeg);
                }
                Err(e) => {
                    shared.write().unwrap().last_error = e;
                }
            },
            Err(e) => {
                eprintln!("snapshot failed: {e}");
                shared.write().unwrap().last_error = e;
            }
        }
        let elapsed = t0.elapsed();
        let target = Duration::from_secs_f64(cfg.interval);
        if elapsed < target {
            thread::sleep(target - elapsed);
        }
    }
}

fn ack_blocks(det: &mut Detector, blocks: &[usize]) {
    for &b in blocks {
        det.acked[b] = true;
        det.diff_count[b] = 0;
        det.clean_count[b] = 0;
    }
}

// ---------------------------------------------------------------------------
// HTTP server (hand-rolled, like status-dashboard)
// ---------------------------------------------------------------------------

fn read_request(stream: &mut TcpStream) -> Option<(String, String)> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 8192 {
            let head = String::from_utf8_lossy(&buf).to_string();
            let first = head.lines().next().unwrap_or("");
            let mut parts = first.split_whitespace();
            let method = parts.next().unwrap_or("").to_string();
            let target = parts.next().unwrap_or("/").to_string();
            return Some((method, target));
        }
    }
}

fn send(stream: &mut TcpStream, status: &str, content_type: &str, body: Vec<u8>) {
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.write_all(&body);
    let _ = stream.flush();
}

fn send_file(stream: &mut TcpStream, path: &Path, content_type: &str) {
    match std::fs::read(path) {
        Ok(b) => send(stream, "200 OK", content_type, b),
        Err(_) => send(stream, "404 Not Found", "text/plain", b"not found".to_vec()),
    }
}

fn list_events(data_dir: &Path) -> Vec<(String, String)> {
    let events_dir = data_dir.join("events");
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&events_dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if let Ok(meta) = std::fs::read_to_string(e.path().join("meta.json")) {
                out.push((name, meta));
            }
        }
    }
    out.sort_by(|a, b| b.0.cmp(&a.0)); // numeric ids => lexical = temporal
    out.truncate(200);
    out
}

fn handle_client(mut stream: TcpStream, shared: Arc<RwLock<Shared>>) {
    let (method, target) = match read_request(&mut stream) {
        Some(t) => t,
        None => return,
    };
    if method != "GET" {
        send(&mut stream, "405 Method Not Allowed", "text/plain", b"get only".to_vec());
        return;
    }
    let path = target.split('?').next().unwrap_or("/").to_string();
    let s = shared.read().unwrap();
    match path.as_str() {
        "/" => send(&mut stream, "200 OK", "text/html; charset=utf-8", INDEX_HTML.as_bytes().to_vec()),
        "/favicon.ico" => send(&mut stream, "404 Not Found", "text/plain", vec![]),
        "/api/status" => {
            let body = format!(
                "{{\"frames\":{},\"last_frame_ts\":{},\"scene_resets\":{},\"events\":{},\"last_error\":{}}}",
                s.frames, s.last_frame_ts, s.scene_resets, s.events, json_str(&s.last_error)
            );
            send(&mut stream, "200 OK", "application/json", body.into_bytes());
        }
        "/api/events" => {
            let metas = list_events(&s.data_dir);
            let body = format!("[{}]", metas.iter().map(|(_, m)| m.as_str()).collect::<Vec<_>>().join(","));
            send(&mut stream, "200 OK", "application/json", body.into_bytes());
        }
        "/api/live" => match &s.latest {
            Some(bytes) => send(&mut stream, "200 OK", "image/jpeg", bytes.clone()),
            None => send(&mut stream, "503 Service Unavailable", "text/plain", b"no frame yet".to_vec()),
        },
        _ => {
            let ev = path.strip_prefix("/api/events/").or_else(|| path.strip_prefix("/files/"));
            if let Some(rest) = ev {
                let is_file = path.starts_with("/files/");
                let parts: Vec<&str> = rest.split('/').collect();
                if parts.len() == (if is_file { 2 } else { 1 })
                    && parts[0].chars().all(|c| c.is_ascii_digit())
                {
                    let dir = s.data_dir.join("events").join(parts[0]);
                    if is_file {
                        if EVENT_FILES.contains(&parts[1]) && dir.join(parts[1]).is_file() {
                            send_file(&mut stream, &dir.join(parts[1]), "image/jpeg");
                            return;
                        }
                    } else if dir.join("meta.json").is_file() {
                        send_file(&mut stream, &dir.join("meta.json"), "application/json");
                        return;
                    }
                }
            }
            send(&mut stream, "404 Not Found", "text/plain", b"not found".to_vec());
        }
    }
}

fn json_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn run_http(cfg: Arc<Config>, shared: Arc<RwLock<Shared>>) {
    let addr = format!("{}:{}", cfg.bind, cfg.port);
    let listener = match TcpListener::bind(&addr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("bind {addr}: {e}");
            return;
        }
    };
    eprintln!("listening on {addr}");
    for stream in listener.incoming() {
        if let Ok(stream) = stream {
            let shared = Arc::clone(&shared);
            thread::spawn(move || handle_client(stream, shared));
        }
    }
}

const INDEX_HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>frigate-monitor · main_space</title>
<style>
  :root { color-scheme: dark; }
  * { box-sizing: border-box; }
  body { margin: 0; background: #111418; color: #dde3ea; font: 15px/1.45 system-ui, sans-serif; }
  header { display: flex; gap: 18px; align-items: center; padding: 12px 20px; background: #181c22; border-bottom: 1px solid #262c35; flex-wrap: wrap; }
  header h1 { font-size: 18px; margin: 0; font-weight: 600; }
  #status-line { color: #8b97a5; font-size: 13px; font-weight: 400; }
  #live { height: 72px; border-radius: 6px; background: #000; }
  .err { color: #e07a7a; font-size: 13px; }
  main { padding: 18px 20px; display: grid; grid-template-columns: repeat(auto-fill, minmax(240px, 1fr)); gap: 16px; }
  .card { background: #181c22; border: 1px solid #262c35; border-radius: 10px; overflow: hidden; cursor: pointer; transition: border-color .15s; }
  .card:hover { border-color: #4a7dbd; }
  .card img { width: 100%; display: block; }
  .card .meta { padding: 8px 12px; font-size: 13px; color: #8b97a5; display: flex; justify-content: space-between; }
  .empty { grid-column: 1/-1; color: #8b97a5; padding: 40px; text-align: center; }
  #detail { padding: 18px 20px; }
  #detail h2 { font-size: 16px; margin: 0 0 4px; }
  #detail .sub { color: #8b97a5; font-size: 13px; margin-bottom: 14px; }
  .pair { display: grid; grid-template-columns: 1fr 1fr; gap: 14px; margin-bottom: 22px; }
  .pair figure { margin: 0; }
  .pair figcaption { font-size: 13px; color: #8b97a5; margin-bottom: 6px; }
  .pair img { width: 100%; border-radius: 8px; border: 1px solid #262c35; cursor: zoom-in; display: block; }
  .zoom-pair img { cursor: zoom-in; }
  a.back { color: #6ea1e0; text-decoration: none; font-size: 14px; }
  a.back:hover { text-decoration: underline; }
  h3 { font-size: 14px; color: #8b97a5; font-weight: 600; margin: 0 0 10px; }
  @media (max-width: 700px) { .pair { grid-template-columns: 1fr; } }
</style>
</head>
<body>
<header>
  <h1>main_space <span id="status-line">…</span></h1>
  <img id="live" alt="live" title="latest snapshot">
  <span id="live-err" class="err"></span>
</header>
<main id="list"></main>
<section id="detail" hidden></section>
<script>
const $ = (s) => document.querySelector(s);
const esc = (s) => String(s).replace(/[&<>"]/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c]));

async function jget(url) {
  const r = await fetch(url);
  if (!r.ok) throw new Error(url + ' -> ' + r.status);
  return r.json();
}

async function refreshStatus() {
  try {
    const s = await jget('/api/status');
    const now = Math.floor(Date.now() / 1000);
    const age = s.last_frame_ts ? (now - s.last_frame_ts) + 's ago' : '—';
    $('#status-line').textContent =
      `frames: ${s.frames} · last ${age} · scene resets: ${s.scene_resets} · events: ${s.events}`;
    if (s.last_error) $('#status-line').textContent += ' · ' + s.last_error;
    $('#live-err').textContent = '';
  } catch (e) {
    $('#status-line').textContent = 'monitor unreachable';
  }
  // live frame (cache-busted)
  const img = $('#live');
  img.onload = () => { $('#live-err').textContent = ''; };
  img.onerror = () => { $('#live-err').textContent = 'no frame'; };
  img.src = '/api/live?t=' + Date.now();
}

async function refreshList() {
  if (!$('#detail').hidden) return;
  let events = [];
  try { events = await jget('/api/events'); } catch (e) {}
  const list = $('#list');
  if (!events.length) {
    list.innerHTML = '<div class="empty">No recorded changes yet.<br>Events appear when something in the scene is added, removed or moved and stays there.</div>';
    return;
  }
  list.innerHTML = events.map(ev => `
    <div class="card" data-id="${esc(ev.id)}">
      <img loading="lazy" src="/files/${esc(ev.id)}/thumb.jpg" alt="">
      <div class="meta"><span>${esc(ev.ts)}</span><span>${ev.regions.length} region${ev.regions.length === 1 ? '' : 's'}</span></div>
    </div>`).join('');
  list.querySelectorAll('.card').forEach(c => c.onclick = () => showEvent(c.dataset.id));
}

async function showEvent(id) {
  let ev;
  try { ev = await jget('/api/events/' + id); } catch (e) { return; }
  $('#list').hidden = true;
  const d = $('#detail');
  d.hidden = false;
  d.innerHTML = `
    <a class="back" href="#" id="back">← all events</a>
    <h2 style="margin-top:12px">${esc(ev.ts)}</h2>
    <div class="sub">${ev.regions.length} changed region${ev.regions.length === 1 ? '' : 's'} · ${ev.width}×${ev.height} · outlined in red</div>
    <h3>Full frame — before / after</h3>
    <div class="pair">
      <figure><figcaption>before</figcaption><img src="/files/${id}/before.jpg" alt="before"></figure>
      <figure><figcaption>after</figcaption><img src="/files/${id}/after.jpg" alt="after"></figure>
    </div>
    <h3>Zoom — largest changed region</h3>
    <div class="pair zoom-pair">
      <figure><figcaption>before</figcaption><img src="/files/${id}/before_z.jpg" alt="before zoom"></figure>
      <figure><figcaption>after</figcaption><img src="/files/${id}/after_z.jpg" alt="after zoom"></figure>
    </div>`;
  $('#back').onclick = (e) => { e.preventDefault(); showList(); };
  d.querySelectorAll('img').forEach(im => im.onclick = () => window.open(im.src, '_blank'));
}

function showList() {
  $('#detail').hidden = true;
  $('#list').hidden = false;
  refreshList();
}

refreshStatus();
refreshList();
setInterval(refreshStatus, 15000);
setInterval(refreshList, 15000);
</script>
</body>
</html>"##;

fn main() {
    // Offline self-test: record a synthetic event from two JPEG files and
    // exit. Used to validate the outline/region/crop pipeline.
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 5 && args[1] == "--selftest" {
        let (before, after, data_dir) = (&args[2], &args[3], PathBuf::from(&args[4]));
        let before_bytes = std::fs::read(before).expect("read before");
        let after_bytes = std::fs::read(after).expect("read after");
        let after = decode_rgb(&after_bytes).expect("decode after");
        match record_event(&data_dir, &before_bytes, &after, now_secs()) {
            Ok(Some(id)) => println!("selftest event: {id}"),
            Ok(None) => { eprintln!("selftest: no regions found"); std::process::exit(1); }
            Err(e) => { eprintln!("selftest failed: {e}"); std::process::exit(1); }
        }
        return;
    }
    let cfg = Arc::new(parse_args());
    let shared = Arc::new(RwLock::new(Shared {
        data_dir: cfg.data_dir.clone(),
        frames: 0,
        last_frame_ts: 0,
        scene_resets: 0,
        events: 0,
        last_error: String::new(),
        latest: None,
    }));
    eprintln!(
        "frigate-monitor: rtsp={} interval={}s persist={} data={}",
        cfg.rtsp,
        cfg.interval,
        cfg.persist,
        cfg.data_dir.display()
    );
    let cfg2 = Arc::clone(&cfg);
    let shared2 = Arc::clone(&shared);
    thread::spawn(move || run_capture(cfg2, shared2));
    run_http(cfg, shared);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene(width: u32, height: u32, fill: u8) -> image::RgbImage {
        image::RgbImage::from_pixel(width, height, image::Rgb([fill, fill, fill]))
    }

    fn stamp(img: &mut image::RgbImage, x: u32, y: u32, w: u32, h: u32, c: image::Rgb<u8>) {
        for py in y..(y + h) {
            for px in x..(x + w) {
                if px < img.width() && py < img.height() {
                    *img.get_pixel_mut(px, py) = c;
                }
            }
        }
    }

    /// Stable scene never triggers.
    #[test]
    fn no_event_on_stable_scene() {
        let mut det = Detector::new(64, 3);
        let mut frame = scene(64, 48, 128);
        let (reset, trig) = det.step(&frame);
        assert!(reset);
        for _ in 0..20 {
            let (reset, trig) = det.step(&frame);
            assert!(!reset);
            assert!(trig.is_empty());
        }
    }

    /// A static object left in the scene triggers once it has persisted for
    /// `persist` consecutive frames.
    #[test]
    fn static_object_triggers() {
        let mut det = Detector::new(64, 3);
        let base = scene(64, 48, 128);
        let (reset, _) = det.step(&base);
        assert!(reset);

        let mut frame = base.clone();
        stamp(&mut frame, 16, 16, 16, 16, image::Rgb([255, 0, 0])); // "scissors"
        let mut triggered_at = None;
        for i in 1..=10 {
            let (reset, trig) = det.step(&frame.clone());
            assert!(!reset);
            if !trig.is_empty() && triggered_at.is_none() {
                triggered_at = Some(i);
            }
        }
        assert_eq!(triggered_at, Some(3), "should trigger on the 3rd consecutive diff");
    }

    /// An object that keeps moving (a walking person) never triggers.
    #[test]
    fn moving_object_never_triggers() {
        let mut det = Detector::new(64, 3);
        let base = scene(64, 48, 128);
        let (reset, _) = det.step(&base);
        assert!(reset);

        // 8x8 blob sliding one block (16 px) every step.
        for x in (0..48).step_by(16) {
            let mut frame = base.clone();
            stamp(&mut frame, x, 8, 8, 8, image::Rgb([0, 0, 255]));
            let (reset, trig) = det.step(&frame);
            assert!(!reset);
            assert!(trig.is_empty(), "moving blob at x={x} must not trigger");
        }
    }

    /// Removing an object that the background has absorbed also triggers.
    #[test]
    fn removal_triggers() {
        let mut det = Detector::new(64, 3);
        // Scene with the object for a while, so the background absorbs it.
        let mut with_obj = scene(64, 48, 128);
        stamp(&mut with_obj, 16, 16, 16, 16, image::Rgb([255, 0, 0]));
        let (reset, _) = det.step(&with_obj.clone());
        assert!(reset);
        for _ in 0..10 {
            let (_, trig) = det.step(&with_obj.clone());
            assert!(trig.is_empty());
        }
        // Now the object is gone: the (object-absorbing) background differs
        // from the clean frame and the difference persists.
        let clean = scene(64, 48, 128);
        let mut triggered_at = None;
        for i in 1..=10 {
            let (reset, trig) = det.step(&clean.clone());
            assert!(!reset);
            if !trig.is_empty() && triggered_at.is_none() {
                triggered_at = Some(i);
            }
        }
        assert_eq!(triggered_at, Some(3));
    }

    /// A huge global change (lights on/off) re-seeds instead of triggering.
    #[test]
    fn global_change_reseeds() {
        let mut det = Detector::new(64, 3);
        let (reset, _) = det.step(&scene(64, 48, 128));
        assert!(reset);
        let (reset, trig) = det.step(&scene(64, 48, 250)); // everything differs
        assert!(reset, "global change should re-seed the background");
        assert!(trig.is_empty());
        // ...and is quiet afterwards.
        let (reset, trig) = det.step(&scene(64, 48, 250));
        assert!(!reset);
        assert!(trig.is_empty());
    }

    fn fmt_ts_roundtrip() {
        assert_eq!(fmt_ts(0), "1970-01-01T00:00:00");
        assert_eq!(fmt_ts(1_700_000_000), "2023-11-14T22:13:20"); // UTC
        assert_eq!(fmt_ts(1_643_695_199), "2022-02-01T05:59:59"); // Feb: y+1 case
    }
    #[test]
    fn ts_formatting() {
        fmt_ts_roundtrip();
    }
}
