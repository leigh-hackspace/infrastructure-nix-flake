//! Answers out loud, on a device whose only door in is a shell.
//!
//! ## What it does
//!
//! `ssh` to the box and pipe PCM16 into its audio client, which reads stdin:
//! `rac play - -r 16000`. Measured 2026-11-16 with a 350 ms tone, which the device accepted and
//! reported as played (`docs/voice-assistant-2026-11-16.md` has the receipt) — that is the
//! *software* path proven, and audibility across a working room remains a question for a human.
//!
//! ## Why this route and not the clean one
//!
//! A root shell is not a pretty way to play a WAV, so the alternatives were checked: the
//! device's own RTSP backchannel is PCMU/8000 on firmware that advertises nothing else, which is
//! worse audio and more code; a dedicated service on the device does not exist. What remains is
//! the fleet's existing hop key, already on every box, used by exactly one binary.
//!
//! **And why nothing in this tree currently uses it.** The key lives in *operator* home
//! directories, which a `DynamicUser` cannot read, and copying a private key into the store is
//! what `infra.buildTimeSecrets` exists to make expensive. The two honest fixes — a dedicated
//! account whose *public* key is installed on the device, or a unit-managed agent holding the
//! hop key — are open in `docs/follow-ups.md`, and `voice-assistant.nix`'s header keeps the
//! reasoning. `speaker = "off"` is the deployed default, and it is a decision with a cost
//! attached, not shyness.
//!
//! ## The one field here that the core acts on
//!
//! `echo_cancelled: false`, because the device reports no canceller (`aec: false` in its own
//! status output). That is the fact the half-duplex rule in `satellite.rs` was written against,
//! and the fact a future platform can *change* — which is the reason it is a capability on the
//! sink rather than a line of code in the core.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::sink::{Capabilities, Played, Sink, SinkStream};
use crate::wav;

pub struct RacSsh {
    host: String,
    user: String,
    key: PathBuf,
    timeout: Duration,
}

impl RacSsh {
    pub fn new(host: String, user: String, key: PathBuf, timeout: Duration) -> RacSsh {
        RacSsh { host, user, key, timeout }
    }
}

impl Sink for RacSsh {
    fn label(&self) -> String {
        // The string `just voice-status` prints; `speaker = "ssh"` is the unit's own word for
        // this path, so the label says which mechanism answered, not just that something did.
        "ssh:rac-play".to_string()
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            audible: true,
            echo_cancelled: false,
        }
    }

    /// The device, the account and the key *path* — never the key, and never a credential.
    fn details(&self) -> String {
        format!("{}@{} via {}", self.user, self.host, self.key.display())
    }

    fn play(&mut self, pcm: &[i16]) -> Result<Played, String> {
        let secs = pcm.len() as f64 / wav::SAMPLE_RATE as f64;
        let started = Instant::now();
        let mut child = Command::new("ssh")
            .arg("-i")
            .arg(&self.key)
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg(format!("ConnectTimeout={}", self.timeout.as_secs().max(1)))
            .arg("-o")
            .arg("StrictHostKeyChecking=accept-new")
            .arg(format!("{}@{}", self.user, self.host))
            .arg(format!("rac play - -r {}", wav::SAMPLE_RATE))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("spawn ssh: {e}"))?;
        {
            let mut stdin = child.stdin.take().ok_or("ssh: no stdin")?;
            let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
            stdin
                .write_all(&bytes)
                .map_err(|e| format!("send pcm to rac: {e}"))?;
            stdin.flush().ok();
        }
        let status = child.wait().map_err(|e| format!("wait ssh: {e}"))?;
        if !status.success() {
            return Err(format!(
                "rac play exited {status} (is the device's audio daemon up? `rac status` says)"
            ));
        }
        // The device played it; the room hears it `secs` after we started writing. The ssh child
        // returns as soon as the bytes are gone, so the remainder of the playing time is slept
        // here — which is the *only* reason the core's half-duplex guard has ever been
        // trustworthy, and why `Played` reports seconds rather than success.
        let slack = (secs - started.elapsed().as_secs_f64()).max(0.0);
        std::thread::sleep(Duration::from_secs_f64(slack));
        Ok(Played { secs })
    }

    /// The device's audio client reads **stdin**, so the pipe is the streaming path and there is
    /// nothing to invent: `rac play -` starts playing as soon as samples arrive, which is exactly
    /// what a turn that produces audio while the model still has sentences left needs.
    fn streams(&self) -> bool {
        true
    }

    fn play_stream(&mut self) -> Result<Box<dyn SinkStream>, String> {
        let mut child = Command::new("ssh")
            .arg("-i")
            .arg(&self.key)
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg(format!("ConnectTimeout={}", self.timeout.as_secs().max(1)))
            .arg("-o")
            .arg("StrictHostKeyChecking=accept-new")
            .arg(format!("{}@{}", self.user, self.host))
            .arg(format!("rac play - -r {}", wav::SAMPLE_RATE))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("spawn ssh: {e}"))?;
        let stdin = child.stdin.take().ok_or("ssh: no stdin")?;
        Ok(Box::new(RacPipe {
            child: Some(child),
            stdin: Some(stdin),
            samples: 0,
            started: Instant::now(),
        }))
    }
}

struct RacPipe {
    child: Option<Child>,
    /// `Option` so `finish` can *close* it: `rac play` ends its track at EOF on this pipe, and a
    /// pipe held open by a `&mut` borrow never reaches EOF.
    stdin: Option<std::process::ChildStdin>,
    samples: usize,
    started: Instant,
}

impl SinkStream for RacPipe {
    fn chunk(&mut self, pcm: &[i16]) -> Result<(), String> {
        self.samples += pcm.len();
        let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| "rac play: the pcm pipe is already closed".to_string())?;
        stdin
            .write_all(&bytes)
            .and_then(|()| stdin.flush())
            .map_err(|e| format!("send pcm to rac: {e} (the device may have hung up)"))
    }

    fn finish(mut self: Box<Self>) -> Result<Played, String> {
        let secs = self.samples as f64 / wav::SAMPLE_RATE as f64;
        // Dropping the owned pipe is what sends EOF; without it the device waits for audio that
        // is never coming and `finish` would block in `wait` forever.
        drop(self.stdin.take());
        let status = self
            .child
            .take()
            .and_then(|mut c| c.wait().ok())
            .ok_or_else(|| "rac play: no child to wait for".to_string())?;
        if !status.success() {
            return Err(format!(
                "rac play exited {status} (is the device's audio daemon up? `rac status` says)"
            ));
        }
        // Same arithmetic as the buffered path, for the same reason: ssh returns when the bytes
        // are gone, and `Played::secs` is what the half-duplex guard is measured against.
        let slack = (secs - self.started.elapsed().as_secs_f64()).max(0.0);
        std::thread::sleep(Duration::from_secs_f64(slack));
        Ok(Played { secs })
    }
}

