//! A stream of 16 kHz mono s16le, produced by the network's audio tool.
//!
//! This module exists so that `satellite.rs` can be about a satellite. It spawns the child,
//! hands over its stdout, and dies when the child does — nothing else. The reasoning that
//! produced these particular arguments belongs to the platform, so it is written here rather
//! than in a core file that would otherwise be quoting device facts it does not own.
//!
//! ## Why the arguments are what they are
//!
//! * **the transport flag is attached to the protocol, not to every input.** The TCP-only flag
//!   is an RTSP *input* option and the tool rejects it on anything else — measured as
//!   `Option rtsp_transport not found` on a WAV, which is exactly how a bench file and a live
//!   stream would otherwise need two different invocations. TCP is what a camera wants, because
//!   UDP fragments die on a congested 2.4 GHz link (`docs/camera-wifi-cells-2026-10-31.md`);
//!   a file needs nothing.
//! * **`-fflags nobuffer`** — the satellite is deciding whether the *last half second* is
//!   someone talking. Buffering a second of audio to make the demuxer happier is how a live
//!   room ends up being answered a sentence late.
//! * **`-vn -map a?`** — video is somebody else's problem (frigate's), and `a?` means a source
//!   with no audio track produces a clean end-of-stream rather than an error, which the core
//!   then reports as "the stream ended" instead of as a crash.
//! * **`-ac 1 -ar 16000 -f s16le`** — the format is the boundary
//!   ([`crate::source::FORMAT`]). The rate is not a preference: every threshold in
//!   `gate.rs`, every dBFS figure in `docs/`, whisper and piper are 16 kHz numbers.
//! * **stderr inherited** — when this child fails, its own words are the diagnosis, and
//!   capturing them to re-print later is how a device fault arrives as a truncated summary.

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use crate::source::Source;

pub struct Stream {
    child: Child,
    stdout: Box<dyn Read + Send>,
    source: String,
}

impl Stream {
    /// Begin one stream. Called again by the satellite's loop every time the previous one ends.
    pub fn open(tool: &Path, source: &str) -> Result<Stream, String> {
        let mut cmd = Command::new(tool);
        cmd.arg("-hide_banner").arg("-loglevel").arg("error").arg("-nostdin");
        if source.starts_with("rtsp://") {
            cmd.arg("-rtsp_transport").arg("tcp").arg("-fflags").arg("nobuffer");
        }
        let mut child = cmd
            .arg("-i")
            .arg(source)
            .arg("-vn")
            .arg("-map")
            .arg("a?")
            .arg("-ac")
            .arg("1")
            .arg("-ar")
            .arg(crate::wav::SAMPLE_RATE.to_string())
            .arg("-f")
            .arg("s16le")
            .arg("-")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| {
                format!(
                    "cannot run {} (the unit names it via --ffmpeg; the arguments this module \
                     passes are the ones it needs from it): {e}",
                    tool.display()
                )
            })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| format!("{source}: no stdout pipe"))?;
        Ok(Stream {
            child,
            stdout: Box::new(stdout),
            source: source.to_string(),
        })
    }
}

impl Source for Stream {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, String> {
        let n = self.stdout.read(buf).map_err(|e| format!("{}: {e}", self.source))?;
        if n == 0 {
            // Reap here so the caller never has to know a process was involved: "the stream
            // ended" is the whole contract, and how a platform ends is this module's business.
            let _ = self.child.wait();
        }
        Ok(n)
    }

    fn label(&self) -> String {
        self.source.clone()
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
