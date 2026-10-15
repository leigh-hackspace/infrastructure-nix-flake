//! Audio helpers: base64 PCM16 decoding, 24 kHz → 16 kHz resampling, and
//! WAV framing for whisper-server uploads.

/// Sample rate the OpenAI Realtime protocol declares for `pcm16` input.
pub const INPUT_RATE: u32 = 24_000;
/// whisper.cpp's native rate.
pub const TARGET_RATE: u32 = 16_000;

/// Decode a base64 `pcm16` payload (little-endian signed 16-bit) into samples.
pub fn pcm16_from_base64(s: &str) -> Result<Vec<i16>, String> {
    let bytes = crate::base64::decode(s)?;
    if bytes.len() % 2 != 0 {
        return Err("audio payload has odd byte length".into());
    }
    Ok(bytes
        .chunks(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect())
}

/// Linear-interpolation resample. Good enough for speech; avoids the
/// complexity of a polyphase filter while staying well within ASR accuracy.
pub fn resample(input: &[i16], from: u32, to: u32) -> Vec<i16> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let ratio = f64::from(from) / f64::from(to);
    let out_len = ((input.len() as f64) / ratio) as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = (i as f64) * ratio;
        let lo = pos.floor() as usize;
        let hi = (lo + 1).min(input.len() - 1);
        let frac = pos - lo as f64;
        let sample = f64::from(input[lo]) * (1.0 - frac) + f64::from(input[hi]) * frac;
        out.push(sample.round().clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16);
    }
    out
}

/// Wrap 16-bit mono PCM in a minimal 44-byte-header WAV (RIFF/PCM).
pub fn wav_bytes(pcm16: &[i16], rate: u32) -> Vec<u8> {
    let data_len = (pcm16.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    wav.extend_from_slice(&1u16.to_le_bytes());  // PCM
    wav.extend_from_slice(&1u16.to_le_bytes());  // mono
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 2).to_le_bytes()); // byte rate
    wav.extend_from_slice(&2u16.to_le_bytes());       // block align
    wav.extend_from_slice(&16u16.to_le_bytes());      // bits per sample
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm16 {
        wav.extend_from_slice(&s.to_le_bytes());
    }
    wav
}
