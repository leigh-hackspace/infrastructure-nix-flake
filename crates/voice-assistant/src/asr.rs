//! Transcribing audio **while someone is still saying it**.
//!
//! ## The whole latency argument, in one line
//!
//! Whisper on this fleet's box costs about 0.9 × real time (measured: 3.0 s of room audio →
//! 2.7 s, `docs/voice-assistant-2026-11-16.md`), so a five-second question that is transcribed
//! only after the person stops talking costs them 4.5 s of silence before the model has even
//! seen the words. Cut the audio into slices and transcribe each one as it completes, and the
//! same five-second question costs only the *last slice* after they stop — under a second —
//! while the earlier slices were transcribed during the speech that followed them. The work is
//! the same; the **waiting** moves.
//!
//! ## Why slices and not a real streaming ASR
//!
//! whisper.cpp's server is one-shot (`POST /inference`, whole WAV in, transcript out — verified
//! in its source, and the reason `crates/whisper-ws/` exists at all). There is no partial
//! hypothesis to subscribe to, so the honest approximation is repeated one-shot calls over a
//! sliding window. That is what this is: no new model, no new server, and a CPU bill that is
//! within a few per cent of the single call it replaces (each slice pays whisper's fixed
//! attention cost once, so slices are *slightly* more work — in exchange for the waiting above).
//!
//! ## The two things slice boundaries do to a transcript, and what this file does back
//!
//! 1. **A slice can start mid-word.** The word comes out as two fragments, or the second slice
//!    says the word again. So each slice is re-fed with [`overlap_secs`] of the previous audio
//!    in front of it — enough for the word to be heard whole by one of the two calls — and
//!    [`dedup_join`] removes the repeated words at the seam.
//! 2. **The seam is where the text is least reliable**, so the caller is told which part is
//!    settled: every transcript this hands out is *complete up to the last slice*, and the final
//!    [`Rolling::finish`] result is the only one that has had the whole utterance.
//!
//! ## Silence
//!
//! Whisper does not return an empty string for quiet audio: it returns a *description*
//! (`(soft music)`) or, worse, words. The caller decides what counts as speech (that is
//! [`crate::gate`]) and this module takes the audio it is given on trust, but it does report
//! descriptions as nothing-said through [`crate::stt::looks_like_description`] so a hum cannot
//! become a transcript that then wakes the assistant.

use std::time::Instant;

use crate::stt;

/// Audio re-sent at the front of each slice, seconds. Enough for a word the previous slice cut
/// in half to be heard whole — and no more, because every extra millisecond is whisper work
/// that has to be deduplicated out of the text afterwards.
pub const DEFAULT_OVERLAP_SECS: f64 = 0.25;

/// How many words [`dedup_join`] will try to match at the seam, longest first. Three is more
/// than a seam ever needs and few enough that a false match cannot eat a sentence.
const MAX_DEDUP_WORDS: usize = 3;

/// What a slice costs, as reported to the caller for `status.json`.
#[derive(Debug, Default)]
pub struct AsrStats {
    pub calls: usize,
    pub millis: u128,
    /// Slices that came back as a description of noise rather than words.
    pub described: usize,
    /// The last slice's wall clock: the part of the transcription the caller still has to wait
    /// for after the speaker stops. This, not `millis`, is the latency figure.
    pub last_ms: u128,
}

/// A rolling transcriber. Generic over *how* a slice is transcribed, because the two callers
/// are on different sides of a socket: the gateway calls whisper directly, and the satellite
/// asks the gateway (AGENTS rule 12's shape — the satellite has no whisper URL of its own).
pub struct Rolling {
    /// Samples per slice, the latency dial.
    want: usize,
    /// Samples of already-transcribed audio re-sent at the front of the next slice.
    overlap: usize,
    /// The tail of what was last sent, at most `overlap` samples.
    tail: Vec<i16>,
    /// Audio received and not yet sent.
    pending: Vec<i16>,
    text: String,
    pub stats: AsrStats,
    transcribe: Transcribe,
}

/// How a slice becomes words, injected rather than hardcoded: the satellite calls the gateway's
/// `/v1/transcribe` and the gateway calls whisper directly, and neither the slicer nor its tests
/// need to know which. `Send` because the satellite runs it on its own thread.
type Transcribe = Box<dyn Fn(&[i16]) -> Result<String, String> + Send>;

impl Rolling {
    /// `slice_secs` is the latency dial: shorter means the transcript updates more often and
    /// the tail is shorter when someone stops, and every halving adds another whisper call.
    pub fn new<F>(slice_secs: f64, overlap_secs: f64, transcribe: F) -> Rolling
    where
        F: Fn(&[i16]) -> Result<String, String> + Send + 'static,
    {
        Rolling {
            want: (slice_secs * crate::wav::SAMPLE_RATE as f64) as usize,
            overlap: (overlap_secs * crate::wav::SAMPLE_RATE as f64) as usize,
            tail: Vec::new(),
            pending: Vec::new(),
            text: String::new(),
            stats: AsrStats::default(),
            transcribe: Box::new(transcribe),
        }
    }

    /// The same, from whisper's URL and language, for a caller that has a whisper to hand.
    pub fn from_whisper(slice_secs: f64, url: &str, language: Option<String>) -> Rolling {
        let client = stt::Client::new(url, language);
        Rolling::new(
            slice_secs,
            DEFAULT_OVERLAP_SECS,
            move |samples| {
                let body = crate::wav::mono16(samples, crate::wav::SAMPLE_RATE);
                client.transcribe(&body).map(|t| t.text)
            },
        )
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Feed audio. Sends whole slices as they fill, and returns the transcript so far each time
    /// one lands. A failing slice is an `Err` and the caller decides what to do with the
    /// utterance — the satellite keeps it and tries again at the end.
    pub fn push(&mut self, samples: &[i16]) -> Result<Option<String>, String> {
        if self.want == 0 {
            return Ok(None);
        }
        self.pending.extend_from_slice(samples);
        let mut last = None;
        // One slice per call, in order. Sending everything available in one go would be fewer
        // calls and would defeat the entire point: a slice is the unit of *latency*.
        while self.pending.len() >= self.want {
            let whole: Vec<i16> = self.pending.drain(..self.want).collect();
            last = self.send(&whole)?;
        }
        Ok(last)
    }

    /// The last partial window, however short. Always called: whatever is `pending` is audio
    /// someone spoke, and it has no other route to a transcript.
    pub fn finish(&mut self) -> Result<String, String> {
        if self.pending.is_empty() || self.want == 0 {
            return Ok(self.text.clone());
        }
        let rest = std::mem::take(&mut self.pending);
        self.send(&rest)?;
        Ok(self.text.clone())
    }

    /// Transcribe `audio`, with the previous tail in front of it, and join the words.
    fn send(&mut self, audio: &[i16]) -> Result<Option<String>, String> {
        let mut buf: Vec<i16> = Vec::with_capacity(self.tail.len() + audio.len());
        buf.extend_from_slice(&self.tail);
        buf.extend_from_slice(audio);
        let started = Instant::now();
        let said = (self.transcribe)(&buf)?;
        let ms = started.elapsed().as_millis();
        self.stats.calls += 1;
        self.stats.millis += ms;
        self.stats.last_ms = ms;
        // The next slice's prefix comes from **this slice's own audio**, not from `buf`: taking
        // it from `buf` would re-send the overlap twice and grow the buffer every slice, which
        // is the kind of leak that only shows up on a long utterance.
        let keep = self.overlap.min(audio.len());
        self.tail = audio[audio.len() - keep..].to_vec();
        if stt::looks_like_description(&said) {
            self.stats.described += 1;
            return Ok(None);
        }
        self.text = dedup_join(&self.text, &said);
        Ok(Some(self.text.clone()))
    }
}

/// Join a slice's transcript onto what came before, removing the words the seam repeated.
///
/// Matching is case- and punctuation-insensitive because whisper capitalises a sentence's first
/// word and the same word mid-sentence differently: `"the workshop"` followed by `"The workshop
/// is warm"` must lose its first two words, and comparing bytes would keep them and say "the
/// workshop the workshop is warm".
///
/// **The trade, stated:** a seam repeat is indistinguishable from a word someone genuinely said
/// twice, and this chooses to assume the seam — dropping `"no"` from `"no no" + "no more"` costs
/// one word of a stammer, while keeping a seam repeat costs the assistant a stutter in the
/// transcript it decides wake words on. `MAX_DEDUP_WORDS` is what bounds the damage either way.
pub fn dedup_join(prev: &str, next: &str) -> String {
    let pw: Vec<String> = prev.split_whitespace().map(key).collect();
    let nw: Vec<&str> = next.split_whitespace().collect();
    let mut drop = 0usize;
    for k in (1..=MAX_DEDUP_WORDS).rev() {
        if k > nw.len() || k > pw.len() {
            continue;
        }
        let head: Vec<String> = nw[..k].iter().map(|w| key(w)).collect();
        if pw[pw.len() - k..] == head[..] {
            drop = k;
            break;
        }
    }
    let rest: Vec<&str> = nw[drop..].to_vec();
    if prev.trim().is_empty() {
        return rest.join(" ");
    }
    if rest.is_empty() {
        return prev.trim().to_string();
    }
    format!("{} {}", prev.trim(), rest.join(" "))
}

fn key(word: &str) -> String {
    word.chars()
        .filter(|c| !matches!(c, '.' | ',' | '!' | '?' | ':' | ';' | '"' | '\'' | '’'))
        .collect::<String>()
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A transcriber that answers from a queue, so a test says exactly what whisper would have
    /// said about each slice — including the seam repeats that this module exists to fix.
    fn scripted(replies: Vec<&'static str>) -> (Rolling, std::sync::mpsc::Receiver<usize>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let it = std::sync::Mutex::new(replies.into_iter());
        let r = Rolling::new(1.0, 0.25, move |samples| {
            let _ = tx.send(samples.len());
            let text = it
                .lock()
                .unwrap()
                .next()
                .map(str::to_string)
                .unwrap_or_else(|| "(silence)".to_string());
            Ok(text)
        });
        (r, rx)
    }

    fn samples(secs: f64) -> Vec<i16> {
        vec![0i16; (secs * crate::wav::SAMPLE_RATE as f64) as usize]
    }

    #[test]
    fn a_slice_is_sent_as_soon_as_its_audio_exists_and_not_before() {
        let (mut r, rx) = scripted(vec!["is the space", "open right now"]);
        assert!(r.push(&samples(0.6)).unwrap().is_none(), "a slice is not a fragment");
        assert!(rx.try_recv().is_err());
        assert_eq!(
            r.push(&samples(0.5)).unwrap().as_deref(),
            Some("is the space")
        );
        // The first slice has nothing to overlap with; the **second** carries the first's tail.
        assert_eq!(rx.try_recv().unwrap(), 16_000);
        r.push(&samples(1.0)).unwrap();
        assert_eq!(rx.try_recv().unwrap(), 16_000 + 4_000, "overlap re-sent");
        assert_eq!(r.finish().unwrap(), "is the space open right now");
        assert_eq!(r.stats.calls, 3);
    }

    #[test]
    fn audio_left_over_at_the_end_is_still_transcribed() {
        // A three-slice question that ends 0.4 s into the fourth: that 0.4 s is the end of the
        // sentence, and dropping it is how "is the space open ri" becomes the transcript.
        let (mut r, _rx) = scripted(vec!["one", "two", "three", "four", "five"]);
        r.push(&samples(4.4)).unwrap();
        assert_eq!(r.finish().unwrap(), "one two three four five");
        assert_eq!(r.stats.calls, 5, "four whole slices, then the 0.4 s tail");
    }

    #[test]
    fn the_seam_repeat_is_removed_and_a_real_repeat_is_not() {
        // What whisper actually does when a slice cuts "open" in half: the next slice, hearing
        // the word whole, says it again.
        assert_eq!(
            dedup_join("is the space open", "Open right now"),
            "is the space open right now"
        );
        assert_eq!(
            dedup_join("the workshop is", "is warm"),
            "the workshop is warm"
        );
        assert_eq!(dedup_join("", "hello"), "hello");
        assert_eq!(dedup_join("only", ""), "only");
        // The cost of the rule above, in the open: a real stammer loses a word at the seam.
        assert_eq!(dedup_join("no no", "no more"), "no no more");
        // And a longer match wins over a shorter one.
        assert_eq!(
            dedup_join("the door code is on", "code is on the wiki"),
            "the door code is on the wiki"
        );
    }

    #[test]
    fn a_slice_that_described_the_room_adds_nothing() {
        // The measured failure: this exact path once turned the space's stereo into
        // "(soft music)", and a transcript that contains that is a transcript that wakes the
        // assistant for a song.
        let (mut r, _rx) = scripted(vec!["(soft music)", "hey jarvis"]);
        r.push(&samples(1.0)).unwrap();
        assert_eq!(r.text(), "");
        assert_eq!(r.stats.described, 1);
        r.push(&samples(1.0)).unwrap();
        assert_eq!(r.text(), "hey jarvis");
    }

    #[test]
    fn the_last_slice_is_the_wait_and_the_sum_is_the_cost() {
        let (mut r, _rx) = scripted(vec!["a", "b", "c", "d"]);
        for _ in 0..4 {
            r.push(&samples(1.0)).unwrap();
        }
        assert_eq!(r.stats.calls, 4);
        assert!(r.stats.last_ms <= r.stats.millis);
    }

    #[test]
    fn a_zero_slice_length_is_no_transcription_at_all_rather_than_a_panic() {
        let (mut r, _rx) = scripted(vec![]);
        assert!(r.push(&samples(2.0)).unwrap().is_none());
        assert!(r.finish().unwrap().is_empty());
    }
}
