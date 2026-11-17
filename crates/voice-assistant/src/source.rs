//! The microphone side of the device boundary.
//!
//! ## The job, and the line
//!
//! A satellite's whole input is **a stream of 16 kHz mono s16le frames that can die**. That
//! is the contract, and nothing else. Where the frames come from — an RTSP restream, a
//! `ffmpeg` child, an ALSA device, a WebSocket from an ESP32, a WAV on a bench — is a
//! [`crate::drivers`] problem, and the words for those things must not appear outside it.
//! `common/check-platform-boundary.nix` is what keeps that sentence true rather than
//! aspirational.
//!
//! The format is the one part of a "device" that is *not* a device detail: `wav.rs`,
//! `gate.rs`, whisper, piper and every measured threshold in this area are 16 kHz mono s16le
//! figures, and a platform that hands over anything else is not a new platform, it is a
//! different system. So the rate is asserted by the format, not negotiated per driver, and a
//! driver that cannot produce it must resample before the boundary rather than teach the core
//! about rates.
//!
//! ## Who owns the half-duplex rule: the core, never the driver
//!
//! While the assistant is speaking, the audio it hears is its own answer, and the only thing
//! that makes a room with a speaker work without an echo canceller is **throwing that audio
//! away** (`satellite.rs`, and `docs/voice-assistant-2026-11-16.md` for the receipt). That
//! decision needs to know what is being played and for how long, which a source cannot know.
//! So the rule lives in the core and a driver is never told to pause or flush: it hands over
//! bytes and the core decides what they mean. This is why `Source` has no `discard(n)`. Put
//! the discarding in a driver and every new platform re-implements — or silently forgets — the
//! one rule that stops the assistant answering itself.
//!
//! ## Why a factory rather than one long-lived source
//!
//! Streams die: an ingest restarts when whatever restreams it does, a Wi-Fi link drops, a
//! file ends. The satellite's recovery is "start another one in five seconds"
//! (`satellite.rs`), and that is a fact about the *loop*, not about a device. So the core holds
//! a [`SourceFactory`] and calls it again; a driver just says how to begin.

use std::fmt;

/// What every source must produce, and the only format the core accepts.
pub const FORMAT: &str = "16 kHz mono s16le";

pub trait Source: Send {
    /// Fill as much of `buf` as the stream can, from whatever the platform produces.
    ///
    /// `Ok(0)` means **the stream ended** — the caller flushes the gate, reports it and opens
    /// another one. That is a normal event, not an error: a camera audio track ending because
    /// its container restarted is the same shape as a WAV finishing, and the core must not
    /// treat one as a fault and the other as clean.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, String>;

    /// What to print, and what `status.json` answers for `source`. It is a **label**: the core
    /// copies it and never parses it, so a driver can put anything a human needs in it.
    fn label(&self) -> String;

    /// Release whatever the platform holds, now, because the core is done with this stream
    /// (the bounded probe is the case that needs it: a child left reading a camera is a second
    /// consumer of a session budget someone else needs).
    fn stop(&mut self);
}

/// How to begin a stream. Held by the satellite's loop, which owns the restarting.
pub type SourceFactory = Box<dyn Fn() -> Result<Box<dyn Source>, String> + Send>;

impl fmt::Display for dyn Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label())
    }
}
