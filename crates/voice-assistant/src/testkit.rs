//! Fixtures for the tests that need more than one process: a fake piper, a fake `slack-agent`, a
//! fake whisper.
//!
//! Everything here speaks the **wire**, not the internals. That is the point: the claim this crate
//! has to defend is that a turn is fast when the parts are overlapped, and a claim about timing
//! across a socket and two child processes cannot be tested by calling the parts. Each fixture is
//! deliberately dumb — it replays a script and sleeps when told to — because a smart fixture would
//! be a second implementation of the thing under test.
//!
//! `#[cfg(test)]` only, and it lives in one file rather than beside each caller for the same
//! reason every `common-rs` crate exists: the fake piper script was already about to be copied.

#![cfg(test)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::Duration;

/// Publish a shell script as an executable **atomically**.
///
/// `fs::write` then `set_permissions` then `exec` is a race: between the truncate and the close
/// another thread's exec of the same path gets `Text file busy (os error 26)`, and a test suite
/// that runs 100 tests over 8 cores will find that window eventually. Written beside a fixture,
/// this reads like paranoia; it was found by a flake that only appeared once the pipeline tests
/// added load, and the fix is a rename, which is atomic.
pub fn write_exec(path: &Path, script: &str) {
    let tmp = path.with_extension("sh.tmp");
    std::fs::write(&tmp, script).unwrap();
    std::fs::set_permissions(&tmp, PermissionsExt::from_mode(0o755)).unwrap();
    std::fs::rename(&tmp, path).unwrap();
}

/// A private scratch directory per test, cleaned before use. Keyed on the test tag and the process
/// id so parallel tests in one binary cannot collide, and so a leftover from a crashed run is
/// removed rather than inherited.
pub fn tmp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("voice-kit-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A voice model path plus the config piper reads for the sample rate. 16 kHz, which is what the
/// whole pipeline assumes, so nothing in a test has to reason about resampling.
pub fn voice_model(dir: &Path) -> PathBuf {
    let model = dir.join("en_GB-alba-medium.onnx");
    std::fs::write(&model, b"not a model").unwrap();
    std::fs::write(
        dir.join("en_GB-alba-medium.onnx.json"),
        br#"{"audio":{"sample_rate":16000}}"#,
    )
    .unwrap();
    model
}

/// A stand-in for piper that answers **both** modes this crate asks for: `--output-raw` (one
/// sentence, one process, stdout) and `--output-dir` (a resident process, one WAV per line, the
/// path announced on stderr). Both are here in one script because the pipeline tests exercise the
/// resident path and its cold fallback in the same turn, and two scripts would be two chances for
/// the two of them to disagree about what piper does.
///
/// `die_after` sentences makes the resident process exit non-zero after that many, which is how the
/// fallback and the restart counter get tested. `pause_ms` is the synthesis cost to simulate — the
/// pipeline's overlap is only observable if the voice is slow enough to still be working when the
/// model finishes.
pub fn fake_piper(dir: &Path, die_after: usize, pause_ms: u64) -> PathBuf {
    let bin = dir.join("piper");
    // `awk` is not in the sandbox this test runs in, so the sleep is spelled by Rust. It is also
    // the only thing that makes the overlap assertion falsifiable: a pause that silently no-ops
    // turns the timing test into an ordering test that passes for the wrong reason.
    let script = r#"#!/bin/sh
dir=""
raw=0
while [ $# -gt 0 ]; do
  case "$1" in
    --output-dir) dir="$2"; shift 2;;
    --output-raw) raw=1; shift;;
    *) shift;;
  esac
done
# 32 samples of a 1-count square wave: short, and non-zero, so a silent synthesiser bug cannot
# hide as "the test's fault".
if [ "$raw" -eq 1 ]; then
  IFS= read -r line
  sleep __PAUSE__
  j=0; while [ $j -lt 32 ]; do printf '\001\000'; j=$((j+1)); done
  exit 0
fi
mkdir -p "$dir"
i=0
while IFS= read -r line; do
  i=$((i+1))
  if [ "$i" -gt __DIE__ ]; then
    echo "Traceback: simulated crash on $line" >&2
    exit 7
  fi
  sleep __PAUSE__
  printf 'RIFFd\000\000\000WAVEfmt \020\000\000\000\001\000\001\000\200>\000\000\000}\000\000\002\000\020\000data@\000\000\000' > "$dir/$i.wav"
  j=0; while [ $j -lt 32 ]; do printf '\001\000' >> "$dir/$i.wav"; j=$((j+1)); done
  echo "INFO:__main__:Wrote $dir/$i.wav" >&2
done
"#
    .replace("__DIE__", &die_after.to_string())
    .replace(
        "__PAUSE__",
        // `sleep 0` is legal and instant, which is how a test asks for no pause at all.
        &format!("{:.3}", pause_ms as f64 / 1000.0),
    );
    write_exec(&bin, &script);
    bin
}

/// One event of a scripted answer: `(event name, data JSON, milliseconds to wait first)`.
pub type SseEvent = (&'static str, String, u64);

/// A fake `slack-agent`: serves `POST /v1/ask/stream` with a scripted SSE body and
/// `POST /v1/ask/prefill` with a canned warm-up receipt, then stops after `connections` requests.
///
/// The sleeps are the test. A fixture that wrote all its events in one burst would pass a check
/// that audio "arrived before the answer" purely by the order of the script, which proves nothing
/// about a pipeline; sleeping before the last event is what leaves room for the voice thread to
/// have gotten there first, and the assertion then means something.
///
/// Returns the base URL and the thread to join. The thread must be joined (or the connection count
/// must be right), or it outlives the assertions and the next test reads a socket it did not open.
pub fn fake_agent(events: Vec<SseEvent>, connections: usize) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        for _ in 0..connections {
            let Ok((mut sock, _)) = listener.accept() else {
                return;
            };
            let events = events.clone();
            std::thread::spawn(move || {
                let path = serve(&mut sock, &events);
                // `/v1/ask/prefill` is a plain JSON reply; everything else on this fake is the
                // streamed turn.
                let body = if path.contains("prefill") {
                    "{\"ok\":true,\"engine\":\"fake (bench GPU)\",\"prompt_tokens\":900,\
                     \"cached_tokens\":0,\"millis\":12,\"skipped\":\"\"}"
                        .to_string()
                } else {
                    String::new()
                };
                let streaming = body.is_empty();
                let head = if streaming {
                    // Chunked, like the real gateway: a streaming body must not carry
                    // `Content-Length: 0`, which is what a careless fixture writes and what makes
                    // a reader report an empty body and an instant, silent end of turn.
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                     Cache-Control: no-store\r\nTransfer-Encoding: chunked\r\n\
                     Connection: close\r\n\r\n"
                        .to_string()
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                };
                let _ = sock.write_all(head.as_bytes());
                if streaming {
                    for (name, data, ms) in &events {
                        if *ms > 0 {
                            std::thread::sleep(Duration::from_millis(*ms));
                        }
                        // One chunk per event, flushed: a fixture that buffered them all would
                        // faithfully reproduce the exact bug `common_web::Sse` exists to avoid.
                        let frame = format!("event: {name}\ndata: {data}\n\n");
                        let _ = sock.write_all(format!("{:x}\r\n{}\r\n", frame.len(), frame).as_bytes());
                        let _ = sock.flush();
                    }
                    let _ = sock.write_all(b"0\r\n\r\n");
                } else {
                    let _ = sock.write_all(body.as_bytes());
                }
                let _ = sock.shutdown(std::net::Shutdown::Write);
            });
        }
    });
    (format!("http://127.0.0.1:{port}"), handle)
}

/// Read one request far enough to know which route it is and to not leave the client writing into
/// a full pipe. No keep-alive: every response here is `Connection: close`.
fn serve(sock: &mut TcpStream, _events: &[SseEvent]) -> String {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    let mut header_end = None;
    while buf.len() < 16 * 1024 {
        if sock.read(&mut byte).unwrap_or(0) == 0 {
            break;
        }
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n\r\n") {
            header_end = Some(buf.len());
            break;
        }
    }
    let Some(end) = header_end else {
        return String::new();
    };
    let head = String::from_utf8_lossy(&buf[..end]).to_lowercase();
    let length: usize = head
        .lines()
        .find_map(|l| {
            l.strip_prefix("content-length:")
                .and_then(|v| v.trim().parse().ok())
        })
        .unwrap_or(0);
    let mut got = buf.len() - end;
    let mut scratch = [0u8; 512];
    while got < length {
        let want = (length - got).min(scratch.len());
        let n = sock.read(&mut scratch[..want]).unwrap_or(0);
        if n == 0 {
            break;
        }
        got += n;
    }
    String::from_utf8_lossy(&buf).lines().next().unwrap_or("").to_string()
}

/// A fake whisper: answers `POST /v1/transcribe` (the shape the satellite uses) with `reply`,
/// sleeping `pause_ms` first so a caller's "did this overlap?" assertions have room.
pub fn fake_whisper(reply: &str, pause_ms: u64, connections: usize) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let reply = reply.to_string();
    let handle = std::thread::spawn(move || {
        for _ in 0..connections {
            let Ok((mut sock, _)) = listener.accept() else {
                return;
            };
            let reply = reply.clone();
            std::thread::spawn(move || {
                let _ = serve(&mut sock, &[]);
                if pause_ms > 0 {
                    std::thread::sleep(Duration::from_millis(pause_ms));
                }
                let body = format!(
                    "{{\"text\":{},\"speech\":true,\"language\":\"en\"}}",
                    common_json::quoted(&reply)
                );
                let _ = sock.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                );
                let _ = sock.shutdown(std::net::Shutdown::Write);
            });
        }
    });
    (format!("http://127.0.0.1:{port}"), handle)
}
