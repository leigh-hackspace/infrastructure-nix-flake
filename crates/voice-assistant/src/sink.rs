//! The loudspeaker side of the device boundary.
//!
//! ## The job, and the line
//!
//! The core hands a sink the PCM of an answer and asks two questions: **did it go out, and for
//! how long** ([`Played`]), and **what is true about the transducer you just sent it to**
//! ([`Capabilities`]). Everything else — which host, which key, which codec, which shell
//! command on the far side — belongs to [`crate::drivers`], and
//! `common/check-platform-boundary.nix` refuses it here.
//!
//! ## Why `play` returns seconds
//!
//! The old shape returned `Ok(())` and the core *inferred* the playing time from the length of
//! the buffer it had been given, then guarded on top of that. That is a guess wearing a
//! measurement: it is right when the sink blocks until the last sample leaves the device and
//! wrong the moment a sink buffers, streams, or refuses half the audio. So the sink answers for
//! what it actually played, and the core does arithmetic on an answer rather than on its own
//! assumption. (The same trap as the wake harness's `0.0 ms per audio-second`: a number the
//! producer did not measure is not a measurement.)
//!
//! ## Why capabilities are data, not a hardcoded rule
//!
//! "Do not believe anything you hear while you are speaking" is currently unconditional,
//! because the only sink that can make noise is a device that reports no echo cancellation. It
//! is a property of the platform, not of the system — a device that cancels its own output can
//! listen while it talks, which is what barge-in needs and what `docs/follow-ups.md` has been
//! blocked on. So it is a field, and `satellite.rs` decides from it.
//!
//! **Honesty about the current state:** no sink in this tree reports `echo_cancelled: true`, so
//! that half of the decision is dead code today. `capture_to_discard_honours_what_the_sink_claimed`
//! in `satellite.rs` pins both halves of the branch, so the day an echo-cancelling sink lands,
//! the behaviour is tested before anyone believes it.

/// What the core is told about a transducer, before it decides what to do with what it hears.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Capabilities {
    /// Does sound actually reach the room? `false` is the deployed default, and the core counts
    /// those answers as `unplayed` in `status.json`, because "nothing was said" and "nobody
    /// could hear it" are different facts for a person debugging a quiet room.
    pub audible: bool,
    /// Does this sink remove its own output from what the microphone hears? The one permission
    /// a platform can grant the core, and the thing no device in this fleet has yet.
    pub echo_cancelled: bool,
}

/// What a sink says it did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Played {
    /// Seconds of the answer that reached the transducer. A sink that played nothing returns
    /// `0.0` — the core's guard then discards nothing, which is the correct consequence of
    /// having made no noise.
    pub secs: f64,
}

pub trait Sink: Send {
    /// The value of `status.json`'s `speaker` key. Stable strings, because `just voice-status`
    /// prints this one verbatim; a sink's label is an operator-facing fact, not a debug string.
    fn label(&self) -> String;

    fn capabilities(&self) -> Capabilities;

    /// Which device, account and key-file this points at, for the startup log line and nothing
    /// else — the core never parses it, and it names paths rather than contents (rule 13). The
    /// reason it exists at all: a trait object makes a struct's fields invisible to a test, and
    /// "did the operator's flags land in the right place" is worth pinning.
    fn details(&self) -> String {
        self.label()
    }

    /// Say it. Blocks until the audio is out of the process's hands and — if the sink can know
    /// — out of the speaker, so that the caller's half-duplex guard is measured rather than
    /// assumed.
    fn play(&mut self, pcm: &[i16]) -> Result<Played, String>;
}
