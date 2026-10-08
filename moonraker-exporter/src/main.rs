//! moonraker-exporter — small Prometheus exporter for the hackspace 3D-print
//! servers (3d-blue / 3d-lime: Raspberry Pi 3 + Klipper + Moonraker).
//!
//! Moonraker (port 7125) has no `/metrics` endpoint, so this tool polls each
//! printer's Moonraker HTTP API and re-exports the interesting bits in the
//! Prometheus text format for the local Prometheus on services1 to scrape.
//!
//! Zero external crates (house style, see dns-sync/status-dashboard). All
//! Moonraker traffic is plain HTTP on the LAN, so requests go over
//! `std::net::TcpStream` directly (no TLS, no curl). JSON is parsed with the
//! shared hand-rolled parser in `common-rs/json`.
//!
//! Usage:
//!
//! ```text
//! moonraker-exporter [--listen 127.0.0.1:9701] --printer NAME=BASE_URL ...
//! ```
//!
//! `BASE_URL` is the Moonraker origin, e.g. `http://10.3.14.62:7125`. The
//! targets are fixed IPs on purpose so the exporter never depends on DNS: the
//! two Pis were cloned from one SD card, giving them a shared
//! `/etc/machine-id` and DHCPv6 DUID, so the `3d-*` names advertised each
//! other's IPv6 addresses (lime's identity was regenerated 2026-10-03).
//!
//! State enums: the numeric value of `moonraker_klippy_state` /
//! `moonraker_print_state` is the index of the label into `KLIPPY_STATES` /
//! `PRINT_STATES` below (the label is also carried as a `state=` tag).  The
//! Grafana value mappings and the Prometheus alert rules are *generated* from
//! these two arrays — machines/services1/lib/printer-states.nix reads them out
//! of this file at eval time — so the exporter and the dashboards cannot drift.
//! If you reorder or add to an array, the alert rule numbers and the dashboard
//! labels follow automatically; if the arrays stop parsing, evaluation fails
//! loudly (`just check`).

use common_json as json;

use std::env;
use std::fmt::Display;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::time::Duration;

use json::Json;

const TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_LISTEN: &str = "127.0.0.1:9701";

/// One printer: `name` is the Prometheus `printer` label (blue/lime);
/// `hostport` is `host:port` of its Moonraker API.
struct Printer {
    name: String,
    hostport: String,
}

fn main() {
    let mut listen = DEFAULT_LISTEN.to_string();
    let mut printers: Vec<Printer> = Vec::new();

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--listen" => match args.next() {
                Some(v) => listen = v,
                None => usage("--listen needs a value"),
            },
            "--printer" => match args.next() {
                Some(v) => match v.split_once('=') {
                    Some((name, url)) => {
                        let hostport = url.strip_prefix("http://").unwrap_or(url).to_string();
                        printers.push(Printer {
                            name: name.to_string(),
                            hostport,
                        });
                    }
                    None => usage("--printer expects NAME=http://host:port"),
                },
                None => usage("--printer needs a value"),
            },
            "--help" | "-h" => usage(""),
            other => usage(&format!("unknown argument: {other}")),
        }
    }

    if printers.is_empty() {
        usage("at least one --printer is required");
    }

    let listener = TcpListener::bind(&listen).unwrap_or_else(|e| {
        eprintln!("cannot bind {listen}: {e}");
        std::process::exit(1);
    });

    for conn in listener.incoming() {
        match conn {
            Ok(mut stream) => {
                if let Err(e) = handle(&mut stream, &printers) {
                    eprintln!("request handling error: {e}");
                }
            }
            Err(e) => eprintln!("accept error: {e}"),
        }
    }
}

/// Serve one HTTP request: only `GET /metrics` is supported.
fn handle(stream: &mut TcpStream, printers: &[Printer]) -> Result<(), String> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));

    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n".as_slice()) {
                    break;
                }
            }
            Err(_) => break,
        }
        if buf.len() > 16_384 {
            break;
        }
    }

    let head = String::from_utf8_lossy(&buf);
    let request_line = head.lines().next().unwrap_or("");
    let parts: Vec<&str> = request_line.split_whitespace().collect();

    let (status, body) = if parts.len() >= 2 && parts[0] == "GET" && parts[1] == "/metrics" {
        ("200 OK", render(printers))
    } else {
        ("404 Not Found", "not found\n".to_string())
    };

    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; version=0.0.4; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(resp.as_bytes()).map_err(|e| e.to_string())
}

/// Fetch the Moonraker API endpoint `path` on `printer` and parse it as JSON.
fn api(printer: &Printer, path: &str) -> Option<Json> {
    let hostport = &printer.hostport;
    let (host, port) = hostport.split_once(':')?;
    let port: u16 = port.parse().ok()?;

    let mut stream: Option<TcpStream> = None;
    for addr in (host, port).to_socket_addrs().ok()? {
        if let Ok(s) = TcpStream::connect_timeout(&addr, TIMEOUT) {
            stream = Some(s);
            break;
        }
    }
    let mut s = stream?;
    let _ = s.set_read_timeout(Some(TIMEOUT));

    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {hostport}\r\nUser-Agent: moonraker-exporter/0.1\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
    );
    s.write_all(req.as_bytes()).ok()?;

    let mut raw = Vec::new();
    s.read_to_end(&mut raw).ok()?;
    let text = String::from_utf8_lossy(&raw);
    let body = text.split("\r\n\r\n").nth(1)?;
    json::parse(body).ok()
}

/// Escapes a string for use inside a Prometheus label value.
fn esc(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// Append one metric line: `name{printer="<printer>"[,extra]} <value>`.
fn emit(out: &mut Vec<String>, printer: &str, name: &str, extra: &str, value: impl Display) {
    let extra = if extra.is_empty() {
        String::new()
    } else {
        format!(",{extra}")
    };
    out.push(format!("{name}{{printer=\"{printer}\"{extra}}} {value}"));
}

/// klippy states, in the order Moonraker reports them.  Index == metric value.
/// Parsed out of this file by machines/services1/lib/printer-states.nix: keep
/// the literal `&["a", "b"]` shape on one line and keep the name.
const KLIPPY_STATES: &[&str] = &["startup", "ready", "error", "shutdown", "disconnected"];

/// print_stats states, in the order Moonraker reports them.  Index == metric
/// value.  Same parsing contract as KLIPPY_STATES.
const PRINT_STATES: &[&str] = &[
    "standby",
    "printing",
    "paused",
    "complete",
    "cancelled",
    "error",
];

/// The enum value for a state label: its position in the table, or 99 for a
/// state Moonraker has started reporting that this exporter does not know
/// (a new upstream state then shows up as 99 rather than silently colliding
/// with an existing one).
fn state_code(states: &[&str], state: &str) -> i64 {
    match states.iter().position(|s| *s == state) {
        Some(i) => i as i64,
        None => 99,
    }
}

/// Scrape one printer, appending its metric lines to `out`.
fn scrape(printer: &Printer, out: &mut Vec<String>) {
    let p = &printer.name;

    // /server/info — reachability + klippy connection state.
    let info = api(printer, "/server/info");
    let Some(info) = info else {
        emit(out, p, "moonraker_up", "", 0);
        return;
    };
    emit(out, p, "moonraker_up", "", 1);

    let klippy_state = info
        .at(&["result", "klippy_state"])
        .and_then(Json::as_str)
        .unwrap_or("unknown");
    emit(
        out,
        p,
        "moonraker_klippy_state",
        &format!("state=\"{}\"", esc(klippy_state)),
        state_code(KLIPPY_STATES, klippy_state),
    );
    emit(
        out,
        p,
        "moonraker_klippy_connected",
        "",
        if info
            .at(&["result", "klippy_connected"])
            .and_then(Json::as_bool)
            == Some(true)
        {
            1
        } else {
            0
        },
    );
    if let Some(v) = info
        .at(&["result", "moonraker_version"])
        .and_then(Json::as_str)
    {
        emit(
            out,
            p,
            "moonraker_info",
            &format!("moonraker_version=\"{}\"", esc(v)),
            1,
        );
    }

    // /printer/objects/query — temps + current print state. Works even when
    // klippy is in its error state (returns last-known object values).
    if let Some(query) = api(
        printer,
        "/printer/objects/query?extruder&heater_bed&print_stats&mcu",
    ) {
        let status = query.at(&["result", "status"]);
        if let Some(status) = status {
            for heater in ["extruder", "heater_bed"] {
                let Some(obj) = status.get(heater) else {
                    continue;
                };
                let label = format!("heater=\"{heater}\"");
                if let Some(t) = obj.get("temperature").and_then(Json::as_f64) {
                    emit(out, p, "moonraker_heater_temperature", &label, t);
                }
                if let Some(t) = obj.get("target").and_then(Json::as_f64) {
                    emit(out, p, "moonraker_heater_target", &label, t);
                }
            }

            if let Some(ps) = status.get("print_stats") {
                if let Some(state) = ps.get("state").and_then(Json::as_str) {
                    emit(
                        out,
                        p,
                        "moonraker_print_state",
                        &format!("state=\"{}\"", esc(state)),
                        state_code(PRINT_STATES, state),
                    );
                }
                if let Some(file) = ps.get("filename").and_then(Json::as_str) {
                    if !file.is_empty() {
                        emit(
                            out,
                            p,
                            "moonraker_print_file",
                            &format!("file=\"{}\"", esc(file)),
                            1,
                        );
                    }
                }
                if let Some(v) = ps.get("progress").and_then(Json::as_f64) {
                    emit(out, p, "moonraker_print_progress", "", v);
                }
                if let Some(v) = ps.get("print_duration").and_then(Json::as_f64) {
                    emit(out, p, "moonraker_print_duration_seconds", "", v);
                }
                if let Some(v) = ps.get("filament_used").and_then(Json::as_f64) {
                    emit(out, p, "moonraker_print_filament_used_mm", "", v);
                }
                if let Some(layer) = ps.at(&["info", "current_layer"]).and_then(Json::as_f64) {
                    emit(out, p, "moonraker_print_current_layer", "", layer);
                }
                if let Some(total) = ps.at(&["info", "total_layer"]).and_then(Json::as_f64) {
                    emit(out, p, "moonraker_print_total_layers", "", total);
                }
            }

            // MCU link health. `mcu.last_stats` holds the counters for the
            // running Klipper session. bytes_retransmit / bytes_invalid are
            // the early-warning signal for the flaky CH340 USB-serial link
            // (docs/3d-blue-klipper-log-review-2026-10-03.md); exported as
            // cumulative counters so Prometheus can rate() them and alert
            // before a print is aborted with "Lost communication with MCU".
            match status.get("mcu") {
                Some(mcu) => {
                    if let Some(baud) = mcu
                        .at(&["mcu_constants", "SERIAL_BAUD"])
                        .and_then(Json::as_u64)
                    {
                        emit(out, p, "moonraker_mcu_info", &format!("baud=\"{baud}\""), 1);
                    }
                    match mcu.get("last_stats") {
                        Some(ls) => {
                            emit(out, p, "moonraker_mcu_connected", "", 1);
                            for (field, name) in [
                                ("bytes_write", "moonraker_mcu_bytes_write_total"),
                                ("bytes_read", "moonraker_mcu_bytes_read_total"),
                                ("bytes_retransmit", "moonraker_mcu_bytes_retransmit_total"),
                                ("bytes_invalid", "moonraker_mcu_bytes_invalid_total"),
                                ("send_seq", "moonraker_mcu_send_seq_total"),
                                ("receive_seq", "moonraker_mcu_receive_seq_total"),
                            ] {
                                if let Some(v) = ls.get(field).and_then(Json::as_f64) {
                                    emit(out, p, name, "", v);
                                }
                            }
                            for (field, name) in [
                                ("srtt", "moonraker_mcu_srtt_seconds"),
                                ("rto", "moonraker_mcu_rto_seconds"),
                            ] {
                                if let Some(v) = ls.get(field).and_then(Json::as_f64) {
                                    emit(out, p, name, "", v);
                                }
                            }
                        }
                        None => emit(out, p, "moonraker_mcu_connected", "", 0),
                    }
                }
                None => emit(out, p, "moonraker_mcu_connected", "", 0),
            }
        }
    }

    // /machine/proc_stats — whole-Pi system stats plus the Moonraker
    // process's own cpu/memory (latest sample).
    if let Some(ps) = api(printer, "/machine/proc_stats") {
        let result = ps.get("result");
        if let Some(result) = result {
            // Whole-system CPU (percent, 0-100).
            if let Some(cpu) = result
                .get("system_cpu_usage")
                .and_then(|c| c.get("cpu"))
                .and_then(Json::as_f64)
            {
                emit(out, p, "moonraker_host_cpu_percent", "", cpu);
            }
            // Whole-system memory (Moonraker reports it in kB).
            if let Some(mem) = result.get("system_memory") {
                if let Some(v) = mem.get("total").and_then(Json::as_f64) {
                    emit(out, p, "moonraker_host_memory_total_bytes", "", v * 1024.0);
                }
                if let Some(v) = mem.get("used").and_then(Json::as_f64) {
                    emit(out, p, "moonraker_host_memory_used_bytes", "", v * 1024.0);
                }
                if let Some(v) = mem.get("available").and_then(Json::as_f64) {
                    emit(
                        out,
                        p,
                        "moonraker_host_memory_available_bytes",
                        "",
                        v * 1024.0,
                    );
                }
            }
            // Raspberry Pi SoC temperature (whole-board heat).
            if let Some(t) = result.get("cpu_temp").and_then(Json::as_f64) {
                emit(out, p, "moonraker_cpu_temperature_celsius", "", t);
            }
        }
        // Moonraker process cpu/memory (latest sample).
        let samples = ps
            .at(&["result", "moonraker_stats"])
            .and_then(Json::as_array);
        if let Some(samples) = samples {
            if let Some(last) = samples.last() {
                if let Some(cpu) = last.get("cpu_usage").and_then(Json::as_f64) {
                    emit(out, p, "moonraker_process_cpu_percent", "", cpu);
                }
                if let Some(mem) = last.get("memory").and_then(Json::as_f64) {
                    emit(out, p, "moonraker_process_memory_bytes", "", mem * 1024.0);
                }
            }
        }
    }

    // /machine/system_info — static host identity (once per scrape is fine).
    if let Some(sys) = api(printer, "/machine/system_info") {
        let cpu = sys.at(&["result", "system_info", "cpu_info"]);
        if let Some(cpu) = cpu {
            let model = cpu.get("model").and_then(Json::as_str).unwrap_or("unknown");
            let serial = cpu
                .get("serial_number")
                .and_then(Json::as_str)
                .unwrap_or("unknown");
            emit(
                out,
                p,
                "moonraker_host_info",
                &format!("model=\"{}\",serial=\"{}\"", esc(model), esc(serial)),
                1,
            );
            // Total RAM already comes from proc_stats' system_memory.
        }
    }
}

fn render(printers: &[Printer]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for p in printers {
        scrape(p, &mut lines);
    }
    lines.push("".to_string());
    lines.join("\n")
}

fn usage(msg: &str) -> ! {
    if !msg.is_empty() {
        eprintln!("error: {msg}");
    }
    eprintln!(
        "usage: moonraker-exporter [--listen 127.0.0.1:9701] --printer NAME=http://host:port ..."
    );
    std::process::exit(2);
}

#[cfg(test)]
mod tests {
    use super::*;

    // The numbers here are what the deployed Grafana dashboards and Prometheus
    // alert rules were built with.  They are generated from the tables (see
    // machines/services1/lib/printer-states.nix), so this test exists only to
    // catch an accidental reorder/relabel of a table: that would silently
    // renumber every panel and alert rule.
    #[test]
    fn klippy_codes_are_the_table_positions() {
        let expected = [
            ("startup", 0),
            ("ready", 1),
            ("error", 2),
            ("shutdown", 3),
            ("disconnected", 4),
        ];
        for (label, code) in expected {
            assert_eq!(state_code(KLIPPY_STATES, label), code, "klippy {label}");
        }
        assert_eq!(KLIPPY_STATES.len(), expected.len());
    }

    #[test]
    fn print_codes_are_the_table_positions() {
        let expected = [
            ("standby", 0),
            ("printing", 1),
            ("paused", 2),
            ("complete", 3),
            ("cancelled", 4),
            ("error", 5),
        ];
        for (label, code) in expected {
            assert_eq!(state_code(PRINT_STATES, label), code, "print {label}");
        }
        assert_eq!(PRINT_STATES.len(), expected.len());
    }

    // A state Moonraker learns about must not collide with a known one.
    #[test]
    fn unknown_state_codes_to_99() {
        assert_eq!(state_code(KLIPPY_STATES, "restarting"), 99);
        assert_eq!(state_code(PRINT_STATES, "cancelled_by_operator"), 99);
    }

    // A duplicate label would make two states share a metric value.
    #[test]
    fn tables_have_no_duplicate_labels() {
        for table in [KLIPPY_STATES, PRINT_STATES] {
            for (i, a) in table.iter().enumerate() {
                for b in &table[i + 1..] {
                    assert_ne!(a, b, "duplicate label in state table: {a}");
                }
            }
        }
    }
}
