//! Image helpers: decode/encode, downscale, and the event-image drawing
//! (region rectangles on before/after, the tinted diff overlay, zoom crops).

use image::RgbImage;

pub fn decode_rgb(bytes: &[u8]) -> Result<RgbImage, String> {
    image::load_from_memory(bytes)
        .map_err(|e| format!("decode: {e}"))
        .map(|d| d.to_rgb8())
}

pub fn encode_jpeg(img: &RgbImage, quality: u8) -> Vec<u8> {
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

pub fn downscale(full: &RgbImage, width: u32) -> RgbImage {
    let scale = width as f32 / full.width() as f32;
    let height = (full.height() as f32 * scale).round().max(1.0) as u32;
    image::imageops::resize(full, width, height, image::imageops::FilterType::Triangle)
}

/// Width (px) of the downscale used by [`blank_reason`].  96 px is plenty to
/// tell "one flat colour" from "a scene": ~2k sample points, ~1600x cheaper
/// than full-res.
const BLANK_CHECK_WIDTH: u32 = 96;
/// Per-channel RMS spread below which a frame counts as one flat colour
/// (solid grey/black/white, a tinted glitch, a decoder test pattern...).
const BLANK_RMS_UNIFORM: f64 = 2.5;
/// A desaturated frame that is *almost* flat is still a glitch (grey frame
/// with a bit of JPEG/sensor noise); the RMS bound for that case.
const BLANK_RMS_GREY: f64 = 8.0;
/// Mean per-pixel saturation below which the "almost flat" rule applies.
const BLANK_SAT_GREY: f64 = 2.5;

/// Camera-glitch guard: is this frame essentially blank (one flat colour,
/// usually mid-grey or black)?  RTSP interruptions and decoder hiccups hand
/// us such frames from time to time.  Feeding one to the detector would seed
/// or reset the background to garbage, and buffering one in the snapshot
/// ring would later make it a "before" image for some real change.  The
/// capture loop therefore drops blank frames entirely.
///
/// The test is a uniformity + desaturation check on a small downscale: real
/// scenes keep structure (a wide luma/chroma spread) and, when lit, colour;
/// blank frames are flat and near-grey.  Thresholds are deliberately
/// generous so genuinely dark or IR (colourless but structured) frames are
/// never rejected.
pub fn blank_reason(full: &RgbImage) -> Option<&'static str> {
    let small = downscale(full, BLANK_CHECK_WIDTH);
    let px = small.as_raw();
    let n = (small.width() as usize) * (small.height() as usize);
    if n == 0 {
        return Some("empty frame");
    }
    // Per-channel mean & mean-square so a solid *tinted* frame (e.g. solid
    // green from a broken decoder) is caught: pooling channels would smear
    // the colour into a fake spread.
    let mut sum = [0f64; 3];
    let mut sum2 = [0f64; 3];
    let mut sat = 0f64;
    for i in 0..n {
        let o = i * 3;
        for c in 0..3 {
            let v = px[o + c] as f64;
            sum[c] += v;
            sum2[c] += v * v;
        }
        let (r, g, b) = (px[o] as f64, px[o + 1] as f64, px[o + 2] as f64);
        sat += r.max(g).max(b) - r.min(g).min(b);
    }
    let mut var = 0f64;
    for c in 0..3 {
        let mean = sum[c] / n as f64;
        var += (sum2[c] / n as f64 - mean * mean).max(0.0);
    }
    let rms = (var / 3.0).sqrt();
    let sat_mean = sat / n as f64;
    if rms < BLANK_RMS_UNIFORM {
        return Some("uniform frame");
    }
    if sat_mean < BLANK_SAT_GREY && rms < BLANK_RMS_GREY {
        return Some("flat grey frame");
    }
    None
}

/// Per-pixel changed-region mask between two same-size images (max channel).
pub fn diff_mask(a: &RgbImage, b: &RgbImage, threshold: u32) -> Vec<bool> {
    let (w, h) = (a.width(), a.height());
    let n = (w * h) as usize;
    let mut mask = vec![false; n];
    let pa = a.as_raw();
    let pb = b.as_raw();
    for i in 0..n {
        let d = (pa[i * 3] as i32 - pb[i * 3] as i32)
            .abs()
            .max((pa[i * 3 + 1] as i32 - pb[i * 3 + 1] as i32).abs())
            .max((pa[i * 3 + 2] as i32 - pb[i * 3 + 2] as i32).abs());
        if d as u32 > threshold {
            mask[i] = true;
        }
    }
    mask
}

/// Draw the border of one rectangle in a bright colour.
pub fn draw_rect(img: &mut RgbImage, x0: u32, y0: u32, x1: u32, y1: u32, stroke: u32, c: image::Rgb<u8>) {
    let (w, h) = (img.width(), img.height());
    for s in 0..stroke {
        for x in x0.saturating_sub(s)..=(x1 + s).min(w - 1) {
            for (y, in_row) in [(y0.saturating_sub(s), true), ((y1 + s).min(h - 1), true)] {
                if in_row && y < h && x < w {
                    *img.get_pixel_mut(x, y) = c;
                }
            }
        }
        for y in y0.saturating_sub(s)..=(y1 + s).min(h - 1) {
            for (x, in_col) in [((x0.saturating_sub(s)).min(w - 1), true), ((x1 + s).min(w - 1), true)] {
                if in_col && x < w && y < h {
                    *img.get_pixel_mut(x, y) = c;
                }
            }
        }
    }
}

/// before/after pair with each changed region outlined by a rectangle.
/// `boxes` are already full-resolution pixel boxes.
pub fn mark_boxes(img: &mut RgbImage, boxes: &[(u32, u32, u32, u32)], stroke: u32) {
    let c = image::Rgb([255, 60, 50]);
    for &(x, y, w, h) in boxes {
        if w == 0 || h == 0 {
            continue;
        }
        let (x1, y1) = ((x + w).saturating_sub(1), (y + h).saturating_sub(1));
        draw_rect(img, x, y, x1, y1, stroke, c);
    }
}

/// Build the diff overlay: `after`, dimmed, with the pixels that actually
/// changed between before/after tinted red, then region rects drawn on top.
pub fn make_diff_image(
    before: &RgbImage,
    after: &RgbImage,
    boxes: &[(u32, u32, u32, u32)],
    stroke: u32,
) -> RgbImage {
    let mut out = after.clone();
    let dim = 0.62f32;
    for p in out.pixels_mut() {
        for c in p.0.iter_mut() {
            *c = (*c as f32 * dim) as u8;
        }
    }
    let mask = diff_mask(before, after, 32);
    let (w, h) = (out.width(), out.height());
    let red = [255u8, 50, 40];
    for &(x0, y0, bw, bh) in boxes {
        for y in y0..(y0 + bh).min(h) {
            for x in x0..(x0 + bw).min(w) {
                if !mask[(y * w + x) as usize] {
                    continue;
                }
                let p = out.get_pixel_mut(x, y);
                for c in 0..3 {
                    p.0[c] = (p.0[c] as f32 * 0.35 + red[c] as f32 * 0.65) as u8;
                }
            }
        }
    }
    mark_boxes(&mut out, boxes, stroke);
    out
}

/// Zoomed crop around one box (with padding), clamped to the image.
pub fn zoom_crop(img: &RgbImage, x: u32, y: u32, w: u32, h: u32) -> RgbImage {
    let pad_x = (w as f64 * 0.35) as u32;
    let pad_y = (h as f64 * 0.35) as u32;
    let x0 = x.saturating_sub(pad_x);
    let y0 = y.saturating_sub(pad_y);
    let x1 = (x + w + pad_x).min(img.width());
    let y1 = (y + h + pad_y).min(img.height());
    let cw = (x1 - x0).max(8);
    let ch = (y1 - y0).max(8);
    let mut tmp = img.clone();
    image::imageops::crop_imm(&mut tmp, x0, y0, cw, ch).to_image()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, rgb: [u8; 3]) -> RgbImage {
        RgbImage::from_pixel(w, h, image::Rgb(rgb))
    }

    /// Solid frames of any colour (the grey/black/white glitches, plus a
    /// tinted decoder artefact) must be rejected.
    #[test]
    fn solid_frames_are_blank() {
        for c in [
            [0, 0, 0],        // black
            [255, 255, 255],  // white
            [90, 90, 90],     // mid grey
            [16, 16, 16],     // near-black
            [0, 255, 0],      // tinted glitch
            [255, 0, 0],
        ] {
            assert!(
                blank_reason(&solid(3840, 2160, c)).is_some(),
                "solid {c:?} must be flagged blank"
            );
        }
    }

    /// A colourless (IR/night-style) frame with real content must pass.
    #[test]
    fn structured_grey_is_not_blank() {
        let mut img = RgbImage::new(3840, 2160);
        for (x, y, p) in img.enumerate_pixels_mut() {
            let v = if ((x / 256 + y / 256) % 2) == 0 { 30u8 } else { 200u8 };
            *p = image::Rgb([v, v, v]);
        }
        assert!(blank_reason(&img).is_none(), "structured grey must not be blank");
    }

    /// A dark room with one bright region (a lit screen/window at night)
    /// must pass, even though it is mostly low-luma and low-saturation.
    #[test]
    fn dark_scene_with_content_is_not_blank() {
        let mut img = solid(3840, 2160, [18, 18, 18]);
        for y in 800..1300 {
            for x in 1500..2300 {
                *img.get_pixel_mut(x, y) = image::Rgb([235, 235, 235]);
            }
        }
        assert!(blank_reason(&img).is_none(), "dark scene with content must not be blank");
    }

    /// A normal lit scene (structure + colour) must pass.
    #[test]
    fn lit_scene_is_not_blank() {
        let mut img = solid(3840, 2160, [120, 118, 110]);
        for (x, y, p) in img.enumerate_pixels_mut() {
            let band = ((x / 128 + y / 128) % 3) as u8;
            *p = image::Rgb([
                (120 + band * 40),
                (100 + band * 30),
                (90 + band * 10),
            ]);
        }
        assert!(blank_reason(&img).is_none(), "lit scene must not be blank");
    }
}
