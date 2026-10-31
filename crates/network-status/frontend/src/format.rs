//! Number, duration and time formatting — the display half of the old `format.ts`, same
//! tiers, because a rate read as "1.2 MB/s" and one read as "1200000" are different
//! questions.

use js_sys::Date;

pub fn fmt_rate(bps: Option<f64>) -> String {
    let Some(v) = bps else { return "–".to_string() };
    if v >= 1e9 {
        format!("{:.2} GB/s", v / 1e9)
    } else if v >= 1e6 {
        format!("{:.2} MB/s", v / 1e6)
    } else if v >= 1e3 {
        format!("{:.1} KB/s", v / 1e3)
    } else {
        format!("{} B/s", v.round() as i64)
    }
}

pub fn fmt_total(b: Option<u64>) -> String {
    let Some(v) = b else { return "–".to_string() };
    let v = v as f64;
    if v >= 1e12 {
        format!("{:.2} TB", v / 1e12)
    } else if v >= 1e9 {
        format!("{:.2} GB", v / 1e9)
    } else if v >= 1e6 {
        format!("{:.1} MB", v / 1e6)
    } else if v >= 1e3 {
        format!("{:.1} KB", v / 1e3)
    } else {
        format!("{v:.0} B")
    }
}

pub fn fmt_dur(s: Option<i64>) -> String {
    let Some(v) = s else { return "–".to_string() };
    let d = v / 86400;
    let h = (v % 86400) / 3600;
    let m = (v % 3600) / 60;
    if d > 0 {
        return format!("{d}d {h}h {m}m");
    }
    if h > 0 {
        return format!("{h}h {m}m");
    }
    format!("{m}m {}s", v % 60)
}

pub fn fmt_mem(b: Option<u64>) -> String {
    let Some(v) = b else { return "–".to_string() };
    let v = v as f64;
    if v >= 1e9 {
        format!("{:.2} GB", v / 1e9)
    } else if v >= 1e6 {
        format!("{:.0} MB", v / 1e6)
    } else {
        format!("{:.0} KB", v / 1e3)
    }
}

fn pad2(n: u32) -> String {
    if n < 10 {
        format!("0{n}")
    } else {
        n.to_string()
    }
}

/// Wall-clock time of a unix timestamp, in the browser's zone, as the old `fmtTime` did:
/// the lease table's "Expires" column is a time of day, not a duration, and it has to read
/// as the clock on the wall.
pub fn fmt_time(ts: i64) -> String {
    let d = Date::new(&wasm_bindgen::JsValue::from_f64(ts as f64 * 1000.0));
    format!(
        "{}:{}:{}",
        pad2(d.get_hours()),
        pad2(d.get_minutes()),
        pad2(d.get_seconds())
    )
}

/// `load 0.12 / 0.34 / 0.51 on 4 CPUs` — the load chart's legend row.
pub fn fmt_list_load(load: &[f64], ncpu: Option<i64>) -> String {
    if load.is_empty() {
        return "–".to_string();
    }
    let parts = load
        .iter()
        .map(|l| format!("{l:.2}"))
        .collect::<Vec<_>>()
        .join(" / ");
    match ncpu {
        Some(n) => format!("load {parts} on {n} CPUs"),
        None => format!("load {parts}"),
    }
}
