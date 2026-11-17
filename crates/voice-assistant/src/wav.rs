//! WAV reading, writing and sample-rate conversion.
//!
//! There are exactly two sample rates in this service, and the reason is measured
//! rather than chosen: **16 kHz** is what the whole capture side speaks — the
//! network's own microphone publishes `l16` at 16000 Hz (the receipt, read off the
//! device, is `docs/voice-assistant-2026-11-16.md`), and it is
//! what whisper.cpp wants — and **22 050 Hz** is what a medium-quality piper voice
//! emits (`audio.sample_rate` in the voice's `.onnx.json`).  So one conversion
//! exists, in one direction, on the way out to a browser or a camera speaker, and
//! `resample` is written for exactly that pair while remaining general.
//!
//! PCM16 only.  Every producer here emits it, and a decoder for float or mu-law
//! would be code nothing exercises — better a clear error naming the format than a
//! guess.

pub const SAMPLE_RATE: u32 = 16_000;

/// A decoded mono or stereo PCM16 WAV.
#[derive(Debug)]
pub struct Wav {
    pub sample_rate: u32,
    /// Interleaved samples, whatever the channel count.
    pub samples: Vec<i16>,
    pub channels: u16,
}

impl Wav {
    /// Sample frames (one per channel group), i.e. duration × rate.
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }

    pub fn seconds(&self) -> f64 {
        self.frames() as f64 / self.sample_rate.max(1) as f64
    }

    /// Downmix to mono by averaging pairs.  The deployed ingest publishes mono already; the path
    /// is here so a stereo upload cannot silently halve the pitch.
    pub fn mono(&self) -> Vec<i16> {
        if self.channels <= 1 {
            return self.samples.clone();
        }
        let ch = self.channels as usize;
        self.samples
            .chunks(ch)
            .map(|c| {
                let sum: i32 = c.iter().map(|&s| s as i32).sum();
                (sum / ch as i32) as i16
            })
            .collect()
    }
}

/// Parse a RIFF/WAVE with a `fmt ` chunk of PCM (format 1, 16-bit) and a `data`
/// chunk.  Chunks before `fmt ` or between `fmt ` and `data` (a `LIST` of metadata,
/// the demuxer's habit) are skipped by size, which is the only safe way to walk it.
pub fn parse(bytes: &[u8]) -> Result<Wav, String> {
    let u16le = |b: &[u8], at: usize| -> u16 { u16::from_le_bytes([b[at], b[at + 1]]) };
    let u32le = |b: &[u8], at: usize| -> u32 {
        u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
    };
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".to_string());
    }
    let mut at = 12;
    let mut rate = 0u32;
    let mut channels = 0u16;
    let mut data: Option<&[u8]> = None;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32le(bytes, at + 4) as usize;
        let body = at + 8;
        if body + size > bytes.len() {
            // A truncated final chunk: take what is there rather than refusing,
            // because a browser that closed mid-upload still sent real audio.
            match id {
                b"data" if rate > 0 => {
                    data = Some(&bytes[body..]);
                    break;
                }
                _ => return Err("WAV chunk runs past the end of the file".to_string()),
            }
        }
        match id {
            b"fmt " => {
                if size < 16 {
                    return Err("fmt chunk is too short to hold a PCM header".to_string());
                }
                let format = u16le(bytes, body);
                if format != 1 {
                    return Err(format!(
                        "WAV format {format} is not PCM16 (this service accepts only 16-bit \
                         integer PCM)"
                    ));
                }
                channels = u16le(bytes, body + 2);
                rate = u32le(bytes, body + 4);
                let bits = u16le(bytes, body + 14);
                if bits != 16 {
                    return Err(format!("WAV is {bits}-bit; only 16-bit PCM is accepted"));
                }
                if channels == 0 || channels > 8 {
                    return Err(format!("implausible channel count {channels}"));
                }
            }
            b"data" if rate > 0 => {
                data = Some(&bytes[body..body + size]);
                break;
            }
            _ => {}
        }
        // Chunks are word-aligned on disk; a size that is odd carries a pad byte
        // nobody will read, and skipping it keeps the next id from being garbage.
        at = body + size + (size % 2);
    }
    if rate == 0 {
        return Err("no fmt chunk before the data chunk".to_string());
    }
    let data = data.ok_or_else(|| "no data chunk".to_string())?;
    let mut samples = Vec::with_capacity(data.len() / 2);
    for pair in data.as_chunks::<2>().0 {
        samples.push(i16::from_le_bytes(*pair));
    }
    Ok(Wav {
        sample_rate: rate,
        samples,
        channels,
    })
}

/// Write a mono PCM16 WAV: the 44-byte canonical header, then the samples.
///
/// `seconds` is in the name of clarity only; the byte counts are derived, so a
/// caller cannot lie about the length.
pub fn mono16(samples: &[i16], sample_rate: u32) -> Vec<u8> {
    let data_len = samples.len() * 2;
    let mut out = Vec::with_capacity(44 + data_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Linear interpolation between `from` and `to`.
///
/// No anti-alias filter.  The only real use is 22 050 → 16 000 Hz on speech that is
/// about to go through a camera's 20 mm piezo or a laptop speaker, where the
/// aliasing products land above the band those transducers reproduce anyway.  If
/// this ever feeds headphones or a recording, put a low-pass in front of it and say
/// so here.
pub fn resample(samples: &[i16], from: u32, to: u32) -> Vec<i16> {
    if from == to || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = from as f64 / to as f64;
    let out_len = ((samples.len() as f64) / ratio).ceil() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src = i as f64 * ratio;
        let a = src.floor() as usize;
        let b = (a + 1).min(samples.len() - 1);
        let t = src - a as f64;
        let v = samples[a] as f64 * (1.0 - t) + samples[b] as f64 * t;
        out.push(clamp16(v));
    }
    out
}

/// Hard-clip a mixed or gained signal to the i16 range.
pub fn clamp16(v: f64) -> i16 {
    if v > i16::MAX as f64 {
        i16::MAX
    } else if v < i16::MIN as f64 {
        i16::MIN
    } else {
        v as i16
    }
}

/// Keep the newest `keep` WAVs in `dir`, and nothing else.
///
/// Shared by the gateway (which keeps a copy of everything it said out loud, so "what did it
/// tell the room" has an answer that is not a guess) and by the silent sink (which keeps the
/// answers it refused to play). Two copies of a retention rule is two rules, and the failure is
/// a state directory on a box full of databases that grows without anyone noticing.
pub fn prune_wavs(dir: &std::path::Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<_> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.ends_with(".wav") {
                return None;
            }
            let mtime = e
                .path()
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            Some((mtime, e.path()))
        })
        .collect();
    if files.len() <= keep {
        return;
    }
    files.sort_by_key(|(t, _)| *t);
    for (_, path) in files.iter().take(files.len() - keep) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(n: usize) -> Vec<i16> {
        (0..n).map(|i| ((i % 512) as i16) - 256).collect()
    }

    #[test]
    fn a_written_file_reads_back_identically() {
        let s = ramp(3200); // 0.2 s at 16 kHz
        let bytes = mono16(&s, SAMPLE_RATE);
        assert_eq!(bytes.len(), 44 + s.len() * 2);
        let w = parse(&bytes).unwrap();
        assert_eq!(w.sample_rate, SAMPLE_RATE);
        assert_eq!(w.channels, 1);
        assert_eq!(w.samples, s);
        assert!((w.seconds() - 0.2).abs() < 1e-6);
    }

    #[test]
    fn skips_the_chunks_a_real_encoder_puts_before_fmt_and_data() {
        let s = ramp(640);
        let mut bytes = Vec::new();
        let body = mono16(&s, 22_050);
        // RIFF header, then a LIST chunk, then fmt/data from a real encode.
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&((36 + body.len() + 8 + 6) as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"LIST");
        bytes.extend_from_slice(&6u32.to_le_bytes());
        bytes.extend_from_slice(b"INFOIS");
        bytes.extend_from_slice(&body[12..]);
        let w = parse(&bytes).unwrap();
        assert_eq!(w.sample_rate, 22_050);
        assert_eq!(w.samples, s);
    }

    #[test]
    fn refuses_the_formats_it_cannot_handle_rather_than_guessing() {
        let mut bytes = mono16(&ramp(8), SAMPLE_RATE);
        bytes[20..22].copy_from_slice(&3u16.to_le_bytes()); // IEEE float (fmt body + 0)
        assert!(parse(&bytes).unwrap_err().contains("not PCM16"));
        let mut bytes = mono16(&ramp(8), SAMPLE_RATE);
        bytes[34..36].copy_from_slice(&8u16.to_le_bytes()); // 8-bit
        assert!(parse(&bytes).unwrap_err().contains("8-bit"));
        assert!(parse(b"hello, not a wav at all").is_err());
        assert!(parse(&mono16(&ramp(8), SAMPLE_RATE)[..30]).is_err(), "truncated");
    }

    #[test]
    fn a_truncated_data_chunk_still_yields_the_audio_that_arrived() {
        let mut bytes = mono16(&ramp(1000), SAMPLE_RATE);
        bytes.truncate(bytes.len() - 37); // browser hung mid-upload
        let w = parse(&bytes).unwrap();
        assert!(w.frames() > 900 && w.frames() < 1000);
    }

    #[test]
    fn resampling_preserves_duration() {
        let s = ramp(22_050); // exactly 1 s at 22 050
        let out = resample(&s, 22_050, SAMPLE_RATE);
        assert!((out.len() as f64 - 16_000.0).abs() <= 1.0, "{}", out.len());
        assert!(resample(&s, 16_000, 16_000).len() == s.len());
        assert!(resample(&[], 22_050, 16_000).is_empty());
    }

    #[test]
    fn a_tone_survives_resampling_at_the_right_frequency() {
        // 440 Hz in, 440 Hz out: if the index arithmetic were wrong the period
        // would visibly shift, and every zero crossing would move.
        let from = 22_050u32;
        let s: Vec<i16> = (0..from as usize)
            .map(|i| (3000.0 * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / from as f64).sin()) as i16)
            .collect();
        let out = resample(&s, from, SAMPLE_RATE);
        let crossings = |x: &[i16]| {
            x.windows(2)
                .filter(|w| (w[0] < 0) != (w[1] < 0))
                .count()
        };
        // 440 Hz sine crosses zero 880 times per second, at either rate.
        assert!((crossings(&s) as f64 - 880.0).abs() < 8.0);
        assert!((crossings(&out) as f64 - 880.0).abs() < 12.0);
    }

    #[test]
    fn stereo_downmixes_instead_of_playing_at_half_speed() {
        let mut w = Wav {
            sample_rate: SAMPLE_RATE,
            samples: vec![100, -100, 200, -200],
            channels: 2,
        };
        assert_eq!(w.frames(), 2);
        assert_eq!(w.mono(), vec![0, 0]);
        w.channels = 1;
        assert_eq!(w.frames(), 4);
    }

    #[test]
    fn the_prune_keeps_the_newest_and_nothing_else() {
        let dir = std::env::temp_dir().join(format!("voice-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..6 {
            std::fs::write(dir.join(format!("answer-{i}.wav")), b"RIFF").unwrap();
        }
        std::fs::write(dir.join("keep.txt"), b"not audio").unwrap();
        prune_wavs(&dir, 3);
        let wavs: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(wavs.iter().filter(|n| n.ends_with(".wav")).count(), 3);
        assert!(wavs.contains(&"keep.txt".to_string()), "pruned a non-WAV");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn clamps_both_ways() {
        assert_eq!(clamp16(1e9), i16::MAX);
        assert_eq!(clamp16(-1e9), i16::MIN);
        assert_eq!(clamp16(12.7), 12);
    }
}
