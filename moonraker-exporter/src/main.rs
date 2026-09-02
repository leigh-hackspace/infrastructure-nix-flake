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
//! tiny hand-rolled parser in `json.rs`.
//!
//! Usage:
//!
//! ```text
//! moonraker-exporter [--listen 127.0.0.1:9701] --printer NAME=BASE_URL ...
//! ```
//!
//! `BASE_URL` is the Moonraker origin, e.g. `http://10.3.14.62:7125`. The
//! targets are fixed IPs on purpose: the `3d-lime.int.leighhack.org` DNS name
//! also advertises 3d-blue's IPv6 addresses, so hostnames must not be used.
//!
//! State enums (numeric value carries the label; dashboards map number ->
//! text, keep these tables in sync with monitoring-dashboards.nix):
//!
//! ```text
//! klippy:   0 startup, 1 ready, 2 error, 3 shutdown, 4 disconnected
//! print:    0 standby, 1 printing, 2 paused, 3 complete, 4 cancelled, 5 error
//! ```

mod json;

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
                        let hostport = url
                            .strip_prefix("http://")
                            .unwrap_or(&url)
                            .to_string();
                        printers.push(Printer { name: name.to_string(), hostport });
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

    let listener = TcpListener::bind(&listen)
        .unwrap_or_else(|e| {
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

fn klippy_code(state: &str) -> i64 {
    match state {
        "startup" => 0,
        "ready" => 1,
        "error" => 2,
        "shutdown" => 3,
        "disconnected" => 4,
        _ => 99,
    }
}

fn print_code(state: &str) -> i64 {
    match state {
        "standby" => 0,
        "printing" => 1,
        "paused" => 2,
        "complete" => 3,
        "cancelled" => 4,
        "error" => 5,
        _ => 99,
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
        klippy_code(klippy_state),
    );
    emit(
        out,
        p,
        "moonraker_klippy_connected",
        "",
        if info.at(&["result", "klippy_connected"]).and_then(Json::as_bool) == Some(true) {
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
    if let Some(query) = api(printer, "/printer/objects/query?extruder&heater_bed&print_stats") {
        let status = query.at(&["result", "status"]);
        if let Some(status) = status {
            for heater in ["extruder", "heater_bed"] {
                let Some(obj) = status.get(heater) else { continue };
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
                        print_code(state),
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
                    emit(out, p, "moonraker_host_memory_available_bytes", "", v * 1024.0);
                }
            }
            // Raspberry Pi SoC temperature (whole-board heat).
            if let Some(t) = result.get("cpu_temp").and_then(Json::as_f64) {
                emit(out, p, "moonraker_cpu_temperature_celsius", "", t);
            }
        }
        // Moonraker process cpu/memory (latest sample).
        let samples = ps.at(&["result", "moonraker_stats"]).and_then(Json::as_array);
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
                &format!(
                    "model=\"{}\",serial=\"{}\"",
                    esc(model),
                    esc(serial)
                ),
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
