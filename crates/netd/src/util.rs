//! The two things every module here needs and none of them should re-invent.

use std::time::{SystemTime, UNIX_EPOCH};

/// Unix seconds. The daemon has no clock abstraction: leases, cache entries and
/// the API all speak in absolute unix time, because that is what an operator
/// comparing two machines needs and a monotonic counter cannot give.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `3d 4h 5m 2s`, for `/v1/status` and the journal.
pub fn human_duration(secs: i64) -> String {
    let s = secs.max(0);
    let (d, s) = (s / 86_400, s % 86_400);
    let (h, s) = (s / 3_600, s % 3_600);
    let (m, s) = (s / 60, s % 60);
    if d > 0 {
        format!("{d}d {h}h {m}m {s}s")
    } else if h > 0 {
        format!("{h}h {m}m {s}s")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

/// A right-aligned two-column table, for `netd check` and `netd explain`.
/// Plain text on purpose: it is read in a terminal and pasted into an issue.
pub fn table(rows: &[(String, String)]) -> String {
    let width = rows.iter().map(|(a, _)| a.len()).max().unwrap_or(0);
    rows.iter()
        .map(|(a, b)| {
            if b.is_empty() {
                a.clone()
            } else {
                format!("{a:<width$}  {b}", width = width)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_the_way_a_person_writes_them() {
        assert_eq!(human_duration(0), "0s");
        assert_eq!(human_duration(61), "1m 1s");
        assert_eq!(human_duration(3_723), "1h 2m 3s");
        assert_eq!(human_duration(86_400 + 3_723), "1d 1h 2m 3s");
        assert_eq!(human_duration(-5), "0s");
    }

    #[test]
    fn the_table_lines_its_columns_up() {
        let out = table(&[
            ("zone".into(), "int.example.org".into()),
            ("listen".into(), "0.0.0.0:53".into()),
        ]);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].find("int"), lines[1].find("0.0"));
    }
}
