//! Scene-change detector with motion gating ("only static objects count").
//!
//! The core problem: distinguish a *static* change (a chair moved, a pencil
//! placed on / taken off a table) from a moving object (a person walking
//! through).  The rule that makes this work is:
//!
//!   > wait for the area being diffed to settle before comparing.
//!
//! Per analysis snapshot (the frame downscaled to [`W`] wide) we maintain:
//!
//!   bg       long-term per-pixel background (EMA).
//!   fg       per-pixel "differs from background" mask, minus shadows.
//!   act      per-pixel "something is moving" = raw frame-to-frame diff OR a
//!            change in the foreground mask itself (fg XOR prev-fg).  The
//!            XOR term is immune to JPEG noise on static edges (the pixel
//!            differs from bg every frame but the *shape* of the difference
//!            is stable); the raw-diff term catches motion that keeps the
//!            same silhouette (spinning fan, waving flag, shifting weight).
//!
//! Then a grid of blocks (BLOCK×BLOCK px) is tracked:
//!
//!   born      first frame this block became foreground in the current
//!             episode.  `before` images come from born-1 — just before the
//!             change began (a moving chair is never the "before" of its own
//!             move).
//!   quiet     consecutive frames the block is foreground *and* still.
//!   acked     the block already produced an event; it stays silent while
//!             its content is unchanged, and is absorbed into the
//!             background (at [`ACK_ALPHA`]) so that its later *removal* is
//!             visible as a fresh difference.
//!
//! A block becomes *ready* once it has been foreground and completely still
//! for [`persist`] consecutive snapshots.  An event fires only when the
//! whole frame has also been still for [`persist`] snapshots (a moving
//! person produces nothing, and both halves of a single change — a chair
//! leaving spot A and arriving at spot B — coalesce into one before/after
//! pair).  In a room that never fully settles, ready blocks fire anyway
//! after [`GRACE`] extra frames, but only the individually settled blocks
//! are included.
//!
//! Lifecycle of an object:
//!   1. appears -> fg, settles -> event fired, blocks acked.
//!   2. sits -> bg absorbed under it over a couple of minutes; ack cleared
//!      once the spot matches bg again.
//!   3. changes content while acked (removed/replaced/slid) -> the block
//!      opens a *mismatch episode* as a fresh candidate (remembering its old
//!      content as a "ghost"); if the old content comes back (someone just
//!      stood in front of it) the episode is cancelled and the ack restored.
//!      Otherwise the mismatch settles and fires its own event, so removals
//!      of previously-recorded objects are recorded too.

use image::RgbImage;

/// Detection width.  The stream is downscaled to this many pixels wide.
pub const W: u32 = 640;
/// Block size in detection pixels; analysis happens per block.
const BLOCK: u32 = 16;
/// Per-pixel max-channel |frame - bg| above which a pixel is foreground.
const FG_THRESHOLD: u32 = 32;
/// Per-pixel max-channel |frame - prev| above which a pixel is "moving".
const MOT_THRESHOLD: u32 = 24;
/// Fraction of a block's pixels that must be foreground to count as such.
const FG_EPS: f32 = 0.30;
/// Fraction of a block's pixels that must be active to count as moving.
const ACT_EPS: f32 = 0.10;
/// More than this fraction of the frame differing => re-seed background.
const GLOBAL_RESET_FRAC: f32 = 0.30;
/// Background adaptation rate per snapshot (matching pixels).
const BG_ALPHA: f32 = 0.02;
/// Background adaptation rate for acked (confirmed static) foreground.
const ACK_ALPHA: f32 = 0.12;
/// Extra still+ready frames a candidate may wait while the frame is busy.
pub const GRACE: u32 = 8;
/// Clean frames after which an acked block re-arms.
const ACK_CLEAR: u32 = 10;
/// Mean-colour distance above which acked content counts as "changed".
const ACK_COLOR_DIST: f32 = 50.0;
/// Shadow heuristics (vs the reference pixel, bg or prev frame).
const SHADOW_MIN_RATIO: f32 = 0.30;
const SHADOW_MAX_RATIO: f32 = 0.95;
const SHADOW_CHROMA_ERR: f32 = 24.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// One fired event: the settled regions (detection pixels) plus the index of
/// the snapshot to use as "before" (one before the earliest change began).
#[derive(Debug)]
pub struct Fired {
    pub regions: Vec<DetRect>,
    pub before_idx: u64,
}

#[derive(Debug)]
pub struct Step {
    pub reset: bool,
    pub fired: Vec<Fired>,
}

/// Mean colour of a block's foreground at the moment it was acked; used both
/// as the absorption target and to detect later content changes.
#[derive(Clone, Copy, Default)]
struct Ack {
    mean: [f32; 3],
}

struct BlockState {
    born: i64,        // first foreground frame of this episode, or -1
    quiet: u32,       // consecutive foreground-and-still frames
    ready_since: i64, // first frame quiet reached `persist`, or -1
    acked: bool,
    clean_count: u32,   // consecutive clean frames while acked
    ack: Ack,           // content this block was acked for
    ghost: Option<Ack>, // old content while a mismatch episode is open
}

pub struct Detector {
    /// Detection size (downscaled working resolution).
    det_w: u32,
    det_h: u32,
    /// Full (source) resolution — for scaling region boxes.
    pub full_w: u32,
    pub full_h: u32,
    bg: Vec<f32>,
    have_bg: bool,
    bw: usize,
    bh: usize,
    blocks: Vec<BlockState>,
    /// Previous snapshot's downscaled RGB + foreground mask (for activity).
    prev: Option<Box<[u8]>>,
    prev_fg: Vec<bool>,
    /// Consecutive snapshots with no activity anywhere.
    global_quiet: u32,
    /// Global frame counter.  Incremented once per analysed snapshot; the
    /// frame that seeds the background is 0 and not itself analysed.
    /// Reset to 0 on every (re)seed, in step with the caller's snapshot ring.
    pub frame: u64,
    persist: u32,
}

fn luma(px: &[u8; 3]) -> f32 {
    0.299 * px[0] as f32 + 0.587 * px[1] as f32 + 0.114 * px[2] as f32
}

/// Is `px` a shadow of `ref`?  Shadows darken a surface while preserving its
/// hue: the pixel must be darker than the reference, not too dark, and close
/// to the reference scaled down by the luma ratio.
fn is_shadow(px: &[u8; 3], r: &[f32; 3]) -> bool {
    let yp = luma(px);
    let yr = luma(&[r[0] as u8, r[1] as u8, r[2] as u8]).max(1.0);
    if yp >= yr {
        return false;
    }
    let ratio = yp / yr;
    if !(SHADOW_MIN_RATIO..=SHADOW_MAX_RATIO).contains(&ratio) {
        return false;
    }
    let mut err = 0.0f32;
    for c in 0..3 {
        let d = px[c] as f32 - ratio * r[c];
        err += d * d;
    }
    err.sqrt() < SHADOW_CHROMA_ERR
}

fn downscale(full: &RgbImage, width: u32) -> RgbImage {
    let scale = width as f32 / full.width() as f32;
    let height = (full.height() as f32 * scale).round().max(1.0) as u32;
    image::imageops::resize(full, width, height, image::imageops::FilterType::Triangle)
}

impl Detector {
    pub fn new(width: u32, persist: u32) -> Self {
        Detector {
            det_w: width,
            det_h: 0,
            full_w: 0,
            full_h: 0,
            bg: Vec::new(),
            have_bg: false,
            bw: 0,
            bh: 0,
            blocks: Vec::new(),
            prev: None,
            prev_fg: Vec::new(),
            global_quiet: 0,
            frame: 0,
            persist: persist.max(1),
        }
    }

    /// Detection (analysis) resolution.
    pub fn det_dims(&self) -> (u32, u32) {
        (self.det_w, self.det_h)
    }

    fn seed(&mut self, full: &RgbImage) {
        let small = downscale(full, self.det_w);
        self.det_h = small.height();
        self.full_w = full.width();
        self.full_h = full.height();
        let n = (self.det_w * self.det_h) as usize;
        self.bg = Vec::with_capacity(n * 3);
        for p in small.pixels() {
            self.bg
                .extend_from_slice(&[p[0] as f32, p[1] as f32, p[2] as f32]);
        }
        self.bw = self.det_w.div_ceil(BLOCK) as usize;
        self.bh = self.det_h.div_ceil(BLOCK) as usize;
        self.blocks = (0..self.bw * self.bh)
            .map(|_| BlockState {
                born: -1,
                quiet: 0,
                ready_since: -1,
                acked: false,
                clean_count: 0,
                ack: Ack::default(),
                ghost: None,
            })
            .collect();
        self.prev = Some(small.into_raw().into_boxed_slice());
        self.prev_fg = vec![false; n];
        self.global_quiet = 0;
        self.frame = 0;
        self.have_bg = true;
    }

    /// Analyse one full-resolution snapshot.  Returns either a whole-scene
    /// reset (background re-seeded; caller must clear its snapshot ring) or
    /// the settled-change events that fired this frame.
    pub fn step(&mut self, full: &RgbImage) -> Step {
        // First frame or size change: (re)seed and reset the model.
        if !self.have_bg || full.width() != self.full_w || full.height() != self.full_h {
            self.seed(full);
            return Step {
                reset: true,
                fired: Vec::new(),
            };
        }
        self.frame += 1;

        let small = downscale(full, self.det_w);
        let (w, h) = (self.det_w, self.det_h);
        let n = (w * h) as usize;
        let px = small.as_raw();
        let prev = self.prev.as_ref().expect("prev set after seed");

        // ---- per-pixel foreground & activity masks ------------------------
        let mut fg = vec![false; n];
        let mut act = vec![false; n];
        let mut masked = 0usize;
        let bg = &mut self.bg;
        for i in 0..n {
            let o = i * 3;
            let f = [px[o], px[o + 1], px[o + 2]];
            let d_bg = (f[0] as i32 - bg[o] as i32)
                .abs()
                .max((f[1] as i32 - bg[o + 1] as i32).abs())
                .max((f[2] as i32 - bg[o + 2] as i32).abs());
            if d_bg as u32 > FG_THRESHOLD && !is_shadow(&f, &[bg[o], bg[o + 1], bg[o + 2]]) {
                fg[i] = true;
                masked += 1;
            }
            // Activity: differs from the previous snapshot...
            let p = [prev[o], prev[o + 1], prev[o + 2]];
            let d_mot = (f[0] as i32 - p[0] as i32)
                .abs()
                .max((f[1] as i32 - p[1] as i32).abs())
                .max((f[2] as i32 - p[2] as i32).abs());
            if d_mot as u32 > MOT_THRESHOLD {
                act[i] = true;
            }
        }

        // Whole-scene guard: lights/camera reset => reseed instead of firing.
        if masked as f32 > GLOBAL_RESET_FRAC * n as f32 {
            self.seed(full);
            return Step {
                reset: true,
                fired: Vec::new(),
            };
        }

        // Fill 1px holes in fg so solid objects read as solid (dilate)...
        let mut fgc = vec![false; n];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) as usize;
                if fg[i] {
                    fgc[i] = true;
                    continue;
                }
                let (x0, x1) = (x.saturating_sub(1), (x + 1).min(w - 1));
                let (y0, y1) = (y.saturating_sub(1), (y + 1).min(h - 1));
                'neigh: for yy in y0..=y1 {
                    for xx in x0..=x1 {
                        if fg[(yy * w + xx) as usize] {
                            fgc[i] = true;
                            break 'neigh;
                        }
                    }
                }
            }
        }
        // ...and mark activity where the foreground *shape* changed (this is
        // the JPEG-noise-immune motion signal).
        let mut global_busy = false;
        for i in 0..n {
            if fgc[i] != self.prev_fg[i] {
                act[i] = true;
            }
        }

        // ---- per-block settle tracking ------------------------------------
        let (bw, bh) = (self.bw, self.bh);
        let nb = bw * bh;
        // Absorption target for acked content-matching blocks (None = don't
        // absorb this block's foreground).
        let mut absorb: Vec<Option<[f32; 3]>> = vec![None; nb];
        let mut ready_now: Vec<usize> = Vec::new();
        for bi in 0..nb {
            let by = bi / bw;
            let bx = bi % bw;
            let mut fg_cnt = 0usize;
            let mut act_cnt = 0usize;
            let mut tot = 0usize;
            let y0 = by * BLOCK as usize;
            let x0 = bx * BLOCK as usize;
            for y in y0..(y0 + BLOCK as usize).min(h as usize) {
                for x in x0..(x0 + BLOCK as usize).min(w as usize) {
                    let i = y * w as usize + x;
                    tot += 1;
                    if fgc[i] {
                        fg_cnt += 1;
                    }
                    if act[i] {
                        act_cnt += 1;
                    }
                }
            }
            let fg_now = tot > 0 && fg_cnt as f32 >= FG_EPS * tot as f32;
            let act_now = tot > 0 && act_cnt as f32 >= ACT_EPS * tot as f32;
            if act_now {
                global_busy = true;
            }

            // A block with an outstanding event stays acked while its
            // content is unchanged, and is absorbed into the background so a
            // later removal of the object is a real difference again.
            if self.blocks[bi].acked {
                if fg_now {
                    self.blocks[bi].clean_count = 0;
                    let mean = block_fg_mean(
                        by,
                        bx,
                        self.det_w as usize,
                        self.det_h as usize,
                        &fgc,
                        px,
                        w as usize,
                    );
                    if mean_dist(mean, self.blocks[bi].ack.mean) <= ACK_COLOR_DIST {
                        absorb[bi] = Some(self.blocks[bi].ack.mean);
                        continue;
                    }
                    // Content changed: open a mismatch episode as a fresh
                    // candidate, remembering the ack so it can be restored
                    // if the old content comes back (someone stood in front).
                    let old = self.blocks[bi].ack;
                    let b = &mut self.blocks[bi];
                    b.ghost = Some(old);
                    b.acked = false;
                    b.born = self.frame as i64;
                    b.quiet = 0;
                    b.ready_since = -1;
                } else {
                    let b = &mut self.blocks[bi];
                    b.clean_count += 1;
                    if b.clean_count >= ACK_CLEAR {
                        // Background re-absorbed this spot: any later change
                        // here is a brand-new episode.
                        b.acked = false;
                        b.clean_count = 0;
                        b.born = -1;
                        b.quiet = 0;
                        b.ready_since = -1;
                    }
                    continue;
                }
            }

            // Ghost candidates revert to acked when their old content is
            // back, or drop the ghost when the spot goes clean.
            if let Some(g) = self.blocks[bi].ghost {
                if fg_now {
                    let mean = block_fg_mean(
                        by,
                        bx,
                        self.det_w as usize,
                        self.det_h as usize,
                        &fgc,
                        px,
                        w as usize,
                    );
                    if mean_dist(mean, g.mean) <= ACK_COLOR_DIST {
                        let b = &mut self.blocks[bi];
                        b.acked = true;
                        b.ack = g;
                        b.ghost = None;
                        b.born = -1;
                        b.quiet = 0;
                        b.ready_since = -1;
                        absorb[bi] = Some(g.mean);
                        continue;
                    }
                } else {
                    self.blocks[bi].ghost = None;
                }
            }

            // Ordinary candidate tracking.
            let b = &mut self.blocks[bi];
            if fg_now {
                if b.born < 0 {
                    b.born = self.frame as i64;
                }
                if act_now {
                    b.quiet = 0;
                    b.ready_since = -1;
                } else {
                    b.quiet = b.quiet.saturating_add(1);
                    if b.quiet >= self.persist {
                        if b.ready_since < 0 {
                            b.ready_since = self.frame as i64;
                        }
                        ready_now.push(bi);
                    }
                }
            } else {
                b.born = -1;
                b.quiet = 0;
                b.ready_since = -1;
                b.ghost = None;
            }
        }

        // ---- adapt the background -----------------------------------------
        // Matching pixels adapt slowly; acked (confirmed) foreground is
        // absorbed toward its recorded colour; everything else (unsettled
        // foreground) is frozen so the difference persists.
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) as usize;
                let o = i * 3;
                if !fg[i] {
                    bg[o] += (px[o] as f32 - bg[o]) * BG_ALPHA;
                    bg[o + 1] += (px[o + 1] as f32 - bg[o + 1]) * BG_ALPHA;
                    bg[o + 2] += (px[o + 2] as f32 - bg[o + 2]) * BG_ALPHA;
                    continue;
                }
                let bi = (y as usize / BLOCK as usize) * bw + (x as usize / BLOCK as usize);
                if let Some(m) = absorb[bi] {
                    bg[o] += (m[0] - bg[o]) * ACK_ALPHA;
                    bg[o + 1] += (m[1] - bg[o + 1]) * ACK_ALPHA;
                    bg[o + 2] += (m[2] - bg[o + 2]) * ACK_ALPHA;
                }
            }
        }

        // ---- fire gate ------------------------------------------------------
        // The frame must have been entirely still for `persist` snapshots
        // (or a ready block must have waited `GRACE` frames in a busy room).
        if global_busy {
            self.global_quiet = 0;
        } else {
            self.global_quiet = self.global_quiet.saturating_add(1);
        }
        let mut fired = Vec::new();
        if !ready_now.is_empty() {
            let grace = ready_now.iter().any(|&bi| {
                let b = &self.blocks[bi];
                self.frame as i64 - b.ready_since >= GRACE as i64
            });
            if self.global_quiet >= self.persist || grace {
                if let Some(ev) = self.assemble_event(&fgc, px, w) {
                    fired.push(ev);
                }
            }
        }

        self.prev = Some(px.to_vec().into_boxed_slice());
        self.prev_fg = fgc;
        Step {
            reset: false,
            fired,
        }
    }

    /// Turn the currently-ready blocks into one fired event: connected
    /// foreground components across ready blocks, bounding boxes, and the
    /// earliest "before" index among them.  Ready blocks are acked here.
    fn assemble_event(&mut self, fgc: &[bool], px: &[u8], w: u32) -> Option<Fired> {
        let (bw, bh) = (self.bw, self.bh);
        let (wpx, hpx) = (self.det_w as usize, self.det_h as usize);
        let w = w as usize;
        let mut mask = vec![false; wpx * hpx];
        let mut min_born = i64::MAX;
        let mut any = false;
        for by in 0..bh {
            for bx in 0..bw {
                let bi = by * bw + bx;
                let b = &self.blocks[bi];
                if b.acked || b.ready_since < 0 {
                    continue;
                }
                if b.born < min_born {
                    min_born = b.born;
                }
                any = true;
                let y0 = by * BLOCK as usize;
                let x0 = bx * BLOCK as usize;
                for y in y0..(y0 + BLOCK as usize).min(hpx) {
                    for x in x0..(x0 + BLOCK as usize).min(wpx) {
                        if fgc[y * w + x] {
                            mask[y * wpx + x] = true;
                        }
                    }
                }
            }
        }
        if !any {
            return None;
        }
        let regions = connected_regions(&mask, wpx as u32, hpx as u32, min_area(wpx, hpx));
        if regions.is_empty() {
            return None;
        }
        // Ack every block touched by a fired region, fingerprinting its
        // current content so a later removal is detected as a new change.
        for r in &regions {
            for y in r.y..r.y + r.h {
                for x in r.x..r.x + r.w {
                    if y >= hpx as u32 || x >= wpx as u32 {
                        continue;
                    }
                    let (yy, xx) = (y as usize, x as usize);
                    if !mask[yy * wpx + xx] {
                        continue;
                    }
                    let bi = ((y / BLOCK) as usize) * bw + (x / BLOCK) as usize;
                    let b = &mut self.blocks[bi];
                    if b.acked || b.ready_since < 0 {
                        continue;
                    }
                    let mean = block_fg_mean(bi / bw, bi % bw, wpx, hpx, fgc, px, w);
                    b.acked = true;
                    b.clean_count = 0;
                    b.ack = Ack { mean };
                    b.ghost = None;
                    b.ready_since = -1;
                    b.quiet = 0;
                    b.born = -1;
                }
            }
        }
        let before_idx = (min_born - 1).max(0) as u64;
        Some(Fired {
            regions,
            before_idx,
        })
    }
}

fn mean_dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Mean colour of the foreground pixels inside one block (free fn so the
/// per-block loop does not need to hold two borrows of the detector).
fn block_fg_mean(
    by: usize,
    bx: usize,
    det_w: usize,
    det_h: usize,
    fgc: &[bool],
    px: &[u8],
    w: usize,
) -> [f32; 3] {
    let mut sum = [0f64; 3];
    let mut cnt = 0u32;
    let y0 = by * BLOCK as usize;
    let x0 = bx * BLOCK as usize;
    for y in y0..(y0 + BLOCK as usize).min(det_h) {
        for x in x0..(x0 + BLOCK as usize).min(det_w) {
            if fgc[y * w + x] {
                let o = (y * w + x) * 3;
                sum[0] += px[o] as f64;
                sum[1] += px[o + 1] as f64;
                sum[2] += px[o + 2] as f64;
                cnt += 1;
            }
        }
    }
    if cnt == 0 {
        return [0.0; 3];
    }
    [
        (sum[0] / cnt as f64) as f32,
        (sum[1] / cnt as f64) as f32,
        (sum[2] / cnt as f64) as f32,
    ]
}

// ---------------------------------------------------------------------------
// small helpers
// ---------------------------------------------------------------------------

/// Relative area floor (a 1/2500 share of the frame, like the old full-res
/// threshold), with an absolute minimum.
pub(crate) fn min_area(w: usize, h: usize) -> u32 {
    ((w * h) as f64 / 2500.0).max(8.0) as u32
}

/// 4-connected components; returns bounding boxes sorted by area (largest
/// first) for components of at least `min_area` pixels.
pub(crate) fn connected_regions(mask: &[bool], w: u32, h: u32, min_area: u32) -> Vec<DetRect> {
    let n = (w * h) as usize;
    let mut visited = vec![false; n];
    let mut stack: Vec<usize> = Vec::with_capacity(1024);
    let mut out = Vec::new();
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
            minx = minx.min(x);
            miny = miny.min(y);
            maxx = maxx.max(x);
            maxy = maxy.max(y);
            area += 1;
            if x > 0 {
                let j = i - 1;
                if mask[j] && !visited[j] {
                    visited[j] = true;
                    stack.push(j);
                }
            }
            if x + 1 < w {
                let j = i + 1;
                if mask[j] && !visited[j] {
                    visited[j] = true;
                    stack.push(j);
                }
            }
            if y > 0 {
                let j = i - w as usize;
                if mask[j] && !visited[j] {
                    visited[j] = true;
                    stack.push(j);
                }
            }
            if y + 1 < h {
                let j = i + w as usize;
                if mask[j] && !visited[j] {
                    visited[j] = true;
                    stack.push(j);
                }
            }
        }
        if area >= min_area {
            out.push(DetRect {
                x: minx,
                y: miny,
                w: maxx - minx + 1,
                h: maxy - miny + 1,
            });
        }
    }
    out.sort_by_key(|b| std::cmp::Reverse(b.w * b.h));
    out
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    /// Neutral dark room: the background every scene below starts from.
    const BG: [u8; 3] = [60, 60, 60];
    /// A "chair": clearly brighter than BG, and *not* a shadow of it (its hue
    /// differs), so it reads as foreground rather than a lighting change.
    const OBJ: [u8; 3] = [200, 60, 60];
    /// A different object for the "replaced while acked" case.
    const OBJ2: [u8; 3] = [60, 200, 60];

    fn scene(colour: [u8; 3], objects: &[(u32, u32, u32, u32)]) -> RgbImage {
        let mut img = RgbImage::from_pixel(W, 480, Rgb(BG));
        for &(x, y, w, h) in objects {
            for yy in y..(y + h) {
                for xx in x..(x + w) {
                    img.put_pixel(xx, yy, Rgb(colour));
                }
            }
        }
        img
    }

    const PERSIST: u32 = 3;
    /// The chair, mid-frame: 60x60 px, i.e. 4x4 whole blocks.
    const CHAIR: (u32, u32, u32, u32) = (100, 100, 60, 60);

    fn overlaps(r: &DetRect, o: &(u32, u32, u32, u32)) -> bool {
        r.x < o.0 + o.2 && r.x + r.w > o.0 && r.y < o.1 + o.3 && r.y + r.h > o.1
    }

    // ---- pure helpers ------------------------------------------------------

    #[test]
    fn shadow_heuristic_accepts_only_a_darker_version_of_the_reference() {
        // Same hue, 60% of the reference luma: a shadow.
        assert!(is_shadow(&[60, 60, 60], &[100.0, 100.0, 100.0]));
        // Same hue but barely darker (outside the shadow luma band): not a
        // shadow.  A 5% darkening *is* inside the band and is deliberately
        // treated as shadow/JPEG noise.
        assert!(!is_shadow(&[98, 98, 98], &[100.0, 100.0, 100.0]));
        // Same hue, far too dark to be a shadow.
        assert!(!is_shadow(&[5, 5, 5], &[100.0, 100.0, 100.0]));
        // Brighter than the reference is never a shadow.
        assert!(!is_shadow(&[200, 200, 200], &[100.0, 100.0, 100.0]));
        // Dark, but a different hue: an object in shadow, not a shadow.
        assert!(!is_shadow(&[200, 40, 40], &[100.0, 100.0, 100.0]));
    }

    #[test]
    fn min_area_has_a_floor_and_scales_with_the_frame() {
        assert_eq!(min_area(640, 480), 122);
        assert_eq!(min_area(100, 100), 8); // floor
        assert!(min_area(1920, 1080) > min_area(640, 480));
    }

    #[test]
    fn connected_regions_boxes_each_blob_and_drops_the_tiny_ones() {
        let (w, h) = (64u32, 32u32);
        let mut mask = vec![false; (w * h) as usize];
        // blob A: 10x10 at (2,2); blob B: 6x6 at (40,20), diagonally apart.
        for y in 2..12 {
            for x in 2..12 {
                mask[(y * w + x) as usize] = true;
            }
        }
        for y in 20..26 {
            for x in 40..46 {
                mask[(y * w + x) as usize] = true;
            }
        }
        // one stray pixel: below min_area, must not become a region
        mask[(30 * w + 60) as usize] = true;

        let regions = connected_regions(&mask, w, h, 20);
        assert_eq!(regions.len(), 2, "regions: {regions:?}");
        // sorted largest first
        assert_eq!(
            regions[0],
            DetRect {
                x: 2,
                y: 2,
                w: 10,
                h: 10
            }
        );
        assert_eq!(
            regions[1],
            DetRect {
                x: 40,
                y: 20,
                w: 6,
                h: 6
            }
        );
    }

    #[test]
    fn connected_regions_merges_4_connected_pixels_only() {
        // A diagonal line is not 4-connected: every pixel is its own component,
        // so with min_area above 1 nothing is reported.
        let (w, h) = (8u32, 8u32);
        let mut mask = vec![false; (w * h) as usize];
        for i in 0..8usize {
            mask[i * w as usize + i] = true;
        }
        assert!(connected_regions(&mask, w, h, 2).is_empty());
    }

    #[test]
    fn mean_dist_is_euclidean() {
        assert_eq!(mean_dist([0.0, 0.0, 0.0], [3.0, 4.0, 0.0]), 5.0);
        assert_eq!(mean_dist([10.0, 10.0, 10.0], [10.0, 10.0, 10.0]), 0.0);
    }

    // ---- detector behaviour ------------------------------------------------

    #[test]
    fn first_snapshot_seeds_the_background_instead_of_firing() {
        let mut d = Detector::new(W, PERSIST);
        let s = d.step(&scene(OBJ, &[]));
        assert!(s.reset);
        assert!(s.fired.is_empty());
        assert_eq!(d.det_dims(), (W, 480));
    }

    #[test]
    fn a_resolution_change_reseeds() {
        let mut d = Detector::new(W, PERSIST);
        d.step(&scene(OBJ, &[]));
        assert_eq!(d.full_w, W);
        let s = d.step(&RgbImage::from_pixel(320, 240, Rgb(BG)));
        assert!(s.reset);
        // 320x240 upscaled to the 640px analysis width.
        assert_eq!(d.det_dims(), (W, 480));
        assert_eq!(d.full_w, 320);
        assert_eq!(d.full_h, 240);
    }

    #[test]
    fn a_still_scene_never_fires() {
        let mut d = Detector::new(W, PERSIST);
        d.step(&scene(OBJ, &[])); // seeds the background
        for _ in 0..30 {
            let s = d.step(&scene(OBJ, &[]));
            assert!(!s.reset);
            assert!(s.fired.is_empty());
        }
    }

    /// The core promise: an object that appears and then *stays still* is
    /// recorded, exactly once, with a box around it.
    #[test]
    fn a_static_change_fires_once_after_settling() {
        let mut d = Detector::new(W, PERSIST);
        for _ in 0..4 {
            d.step(&scene(OBJ, &[]));
        }

        let mut events = Vec::new();
        for _ in 0..20 {
            let s = d.step(&scene(OBJ, &[CHAIR]));
            assert!(!s.reset);
            events.extend(s.fired);
        }
        assert_eq!(
            events.len(),
            1,
            "expected exactly one event, got {events:?}"
        );
        let ev = &events[0];
        assert_eq!(ev.regions.len(), 1, "regions: {:?}", ev.regions);
        let r = &ev.regions[0];
        assert!(
            overlaps(r, &CHAIR),
            "region {r:?} misses the object {CHAIR:?}"
        );
        // The box should be tight-ish: within a block of the object's edges.
        assert!(r.x >= CHAIR.0 - BLOCK && r.x <= CHAIR.0 + BLOCK);
        assert!(r.y >= CHAIR.1 - BLOCK && r.y <= CHAIR.1 + BLOCK);
        assert!(r.w >= CHAIR.2 && r.w <= CHAIR.2 + 2 * BLOCK);
        assert!(r.h >= CHAIR.3 && r.h <= CHAIR.3 + 2 * BLOCK);
        // "before" is the snapshot just before the change began, i.e. an empty
        // frame rather than one already containing the object.
        assert!(ev.before_idx < d.frame);
    }

    /// The other half of the promise: a person walking through produces
    /// nothing, because the frame never settles.
    #[test]
    fn a_moving_object_never_fires() {
        let mut d = Detector::new(W, PERSIST);
        for _ in 0..4 {
            d.step(&scene(OBJ, &[]));
        }
        for i in 0..40u32 {
            let x = 60 + i * 12; // keeps moving, never leaves the frame
            let s = d.step(&scene(OBJ, &[(x, 100, 60, 60)]));
            assert!(!s.reset);
            assert!(
                s.fired.is_empty(),
                "frame {i}: moving object fired {fired:?} at x={x}",
                fired = s.fired
            );
        }
    }

    /// A change that covers the whole frame is a light change / camera reset,
    /// not an event: the background is re-seeded instead.
    #[test]
    fn a_whole_scene_change_reseeds_rather_than_firing() {
        let mut d = Detector::new(W, PERSIST);
        for _ in 0..4 {
            d.step(&scene(OBJ, &[]));
        }
        // Every pixel moves by 140 => far past GLOBAL_RESET_FRAC.
        let bright = RgbImage::from_pixel(W, 480, Rgb([200, 200, 200]));
        let s = d.step(&bright);
        assert!(s.reset, "a whole-frame difference must re-seed");
        assert!(s.fired.is_empty(), "...and must not fire: {s:?}");
        // The new lighting becomes the background: the next frame is calm.
        let s2 = d.step(&bright);
        assert!(!s2.reset && s2.fired.is_empty());
    }

    /// Recorded objects are absorbed into the background, so swapping them for
    /// something else is a fresh change and fires again.
    #[test]
    fn replacing_a_recorded_object_fires_again() {
        let mut d = Detector::new(W, PERSIST);
        for _ in 0..4 {
            d.step(&scene(OBJ, &[]));
        }
        let mut first = Vec::new();
        for _ in 0..20 {
            first.extend(d.step(&scene(OBJ, &[CHAIR])).fired);
        }
        assert_eq!(first.len(), 1, "setup: expected one event");

        // Let the object settle into the background (ACK_ALPHA absorbs it).
        for _ in 0..40 {
            let s = d.step(&scene(OBJ, &[CHAIR]));
            assert!(s.fired.is_empty(), "the object sat still and re-fired");
        }

        // Swap it for a different object in the same spot.
        let mut second = Vec::new();
        for _ in 0..25 {
            second.extend(d.step(&scene(OBJ2, &[CHAIR])).fired);
        }
        assert_eq!(second.len(), 1, "expected one replacement event");
        assert!(
            overlaps(&second[0].regions[0], &CHAIR),
            "replacement region misses the object: {:?}",
            second[0].regions
        );
    }

    /// ... and taking a recorded object away is caught too: the background has
    /// absorbed the object's colour, so its absence is a difference.
    #[test]
    fn removing_a_recorded_object_fires() {
        let mut d = Detector::new(W, PERSIST);
        for _ in 0..4 {
            d.step(&scene(OBJ, &[]));
        }
        let mut first = Vec::new();
        for _ in 0..20 {
            first.extend(d.step(&scene(OBJ, &[CHAIR])).fired);
        }
        assert_eq!(first.len(), 1, "setup: expected one event");
        for _ in 0..40 {
            d.step(&scene(OBJ, &[CHAIR]));
        }

        let mut second = Vec::new();
        for _ in 0..25 {
            second.extend(d.step(&scene(OBJ, &[])).fired);
        }
        assert_eq!(second.len(), 1, "expected one removal event");
        assert!(
            overlaps(&second[0].regions[0], &CHAIR),
            "removal region misses the object: {:?}",
            second[0].regions
        );
    }
}
