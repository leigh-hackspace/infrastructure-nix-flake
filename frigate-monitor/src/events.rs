//! Event storage: one directory per event under `<data-dir>/events/<id>/`
//! with thumb.jpg, before.jpg, after.jpg, diff.jpg, before_z.jpg, after_z.jpg
//! and meta.json (id, timestamp, full-resolution region boxes).

use std::path::{Path, PathBuf};

use crate::imgutil;
use crate::detect::DetRect;

pub const EVENT_FILES: [&str; 6] = [
    "thumb.jpg",
    "before.jpg",
    "after.jpg",
    "diff.jpg",
    "before_z.jpg",
    "after_z.jpg",
];

/// A full-resolution region box: x, y, w, h (pixels in the source frame).
#[derive(Clone, Copy, Debug)]
pub struct Box {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Scale detection-resolution boxes up to full resolution.
pub fn scale_boxes(regions: &[DetRect], full_w: u32, det_w: u32, full_h: u32, det_h: u32) -> Vec<Box> {
    let sx = full_w as f64 / det_w.max(1) as f64;
    let sy = full_h as f64 / det_h.max(1) as f64;
    regions
        .iter()
        .map(|r| {
            let x = (r.x as f64 * sx).round() as u32;
            let y = (r.y as f64 * sy).round() as u32;
            let w = ((r.x + r.w) as f64 * sx).round() as u32 - x;
            let h = ((r.y + r.h) as f64 * sy).round() as u32 - y;
            Box { x, y, w: w.max(1), h: h.max(1) }
        })
        .collect()
}

fn fmt_ts(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (hh, mm, ss) = (tod / 3600, (tod % 3600) / 60, tod % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = y + if mo <= 2 { 1 } else { 0 };
    format!("{y:04}-{mo:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}")
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Record one event.  `before_bytes` is the JPEG to show as "before" (may be
/// undecodable / a different size; then a grey placeholder is used), `after`
/// is the current full frame, `boxes` the changed regions in full-res pixels.
/// Returns the event id on success, Ok(None) if there is nothing worth
/// storing (should not happen when called from the detector).
pub fn record_event(
    data_dir: &Path,
    before_bytes: Option<&[u8]>,
    after: &image::RgbImage,
    boxes: &[Box],
    ts: i64,
) -> std::io::Result<Option<String>> {
    if boxes.is_empty() {
        return Ok(None);
    }
    let (fw, fh) = (after.width(), after.height());
    let stroke = (3.0 * fw as f64 / 640.0).round().max(2.0) as u32;

    let before: image::RgbImage = match before_bytes {
        Some(b) => match imgutil::decode_rgb(b) {
            Ok(img) if img.width() == fw && img.height() == fh => img,
            _ => image::RgbImage::from_pixel(fw, fh, image::Rgb([90, 90, 90])),
        },
        None => image::RgbImage::from_pixel(fw, fh, image::Rgb([90, 90, 90])),
    };

    // Unique id from wall clock (seconds), bumping on collision.
    let events_dir = data_dir.join("events");
    std::fs::create_dir_all(&events_dir)?;
    let mut id = ts;
    let mut dir = events_dir.join(id.to_string());
    while dir.exists() {
        id += 1;
        dir = events_dir.join(id.to_string());
    }
    std::fs::create_dir_all(&dir)?;

    let box_tuples: Vec<(u32, u32, u32, u32)> = boxes.iter().map(|b| (b.x, b.y, b.w, b.h)).collect();

    let mut before_marked = before.clone();
    let mut after_marked = after.clone();
    imgutil::mark_boxes(&mut before_marked, &box_tuples, stroke);
    imgutil::mark_boxes(&mut after_marked, &box_tuples, stroke);

    let diff = imgutil::make_diff_image(&before, after, &box_tuples, stroke);
    let thumb = imgutil::downscale(&after_marked, THUMB_WIDTH);
    let (zx, zy, zw, zh) = box_tuples[0];
    let before_z = imgutil::zoom_crop(&before_marked, zx, zy, zw, zh);
    let after_z = imgutil::zoom_crop(&after_marked, zx, zy, zw, zh);

    std::fs::write(dir.join("thumb.jpg"), imgutil::encode_jpeg(&thumb, 82))?;
    std::fs::write(dir.join("before.jpg"), imgutil::encode_jpeg(&before_marked, 82))?;
    std::fs::write(dir.join("after.jpg"), imgutil::encode_jpeg(&after_marked, 82))?;
    std::fs::write(dir.join("diff.jpg"), imgutil::encode_jpeg(&diff, 85))?;
    std::fs::write(dir.join("before_z.jpg"), imgutil::encode_jpeg(&before_z, 88))?;
    std::fs::write(dir.join("after_z.jpg"), imgutil::encode_jpeg(&after_z, 88))?;

    let regions_json: Vec<String> = boxes
        .iter()
        .map(|b| {
            format!(
                "{{\"x\":{},\"y\":{},\"w\":{},\"h\":{},\"area\":{}}}",
                b.x,
                b.y,
                b.w,
                b.h,
                (b.w as u64) * (b.h as u64)
            )
        })
        .collect();
    let meta = format!(
        "{{\"id\":{},\"ts\":\"{}\",\"width\":{},\"height\":{},\"regions\":[{}]}}",
        id,
        fmt_ts(ts),
        fw,
        fh,
        regions_json.join(",")
    );
    std::fs::write(dir.join("meta.json"), meta)?;
    eprintln!(
        "event {id}: {} region(s), largest {}x{} at ({},{})",
        boxes.len(),
        zw,
        zh,
        zx,
        zy
    );
    Ok(Some(id.to_string()))
}

const THUMB_WIDTH: u32 = 320;

/// All event ids (dir names that look numeric), newest first.
pub fn event_ids(data_dir: &Path) -> Vec<u64> {
    let mut ids = Vec::new();
    if let Ok(entries) = std::fs::read_dir(data_dir.join("events")) {
        for e in entries.flatten() {
            let name = e.file_name();
            if let Ok(n) = name.to_string_lossy().parse::<u64>() {
                if e.path().join("meta.json").is_file() {
                    ids.push(n);
                }
            }
        }
    }
    ids.sort_unstable_by(|a, b| b.cmp(a));
    ids
}

pub fn event_dir(data_dir: &Path, id: u64) -> PathBuf {
    data_dir.join("events").join(id.to_string())
}

pub fn read_meta(path: &Path) -> Option<String> {
    std::fs::read_to_string(path.join("meta.json")).ok()
}

/// Page of event metas, newest first.  `before` (optional) excludes events
/// with id >= before, i.e. returns strictly older events — the infinite
/// scroll cursor.  `limit` caps the page size.
pub fn list_events_paged(data_dir: &Path, before: Option<u64>, limit: usize) -> (Vec<String>, bool) {
    let mut ids = event_ids(data_dir);
    if let Some(b) = before {
        ids.retain(|&i| i < b);
    }
    let has_more = ids.len() > limit;
    ids.truncate(limit);
    let metas = ids
        .iter()
        .filter_map(|&id| read_meta(&event_dir(data_dir, id)))
        .collect();
    (metas, has_more)
}

/// JSON string escaping.
pub fn json_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}
