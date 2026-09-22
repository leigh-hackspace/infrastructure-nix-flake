//! frigate-monitor — persistent scene-change detector for an RTSP stream.
//!
//! Every `--interval` seconds a JPEG snapshot is grabbed from an RTSP stream
//! (via ffmpeg) and fed to the detector in [`detect`].  The detector only
//! records a change once the affected area has *settled*: the region must
//! differ from the slowly adapting background and be completely still for
//! `--persist` consecutive snapshots, and the whole frame must also be still
//! (moving people are dismissed).  When an event fires, the "before" image
//! is the snapshot from just before the change began, so a moved chair shows
//! its old spot in "before" and its new spot in "after".  Two guards keep
//! bogus events out: solid-colour glitch frames (a grey/black frame from an
//! RTSP dropout) are ignored entirely, and a change whose "before" snapshot
//! is no longer buffered (background reset mid-change, or a change that
//! took >15 min to settle) is not recorded at all — never diffed against a
//! meaningless frame.
//!
//! Each event is stored under `<data-dir>/events/<id>/`:
//!   thumb.jpg    small "after" thumbnail
//!   before.jpg   full-res before with the changed regions boxed
//!   after.jpg    full-res after  with the changed regions boxed
//!   diff.jpg     after, dimmed, with the changed pixels tinted red
//!   before_z.jpg / after_z.jpg   zoomed crops of the largest region
//!   meta.json    id, timestamp, region boxes (full-resolution pixels)
//!
//! The web UI is a Dioxus SPA (frontend/dist, embedded at build time):
//!   GET /                       SPA
//!   GET /api/status             {frames, last_frame_ts, scene_resets,
//!                                blank_frames, skipped_no_before, events}
//!   GET /api/events?limit&before  paginated metas, newest first
//!   GET /api/events/<id>        one meta
//!   GET /files/<id>/<file>      one of the event JPEGs
//!   GET /api/live               latest raw snapshot

mod assets;
mod detect;
mod events;
mod imgutil;
mod server;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use detect::{Detector, Fired};
use events::{scale_boxes, Box};

const DEFAULT_BIND: &str = "0.0.0.0";
const DEFAULT_PORT: &str = "8090";
const DEFAULT_RTSP: &str = "rtsp://10.3.1.20:8554/main_space";
const DEFAULT_FFMPEG: &str = "ffmpeg";
const DEFAULT_INTERVAL: f64 = 10.0;
const FFMPEG_TIMEOUT: Duration = Duration::from_secs(20);
/// Snapshots kept for "before" images (must cover persist + grace + margin).
const RING_MAX: usize = 90;

pub struct Config {
    pub bind: String,
    pub port: u16,
    pub rtsp: String,
    pub ffmpeg: String,
    pub interval: f64,
    pub width: u32,
    pub persist: u32,
    pub data_dir: PathBuf,
}

impl Clone for Config {
    fn clone(&self) -> Self {
        Config {
            bind: self.bind.clone(),
            port: self.port,
            rtsp: self.rtsp.clone(),
            ffmpeg: self.ffmpeg.clone(),
            interval: self.interval,
            width: self.width,
            persist: self.persist,
            data_dir: self.data_dir.clone(),
        }
    }
}

fn parse_args() -> Config {
    let mut cfg = Config {
        bind: DEFAULT_BIND.to_string(),
        port: DEFAULT_PORT.parse().unwrap(),
        rtsp: DEFAULT_RTSP.to_string(),
        ffmpeg: DEFAULT_FFMPEG.to_string(),
        interval: DEFAULT_INTERVAL,
        width: detect::W,
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

/// Ring of snapshots, indexed by the detector's frame counter (see
/// [`Detector::frame`]).  The entry for frame 0 is pushed right after a
/// (re)seed; every analysed frame is pushed afterwards.
struct RingEntry {
    idx: u64,
    jpeg: Vec<u8>,
}

/// Newest ring entry at or before `idx` (the "before" image).
fn ring_before(ring: &VecDeque<RingEntry>, idx: u64) -> Option<Vec<u8>> {
    ring.iter()
        .rev()
        .find(|e| e.idx <= idx)
        .map(|e| e.jpeg.clone())
}

fn run_capture(cfg: Arc<Config>, shared: Arc<RwLock<server::Shared>>) {
    let mut detector = Detector::new(cfg.width, cfg.persist);
    let mut ring: VecDeque<RingEntry> = VecDeque::with_capacity(RING_MAX);
    let tmp = cfg.data_dir.join("frame.jpg");
    let _ = std::fs::create_dir_all(&cfg.data_dir);

    loop {
        let t0 = Instant::now();
        let ts = events::now_secs();
        let res = grab_frame(&cfg.ffmpeg, &cfg.rtsp, &tmp).and_then(|()| {
            std::fs::read(&tmp).map_err(|e| e.to_string())
        });
        match res {
            Ok(jpeg) => match imgutil::decode_rgb(&jpeg) {
                Ok(full) => {
                    if let Some(reason) = imgutil::blank_reason(&full) {
                        // Stream glitch: a solid grey/black/colour frame (e.g.
                        // a brief RTSP dropout or decoder hiccup).  Never feed
                        // it to the detector (it would reset the background to
                        // garbage) and never buffer it as a future "before"
                        // image — diffs are only ever made against real frames.
                        let mut s = shared.write().unwrap();
                        s.blank_frames += 1;
                        s.last_error = format!("ignored blank frame ({reason})");
                    } else {
                        let step = detector.step(&full);
                        if step.reset {
                            // New background baseline: the snapshot ring
                            // restarts so "before" images are never from
                            // before a reset.
                            ring.clear();
                            ring.push_back(RingEntry { idx: detector.frame, jpeg: jpeg.clone() });
                            let mut s = shared.write().unwrap();
                            s.frames += 1;
                            s.last_frame_ts = ts;
                            s.scene_resets += 1;
                            s.last_error.clear();
                            s.latest = Some(jpeg);
                        } else {
                            for fired in step.fired {
                                record(&cfg, &detector, &ring, &shared, &full, fired, ts);
                            }
                            ring.push_back(RingEntry { idx: detector.frame, jpeg: jpeg.clone() });
                            while ring.len() > RING_MAX {
                                ring.pop_front();
                            }
                            let mut s = shared.write().unwrap();
                            s.frames += 1;
                            s.last_frame_ts = ts;
                            s.last_error.clear();
                            s.latest = Some(jpeg);
                        }
                    }
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

fn record(
    cfg: &Config,
    detector: &Detector,
    ring: &VecDeque<RingEntry>,
    shared: &Arc<RwLock<server::Shared>>,
    after_full: &image::RgbImage,
    fired: Fired,
    ts: i64,
) {
    // The "before" snapshot is what makes an event meaningful: it shows the
    // frame just before the change began and is diffed against "after".
    // The ring normally covers far more than the persist+grace window, so a
    // miss means the background (and ring) reset in the middle of this
    // change, or the change's `born` frame predates the ring (a block that
    // stayed foreground-but-flickering for >15 min before the GRACE path let
    // it fire).  Either way the true "before" is gone; recording would store
    // a grey placeholder and a bogus all-red diff, so drop the event instead.
    let before_bytes = match ring_before(ring, fired.before_idx) {
        Some(b) => b,
        None => {
            eprintln!(
                "event skipped: no before snapshot (before_idx {})",
                fired.before_idx
            );
            shared.write().unwrap().skipped_no_before += 1;
            return;
        }
    };
    // Belt-and-braces: the ring only ever holds frames that decoded at the
    // detector's current resolution, but if the "before" ever turns out
    // undecodable or size-mismatched, skip rather than fabricate a placeholder.
    let before_usable = imgutil::decode_rgb(&before_bytes)
        .map(|img| img.width() == after_full.width() && img.height() == after_full.height())
        .unwrap_or(false);
    if !before_usable {
        eprintln!(
            "event skipped: unusable before snapshot (before_idx {})",
            fired.before_idx
        );
        shared.write().unwrap().skipped_no_before += 1;
        return;
    }
    let (dw, dh) = detector.det_dims();
    let boxes: Vec<Box> = scale_boxes(
        &fired.regions,
        after_full.width(),
        dw,
        after_full.height(),
        dh,
    );
    match events::record_event(&cfg.data_dir, Some(&before_bytes), after_full, &boxes, ts) {
        Ok(Some(_)) => {}
        Ok(None) => eprintln!("event skipped: no regions"),
        Err(e) => eprintln!("event record failed: {e}"),
    }
}

// ---------------------------------------------------------------------------
// HTTP server
// ---------------------------------------------------------------------------

fn run_http(cfg: Config, shared: Arc<RwLock<server::Shared>>) {
    server::run(cfg, shared);
}

// ---------------------------------------------------------------------------
// Offline self-test
// ---------------------------------------------------------------------------

/// Record a synthetic event from two JPEG files (before/after), computing
/// the changed regions directly from the full-resolution difference.  Used
/// to validate the image pipeline without a camera.
fn selftest(before_path: &str, after_path: &str, data_dir: &str) {
    let before_bytes = std::fs::read(before_path).expect("read before");
    let after_bytes = std::fs::read(after_path).expect("read after");
    let after = imgutil::decode_rgb(&after_bytes).expect("decode after");
    let before = imgutil::decode_rgb(&before_bytes).expect("decode before");
    if before.width() != after.width() || before.height() != after.height() {
        eprintln!("selftest: images differ in size");
        std::process::exit(1);
    }
    let mask = imgutil::diff_mask(&before, &after, 32);
    let (fw, fh) = (after.width(), after.height());
    let regions = detect::connected_regions(&mask, fw, fh, detect::min_area(fw as usize, fh as usize));
    let boxes: Vec<Box> = regions
        .iter()
        .map(|r| Box { x: r.x, y: r.y, w: r.w, h: r.h })
        .collect();
    let dir = PathBuf::from(data_dir);
    match events::record_event(&dir, Some(&before_bytes), &after, &boxes, events::now_secs()) {
        Ok(Some(id)) => println!("selftest event: {id}"),
        Ok(None) => {
            eprintln!("selftest: no regions found");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("selftest failed: {e}");
            std::process::exit(1);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 5 && args[1] == "--selftest" {
        selftest(&args[2], &args[3], &args[4]);
        return;
    }
    let cfg = Arc::new(parse_args());
    let shared = Arc::new(RwLock::new(server::Shared {
        data_dir: cfg.data_dir.clone(),
        frames: 0,
        last_frame_ts: 0,
        scene_resets: 0,
        blank_frames: 0,
        skipped_no_before: 0,
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
    run_http((*cfg).clone(), shared);
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

    /// Seed helper: returns a detector whose background is `base`.
    fn seed(det: &mut Detector, base: &image::RgbImage) {
        let s = det.step(base);
        assert!(s.reset, "first step must seed");
        assert!(s.fired.is_empty());
    }

    /// Run snapshots, returning the fired events (each step must not reset).
    fn run(det: &mut Detector, frames: &[&image::RgbImage]) -> Vec<Fired> {
        let mut out = Vec::new();
        for f in frames {
            let s = det.step(f);
            assert!(!s.reset, "unexpected scene reset");
            out.extend(s.fired);
        }
        out
    }

    /// A stable scene never fires.
    #[test]
    fn no_event_on_stable_scene() {
        let mut det = Detector::new(64, 3);
        let base = scene(64, 48, 128);
        seed(&mut det, &base);
        let mut fired = 0;
        for _ in 0..30 {
            let s = det.step(&base);
            assert!(!s.reset);
            fired += s.fired.len();
        }
        assert_eq!(fired, 0);
    }

    /// A static object left in the scene fires exactly once, shortly after
    /// it has been still for `persist` snapshots; "before" points at the
    /// frame just before it appeared.
    #[test]
    fn static_object_triggers_once() {
        let mut det = Detector::new(64, 3);
        let base = scene(64, 48, 128);
        seed(&mut det, &base);

        // Two unchanged frames, then the object appears and stays.
        let mut frames = vec![base.clone(), base.clone()];
        for _ in 0..20 {
            let mut f = scene(64, 48, 128);
            stamp(&mut f, 16, 16, 16, 16, image::Rgb([255, 0, 0]));
            frames.push(f);
        }
        let frefs: Vec<&image::RgbImage> = frames.iter().collect();
        let fired = run(&mut det, &frefs);
        assert_eq!(fired.len(), 1, "a static object fires exactly once");
        // The object appears on detector frame 3 (frames[2]); before = frame 2.
        assert_eq!(fired[0].before_idx, 2, "before must precede the appearance");
        assert_eq!(fired[0].regions.len(), 1);
        let r = fired[0].regions[0];
        assert!(
            r.x <= 16 && r.y <= 16 && r.x + r.w >= 32 && r.y + r.h >= 32,
            "region must cover the object, got {r:?}"
        );
    }

    /// A moving object (a person) never fires, however long it is present.
    #[test]
    fn moving_object_never_triggers() {
        let mut det = Detector::new(64, 3);
        let base = scene(64, 48, 128);
        seed(&mut det, &base);

        // 16x16 blob cycling through block positions: it always moves
        // between snapshots, so it can never settle.
        let mut fired_total = 0;
        for i in 0..12 {
            let x = [0u32, 16, 32][i % 3];
            let mut frame = base.clone();
            stamp(&mut frame, x, 8, 16, 16, image::Rgb([0, 0, 255]));
            let s = det.step(&frame);
            assert!(!s.reset);
            fired_total += s.fired.len();
        }
        assert_eq!(fired_total, 0, "a moving blob must never trigger");
    }

    /// A slow-moving object (less than a block per snapshot) never fires
    /// either: every snapshot differs, so nothing ever settles.
    #[test]
    fn slow_moving_object_never_triggers() {
        let mut det = Detector::new(64, 3);
        let base = scene(64, 48, 128);
        seed(&mut det, &base);

        for x in (0..48).step_by(4) {
            let mut frame = base.clone();
            stamp(&mut frame, x, 8, 16, 16, image::Rgb([0, 0, 255]));
            let s = det.step(&frame);
            assert!(!s.reset);
            assert!(s.fired.is_empty(), "moving blob at x={x} must not trigger");
        }
    }

    /// Removing an object that the background absorbed also fires, with the
    /// before image showing the object still present.
    #[test]
    fn removal_of_absorbed_object_fires() {
        let mut det = Detector::new(64, 3);
        // Seed with the object present, so the bg absorbs it.
        let mut with_obj = scene(64, 48, 128);
        stamp(&mut with_obj, 16, 16, 16, 16, image::Rgb([255, 0, 0]));
        seed(&mut det, &with_obj);
        for _ in 0..10 {
            let s = det.step(&with_obj.clone());
            assert!(s.fired.is_empty());
        }
        // Remove it: exactly one event; before = frame 10 (last with object).
        let clean = scene(64, 48, 128);
        let mut fired_total = 0;
        let mut first_before = None;
        for _ in 0..30 {
            let s = det.step(&clean.clone());
            assert!(!s.reset);
            for f in s.fired {
                fired_total += 1;
                if first_before.is_none() {
                    first_before = Some(f.before_idx);
                }
            }
        }
        assert_eq!(fired_total, 1);
        assert_eq!(first_before, Some(10), "before must still show the object");
    }

    /// A previously-recorded object that is later removed fires a second,
    /// removal event, once its ack has cleared.
    #[test]
    fn placed_then_removed_object_fires_twice() {
        let mut det = Detector::new(64, 3);
        let base = scene(64, 48, 128);
        seed(&mut det, &base);

        // Place the object and let the placement event fire.
        let mut placed = scene(64, 48, 128);
        stamp(&mut placed, 16, 16, 16, 16, image::Rgb([255, 0, 0]));
        let mut fires_place = 0;
        let mut before_place = None;
        for _ in 0..60 {
            let s = det.step(&placed.clone());
            for f in s.fired {
                fires_place += 1;
                if before_place.is_none() {
                    before_place = Some(f.before_idx);
                }
            }
        }
        assert_eq!(fires_place, 1);
        assert_eq!(before_place, Some(0), "before = frame before the placement");

        // Now remove it: the removal fires once, near the removal.
        let clean = scene(64, 48, 128);
        let mut fires_remove = 0;
        for i in 0..30 {
            let s = det.step(&clean.clone());
            assert!(!s.reset);
            for f in s.fired {
                fires_remove += 1;
                assert!(f.before_idx >= 60, "removal before must be near the removal (step {i})");
            }
        }
        assert_eq!(fires_remove, 1, "removal of a recorded object fires once");
    }

    /// An object with internal motion but a constant silhouette (spinning
    /// fan, flickering screen) never fires: raw frame-diff marks it active
    /// even though the foreground shape never changes.
    #[test]
    fn internal_motion_never_triggers() {
        let mut det = Detector::new(64, 3);
        let base = scene(64, 48, 128);
        seed(&mut det, &base);

        let mut fired = 0;
        for i in 0..20 {
            let mut frame = scene(64, 48, 128);
            let c = if i % 2 == 0 { image::Rgb([255, 0, 0]) } else { image::Rgb([0, 0, 255]) };
            stamp(&mut frame, 16, 16, 16, 16, c); // same place, alternating colour
            let s = det.step(&frame);
            assert!(!s.reset);
            fired += s.fired.len();
        }
        assert_eq!(fired, 0, "a flickering object must never trigger");
    }

    /// A static change made *while a person is still walking through* only
    /// fires once the whole frame has settled (after the person leaves).
    #[test]
    fn change_during_activity_fires_after_settle() {
        let mut det = Detector::new(64, 3);
        let base = scene(64, 48, 128);
        seed(&mut det, &base);

        let person = |x: u32| -> image::RgbImage {
            let mut f = base.clone();
            stamp(&mut f, x, 4, 16, 16, image::Rgb([0, 0, 255]));
            f
        };
        let object = |x: u32| -> image::RgbImage {
            let mut f = base.clone();
            stamp(&mut f, 16, 32, 16, 8, image::Rgb([255, 0, 0]));
            stamp(&mut f, x, 4, 16, 16, image::Rgb([0, 0, 255])); // person still there
            f
        };
        let object_alone = || -> image::RgbImage {
            let mut f = base.clone();
            stamp(&mut f, 16, 32, 16, 8, image::Rgb([255, 0, 0]));
            f
        };

        // Person walks across (frames 1-3); at frame 4 an object is placed
        // while the person is still visible; person leaves at frame 7.
        let mut fired = Vec::new();
        for x in [0u32, 16, 32] {
            let s = det.step(&person(x));
            assert!(!s.reset);
        }
        for x in [0u32, 16] {
            let s = det.step(&object(x));
            assert!(!s.reset);
            fired.extend(s.fired);
        }
        for _ in 0..25 {
            let s = det.step(&object_alone());
            assert!(!s.reset);
            fired.extend(s.fired);
        }
        assert_eq!(fired.len(), 1, "exactly one event once everything settled");
        assert_eq!(fired[0].before_idx, 3, "before = frame just before the object appeared");
    }

    /// A huge global change (lights on/off) re-seeds instead of triggering.
    #[test]
    fn global_change_reseeds() {
        let mut det = Detector::new(64, 3);
        let s = det.step(&scene(64, 48, 128));
        assert!(s.reset);
        let s = det.step(&scene(64, 48, 250)); // everything differs
        assert!(s.reset, "global change should re-seed the background");
        assert!(s.fired.is_empty());
        let s = det.step(&scene(64, 48, 250));
        assert!(!s.reset);
        assert!(s.fired.is_empty());
    }
}
