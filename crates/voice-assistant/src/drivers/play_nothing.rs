//! The safe speaker: synthesise the answer, keep it, say nothing.
//!
//! ## This is the deployed default, and it is a policy
//!
//! `speaker = "off"` in `machines/services1/services/voice-assistant.nix` — so in a live space,
//! every answer is produced, counted, and written to the unit's state directory as a WAV for
//! `just voice-say` and for a human to listen to, and **no sound reaches the room**. That is not
//! shyness: the alternative is an unattended voice in a room full of other people's conversation
//! on the strength of a tone test nobody in the room heard. The two things that have to be true
//! first are in that module's header, and the audibility question is open in
//! `docs/follow-ups.md`.
//!
//! ## Why it is a sink rather than an `if`
//!
//! Because "will anyone hear this" is a property of the output, and the core already has to know
//! it for two other reasons: the half-duplex guard must not throw away the room's audio on
//! account of audio that never played, and `status.json`'s `unplayed` counter exists to answer
//! the question "did it fail to speak, or did it speak and nobody could hear". A boolean
//! scattered through the loop is how those two answers start disagreeing.
//!
//! The label stays the deployment's own word — `off`, not `dry-run` — for exactly that reason: a
//! status file that answers a different question than the config asked is how an operator ends up
//! reasoning about a setting nobody set.

use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::sink::{Capabilities, Played, Sink, SinkStream};
use crate::wav;

pub struct PlayNothing {
    out_dir: PathBuf,
}

impl PlayNothing {
    pub fn new(out_dir: PathBuf) -> PlayNothing {
        PlayNothing { out_dir }
    }
}

impl Sink for PlayNothing {
    fn label(&self) -> String {
        "off".to_string()
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            audible: false,
            // Not a claim about cancellers — there is no transducer here. It makes no noise, so
            // `capture_to_discard` never reaches this field for a silent sink.
            echo_cancelled: false,
        }
    }

    fn details(&self) -> String {
        format!("answers kept in {}", self.out_dir.display())
    }

    fn play(&mut self, pcm: &[i16]) -> Result<Played, String> {
        let body = wav::mono16(pcm, wav::SAMPLE_RATE);
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = self.out_dir.join(format!("answer-{at}.wav"));
        let mut f = std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        f.write_all(&body)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        f.flush().ok();
        eprintln!(
            "not speaking: {} bytes of answer at {}",
            body.len(),
            path.display()
        );
        // Best effort on the retention: a state directory that cannot be pruned must not fail an
        // answer that has already been written.
        wav::prune_wavs(&self.out_dir, 40);
        Ok(Played { secs: 0.0 })
    }

    /// The streamed answer is still **one** answer, so it is buffered and written as one WAV.
    ///
    /// This sink is the reason the streaming question has to be asked of the sink rather than
    /// answered in the core: there is nothing here to stream *to*, and writing forty files for one
    /// turn would make the state directory useless for the thing it exists for — `just voice-say`
    /// and a human listening to what the assistant told the room.
    fn streams(&self) -> bool {
        true
    }

    fn play_stream(&mut self) -> Result<Box<dyn SinkStream>, String> {
        Ok(Box::new(KeepAll {
            out_dir: self.out_dir.clone(),
            pcm: Vec::new(),
        }))
    }
}

struct KeepAll {
    out_dir: PathBuf,
    pcm: Vec<i16>,
}

impl SinkStream for KeepAll {
    fn chunk(&mut self, pcm: &[i16]) -> Result<(), String> {
        self.pcm.extend_from_slice(pcm);
        Ok(())
    }

    fn finish(self: Box<Self>) -> Result<Played, String> {
        if self.pcm.is_empty() {
            return Ok(Played { secs: 0.0 });
        }
        let body = wav::mono16(&self.pcm, wav::SAMPLE_RATE);
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = self.out_dir.join(format!("answer-{at}.wav"));
        let mut f = std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        f.write_all(&body)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        f.flush().ok();
        eprintln!(
            "not speaking: {:.1} s of streamed answer at {}",
            self.pcm.len() as f64 / wav::SAMPLE_RATE as f64,
            path.display()
        );
        wav::prune_wavs(&self.out_dir, 40);
        Ok(Played { secs: 0.0 })
    }
}
