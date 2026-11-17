//! The only place in this crate that is allowed to know what a device is.
//!
//! ## The rule, and the gate that enforces it
//!
//! Host names, URLs, shell commands, codecs, the name of the tool we spawn: **here, or
//! nowhere.** [`crate::satellite`] drives a [`crate::source::SourceFactory`] and a
//! [`crate::sink::Sink`] and contains no device nouns; `common/check-platform-boundary.nix`
//! substring-checks that, so the boundary cannot rot the way a comment would. The word
//! *platform* is used deliberately: a camera vendor, a browser tab and an ESP32 are the same
//! kind of thing to the core, and adding one is supposed to mean a new module in this
//! directory plus a row in the module's own Nix, never an edit to the turn logic.
//!
//! ## What is *not* here
//!
//! The two decisions that look like device details and are not:
//!
//! * **when to stop listening** (the half-duplex guard, `satellite.rs`) — a driver cannot know
//!   what is being played, and every platform that re-implements it re-implements the bug where
//!   the assistant answers itself;
//! * **whether a transcript was addressed to us** (`wake.rs`, and the gate in `gate.rs`) — the
//!   same phrase must mean the same thing on every platform, and a per-device copy of it is
//!   rule 2 with extra steps.
//!
//! ## What a new platform costs, deliberately
//!
//! One module implementing two small traits, one line in [`open_source`]/[`open_sink`], and the
//! defaults for its own address. If a new platform needs anything more than that from the core,
//! that is a finding about the boundary, not a licence to reach in.

pub mod ffmpeg_stream;
pub mod play_nothing;
pub mod rac_ssh;

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::sink::Sink;
use crate::source::SourceFactory;

/// Where the network's own microphone lives. `127.0.0.1` because this is **go2rtc's
/// restream**, not the camera: the device's RTSP server allows two clients and frigate already
/// holds one, so the second session would starve the NVR on its next reconnect. Reading the
/// copy that is already up costs nothing and asks nothing of the camera — `cameras/README.md`
/// states that discipline as a rule, and `docs/voice-assistant-2026-11-16.md` has the
/// measurement behind it (`L16/16000` on the restream, which is the rate the core wants).
pub const DEFAULT_SOURCE: &str = "rtsp://127.0.0.1:8554/cam8";

/// The device whose speaker we drive when someone turns it on, and the account on it that owns
/// the audio client. Both are this file's business only: the core's word for a thing that can
/// make noise is [`crate::sink::Sink`].
pub const DEFAULT_SPEAKER_HOST: &str = "cam8.int.leighhack.org";
pub const DEFAULT_SPEAKER_USER: &str = "root";

/// The operator's own hop key: the only path to the device's audio client today, and the
/// reason the speaker stays off until a `DynamicUser` is given a key it may legitimately read
/// (the two honest options are in `docs/follow-ups.md`, and `voice-assistant.nix`'s header
/// says why nothing in this tree provisions one).
pub fn default_ssh_key() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(format!("{home}/.ssh/agent-hop-key"))
}

/// Everything a sink might need that is not itself a device fact. Built by `main.rs` from the
/// flags the deployed unit already passes, so the CLI does not change when the internals do.
pub struct SinkSpec {
    pub host: String,
    pub user: String,
    pub key: PathBuf,
    pub out_dir: PathBuf,
    pub timeout: Duration,
}

/// Open a stream. One driver for now: [`ffmpeg_stream`] handles RTSP, a file, or a pipe
/// because it is the same child process with one argument set conditionally — which is also
/// why `--source` may be a URL or a path and nothing upstream needs to know which.
pub fn open_source(source: &str, tool: &Path) -> SourceFactory {
    let source = source.to_string();
    let tool = tool.to_path_buf();
    Box::new(move || {
        ffmpeg_stream::Stream::open(&tool, &source).map(|s| Box::new(s) as Box<_>)
    })
}

/// Open a sink from the word the operator (or the unit) used.
///
/// The vocabulary is the unit's, and it is *not* validated here beyond these four words: an
/// unknown one is an error so the caller can fall back to the safe default out loud rather than
/// silently acquiring a speaker nobody asked for.
pub fn open_sink(kind: &str, spec: &SinkSpec) -> Result<Box<dyn Sink>, String> {
    match kind {
        "off" | "dry-run" | "none" => Ok(Box::new(play_nothing::PlayNothing::new(spec.out_dir.clone()))),
        "ssh" => Ok(Box::new(rac_ssh::RacSsh::new(
            spec.host.clone(),
            spec.user.clone(),
            spec.key.clone(),
            spec.timeout,
        ))),
        other => Err(format!("unknown speaker {other:?}")),
    }
}

/// The defaults are device facts, so the values themselves are pinned *here* — the one file
/// allowed to name them.  `main.rs`'s test uses a fake host instead, because what it is proving
/// is that the operator's flags reach the driver, not that this constant still says what the
/// deployed unit does.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_deployed_defaults_are_the_ones_the_unit_relies_on() {
        assert_eq!(DEFAULT_SOURCE, "rtsp://127.0.0.1:8554/cam8");
        assert_eq!(DEFAULT_SPEAKER_HOST, "cam8.int.leighhack.org");
        assert_eq!(DEFAULT_SPEAKER_USER, "root");
        assert!(default_ssh_key().ends_with(".ssh/agent-hop-key"));
    }

    #[test]
    fn the_vocabulary_is_the_units_and_an_unknown_word_is_refused() {
        let spec = SinkSpec {
            host: "h".into(),
            user: "u".into(),
            key: PathBuf::from("/nonexistent/key"),
            out_dir: std::env::temp_dir(),
            timeout: std::time::Duration::from_secs(1),
        };
        assert_eq!(open_sink("off", &spec).unwrap().label(), "off");
        assert_eq!(open_sink("dry-run", &spec).unwrap().label(), "off");
        assert_eq!(open_sink("ssh", &spec).unwrap().label(), "ssh:rac-play");
        // The safety property: a typo does not silently acquire a loudspeaker.
        assert!(open_sink("yes-please", &spec).is_err());
    }

    #[test]
    fn only_the_loud_sink_claims_it_can_be_heard() {
        let spec = SinkSpec {
            host: "h".into(),
            user: "u".into(),
            key: PathBuf::from("/nonexistent/key"),
            out_dir: std::env::temp_dir(),
            timeout: std::time::Duration::from_secs(1),
        };
        assert!(!open_sink("off", &spec).unwrap().capabilities().audible);
        let loud = open_sink("ssh", &spec).unwrap();
        assert!(loud.capabilities().audible);
        // The claim that would unlock barge-in, and which nothing in this fleet can make yet.
        assert!(!loud.capabilities().echo_cancelled);
    }
}
