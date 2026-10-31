//! Canvas charts — the hand-rolled, dependency-free drawing the old `chart.ts` did, kept
//! on purpose. There is no chart library here: the window is a fixed number of points and
//! the series are polylines.
//!
//! Two rules are easy to lose in a rewrite and are the reason this file exists:
//!
//! - **`nice_max` is head-rounded** (1/2/5 × 10ⁿ), so the axis gets at most seven
//!   gridlines whatever the peak is. The gridlines themselves are five, drawn at quarters
//!   of that rounded peak.
//! - **A `null` in a series is a gap, not a zero.** The path breaks there rather than
//!   drawing through it, because "no sample for this point" and "the rate was zero" are
//!   different facts and the reader cannot tell which one was drawn.
//!
//! Text drawn here is canvas text, not markup: the device-supplied strings the panels show
//! (hostname, MAC, client-id) cannot inject anything through a `fill_text` call. That is a
//! change from the old `panels.tsx`/`leases.tsx`, which built HTML strings and needed an
//! `esc()` for exactly that reason; in Dioxus the vnode layer escapes text nodes, so no
//! `esc` exists in this port.

use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

use crate::format::fmt_time;

pub const COLOR_UP: &str = "#2dd4bf";
pub const COLOR_DOWN: &str = "#60a5fa";

pub struct Plot<'a> {
    pub data: &'a [Option<f64>],
    pub color: &'a str,
    pub fill: bool,
    /// Points from the right end to draw. The old default was the whole series.
    pub window: usize,
    pub width: f64,
}

impl<'a> Plot<'a> {
    pub fn new(data: &'a [Option<f64>], color: &'a str) -> Self {
        Self { data, color, fill: false, window: data.len(), width: 1.8 }
    }

    pub fn filled(mut self, fill: bool) -> Self {
        self.fill = fill;
        self
    }

    pub fn window(mut self, w: usize) -> Self {
        self.window = w;
        self
    }
}

/// Look the canvas up by id. The components do this from an effect after the element has
/// mounted — see the note in `charts.rs` about why the draw waits for `onmounted`.
pub fn canvas(id: &str) -> Option<HtmlCanvasElement> {
    web_sys::window()?
        .document()?
        .get_element_by_id(id)?
        .dyn_into::<HtmlCanvasElement>()
        .ok()
}

fn setup(canvas: &HtmlCanvasElement, css_height: f64) -> Option<(CanvasRenderingContext2d, f64, f64)> {
    let dpr = web_sys::window()?.device_pixel_ratio();
    let w = (canvas.client_width() as f64).max(300.0);
    canvas.set_width(((w * dpr) as u32).max(1));
    canvas.set_height(((css_height * dpr) as u32).max(1));
    let ctx = canvas
        .get_context("2d")
        .ok()?
        .as_ref()
        .and_then(|o| Some(o.dyn_ref::<CanvasRenderingContext2d>()?.clone()))?;
    let _ = ctx.set_transform(dpr, 0.0, 0.0, dpr, 0.0, 0.0);
    Some((ctx, w, css_height))
}

pub fn nice_max(v: f64) -> f64 {
    if v <= 0.0 {
        return 1.0;
    }
    let exp = 10f64.powi(v.log10().floor() as i32);
    let m = v / exp;
    if m <= 1.0 {
        exp
    } else if m <= 2.0 {
        2.0 * exp
    } else if m <= 5.0 {
        5.0 * exp
    } else {
        10.0 * exp
    }
}

/// The full chart: gridlines with labels, three x labels, then fill, then the line.
///
/// `#[allow(deprecated)]` for one line: web-sys deprecated the JsValue setters in favour of
/// the `_str` forms, and a `_str` form cannot take a gradient — the fade under the line is a
/// gradient, so that one call is the deprecated one. Everything else here uses `_str`.
#[allow(deprecated)]
pub fn draw(
    canvas: &HtmlCanvasElement,
    plots: &[Plot],
    fmt: fn(f64) -> String,
    height: f64,
    ts: &[i64],
) {
    let Some((ctx, w, h)) = setup(canvas, height) else {
        return;
    };
    let (pad_l, pad_r, pad_t, pad_b) = (58.0, 8.0, 8.0, 20.0);
    let pw = w - pad_l - pad_r;
    let ph = h - pad_t - pad_b;

    let mut maxv: f64 = 0.0;
    for p in plots {
        let start = p.data.len().saturating_sub(p.window);
        for &v in &p.data[start..] {
            if let Some(v) = v {
                maxv = maxv.max(v);
            }
        }
    }
    let ymax = nice_max(maxv * 1.05);
    let n = plots.first().map(|p| p.data.len()).unwrap_or(0);

    ctx.clear_rect(0.0, 0.0, w, h);
    ctx.set_font("11px ui-monospace, SFMono-Regular, Menlo, monospace");
    ctx.set_stroke_style_str("#232d38");
    ctx.set_fill_style_str("#8b98a5");
    ctx.set_line_width(1.0);

    for g in 0..=4 {
        let y = pad_t + ph - (ph * g as f64) / 4.0;
        ctx.begin_path();
        ctx.move_to(pad_l, y);
        ctx.line_to(w - pad_r, y);
        ctx.stroke();
        ctx.fill_text(&fmt((ymax * g as f64) / 4.0), 4.0, y + 4.0).ok();
    }

    // X labels. A series can be shorter than `ts` — an interface that appeared later in
    // the history — so the offset into the timestamp array is explicit, as it was.
    if n > 1 && !ts.is_empty() {
        let off = ts.len() - n;
        for i in [0, (n - 1) / 2, n - 1] {
            let x = pad_l + (pw * i as f64) / (n - 1) as f64;
            ctx.set_text_align(if i == n - 1 {
                "right"
            } else if i == (n - 1) / 2 {
                "center"
            } else {
                "left"
            });
            ctx.fill_text(&fmt_time(ts[off + i]), x, h - 5.0).ok();
        }
        ctx.set_text_align("left");
    }

    if n == 0 {
        ctx.fill_text("no data yet", pad_l + 8.0, pad_t + ph / 2.0).ok();
        return;
    }

    for p in plots {
        let start = p.data.len().saturating_sub(p.window);
        let count = p.data.len() - start;
        if count < 2 {
            continue;
        }
        let x_at = |i: usize| pad_l + (pw * (i - start) as f64) / (count - 1) as f64;
        let y_at = |v: f64| pad_t + ph - (ph * v.min(ymax)) / ymax;

        if p.fill {
            ctx.begin_path();
            let mut started = false;
            let mut first_x: Option<f64> = None;
            let mut last_x: Option<f64> = None;
            for i in start..p.data.len() {
                let Some(v) = p.data[i] else {
                    started = false;
                    continue;
                };
                let (x, y) = (x_at(i), y_at(v));
                if started {
                    ctx.line_to(x, y);
                } else {
                    ctx.move_to(x, y);
                    started = true;
                }
                first_x = first_x.or(Some(x));
                last_x = Some(x);
            }
            if let (Some(f), Some(l)) = (first_x, last_x) {
                ctx.line_to(l, pad_t + ph);
                ctx.line_to(f, pad_t + ph);
                ctx.close_path();
                let g = ctx.create_linear_gradient(0.0, pad_t, 0.0, pad_t + ph);
                g.add_color_stop(0.0, &rgba(p.color, 0.30)).ok();
                g.add_color_stop(1.0, &rgba(p.color, 0.05)).ok();
                ctx.set_fill_style(&wasm_bindgen::JsValue::from(&g));
                ctx.fill();
            }
        }

        ctx.begin_path();
        let mut started = false;
        for i in start..p.data.len() {
            let Some(v) = p.data[i] else {
                started = false;
                continue;
            };
            let (x, y) = (x_at(i), y_at(v));
            if started {
                ctx.line_to(x, y);
            } else {
                ctx.move_to(x, y);
                started = true;
            }
        }
        ctx.set_stroke_style_str(p.color);
        ctx.set_line_width(p.width);
        ctx.set_line_join("round");
        ctx.stroke();
    }
}

/// Sparkline at card size: no labels, no gridlines, just the line.
pub fn sparkline(canvas: &HtmlCanvasElement, plots: &[Plot], window: usize) {
    let Some((ctx, w, h)) = setup(canvas, 46.0) else {
        return;
    };
    ctx.clear_rect(0.0, 0.0, w, h);
    let n = plots.first().map(|p| p.data.len()).unwrap_or(0);
    let start = n.saturating_sub(window);
    let count = n - start;
    if count < 2 {
        ctx.set_fill_style_str("#4a5560");
        ctx.set_font("10px ui-monospace, monospace");
        ctx.fill_text("…", 4.0, h / 2.0 + 3.0).ok();
        return;
    }

    let mut maxv: f64 = 0.0;
    for p in plots {
        for &v in &p.data[start..] {
            if let Some(v) = v {
                maxv = maxv.max(v);
            }
        }
    }
    let ymax = nice_max(maxv * 1.1).max(1.0);
    let x_at = |i: usize| (w * (i - start) as f64) / (count - 1) as f64;
    let y_at = |v: f64| h - 2.0 - ((h - 6.0) * v.min(ymax)) / ymax;

    for p in plots {
        ctx.begin_path();
        let mut started = false;
        for i in start..n {
            let Some(v) = p.data[i] else {
                started = false;
                continue;
            };
            let (x, y) = (x_at(i), y_at(v));
            if started {
                ctx.line_to(x, y);
            } else {
                ctx.move_to(x, y);
                started = true;
            }
        }
        ctx.set_stroke_style_str(p.color);
        ctx.set_line_width(1.5);
        ctx.stroke();
    }
}

/// `#rrggbb` + alpha, as the original did.
fn rgba(hex: &str, a: f64) -> String {
    let r = hex.get(1..3).unwrap_or("00").parse::<u8>().unwrap_or(0);
    let g = hex.get(3..5).unwrap_or("00").parse::<u8>().unwrap_or(0);
    let b = hex.get(5..7).unwrap_or("00").parse::<u8>().unwrap_or(0);
    format!("rgba({r}, {g}, {b}, {a:.2})")
}
