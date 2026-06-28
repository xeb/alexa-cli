use crate::audio::resample_to_16k;
use crate::tts::TtsBackend;
use anyhow::{Context, Result};
use std::io::Cursor;
use std::process::Command;

pub struct Espeak;

pub fn wav_bytes_to_16k_mono_i16(wav: &[u8]) -> Result<Vec<i16>> {
    let reader = hound::WavReader::new(Cursor::new(wav)).context("reading espeak WAV")?;
    let spec = reader.spec();
    let channels = spec.channels as usize;
    // espeak emits i16 PCM
    let samples_i16: Vec<i16> = reader.into_samples::<i16>().collect::<Result<_, _>>()?;
    // to f32, downmix, resample, back to i16
    let f: Vec<f32> = samples_i16.iter().map(|&s| s as f32 / 32768.0).collect();
    let mono = crate::audio::downmix_to_mono(&f, channels);
    let resampled = resample_to_16k(&mono, spec.sample_rate)?;
    Ok(crate::audio::f32_to_i16(&resampled))
}

impl TtsBackend for Espeak {
    fn synth(&self, text: &str) -> Result<Vec<i16>> {
        let output = Command::new("espeak-ng")
            .args(["--stdout"])
            .arg(text)
            .output()
            .context("running espeak-ng (is it installed? `sudo apt install espeak-ng`)")?;
        if !output.status.success() {
            anyhow::bail!("espeak-ng failed: {}", String::from_utf8_lossy(&output.stderr));
        }
        wav_bytes_to_16k_mono_i16(&output.stdout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_22k_wav_to_16k_mono_i16() {
        let wav = include_bytes!("../../tests/fixtures/espeak.wav");
        let pcm = wav_bytes_to_16k_mono_i16(wav).unwrap();
        // 0.3s @16k ~= 4800 samples; allow slack
        assert!(pcm.len() > 3500 && pcm.len() < 6500, "got {}", pcm.len());
    }
}
