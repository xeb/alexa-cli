use crate::audio::{downmix_to_mono, resample_to_16k};
use crate::config::Config;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub fn decode_mp3_to_16k_mono(mp3: &[u8]) -> Result<Vec<f32>> {
    let mss = MediaSourceStream::new(
        Box::new(std::io::Cursor::new(mp3.to_vec())),
        Default::default(),
    );
    let mut hint = Hint::new();
    hint.with_extension("mp3");
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .context("probing mp3")?;
    let mut format = probed.format;
    let track = format.default_track().context("no default track")?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .context("making mp3 decoder")?;

    let mut interleaved: Vec<f32> = Vec::new();
    let mut channels = 1usize;
    let mut rate = 0u32;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break
            }
            Err(symphonia::core::errors::Error::ResetRequired) => break,
            Err(e) => return Err(e.into()),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(e) => return Err(e.into()),
        };
        let spec = *decoded.spec();
        rate = spec.rate;
        channels = spec.channels.count();
        let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buf.copy_interleaved_ref(decoded);
        interleaved.extend_from_slice(buf.samples());
    }

    if interleaved.is_empty() || rate == 0 {
        anyhow::bail!("decoded no audio from mp3");
    }
    let mono = downmix_to_mono(&interleaved, channels);
    resample_to_16k(&mono, rate)
}

fn model_filename(model: &str) -> String {
    format!("ggml-{model}.bin")
}

pub fn ensure_model(model: &str) -> Result<PathBuf> {
    let dir = Config::models_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(model_filename(model));
    if path.exists() {
        return Ok(path);
    }
    let url = format!(
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}",
        model_filename(model)
    );
    eprintln!("downloading whisper model {model} ...");
    let bytes = reqwest::blocking::get(&url)
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.bytes())
        .with_context(|| format!("downloading {url}"))?;
    std::fs::write(&path, &bytes).context("writing model file")?;
    Ok(path)
}

pub fn transcribe_samples(samples_16k_mono: &[f32], model_path: &Path) -> Result<String> {
    let ctx = WhisperContext::new_with_params(
        model_path.to_str().context("model path not utf8")?,
        WhisperContextParameters::default(),
    )
    .context("loading whisper model")?;
    let mut state = ctx.create_state().context("creating whisper state")?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_language(Some("en"));
    state
        .full(params, samples_16k_mono)
        .context("running whisper")?;

    let n = state.full_n_segments(); // i32, not a Result
    let mut out = String::new();
    for i in 0..n {
        if let Some(seg) = state.get_segment(i) {
            out.push_str(&seg.to_str_lossy().unwrap_or_default());
        }
    }
    Ok(out.trim().to_string())
}

pub fn transcribe_mp3(mp3: &[u8], config: &Config) -> Result<String> {
    let samples = decode_mp3_to_16k_mono(mp3)?;
    let model_path = ensure_model(&config.model)?;
    transcribe_samples(&samples, &model_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_mp3_to_16k_mono() {
        let mp3 = include_bytes!("../tests/fixtures/sample.mp3");
        let samples = decode_mp3_to_16k_mono(mp3).unwrap();
        // ~0.5s at 16kHz -> a few thousand samples; allow generous bounds
        assert!(
            samples.len() > 4000 && samples.len() < 12000,
            "got {}",
            samples.len()
        );
        // sine should have non-trivial amplitude
        let peak = samples.iter().cloned().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(peak > 0.1, "peak {peak}");
    }

    #[test]
    fn model_filename_maps_correctly() {
        assert_eq!(model_filename("base.en"), "ggml-base.en.bin");
        assert_eq!(model_filename("tiny.en"), "ggml-tiny.en.bin");
        assert_eq!(model_filename("small.en"), "ggml-small.en.bin");
    }

    #[test]
    #[ignore] // live: downloads model + runs whisper
    fn transcribe_sample_runs() {
        let mp3 = include_bytes!("../tests/fixtures/sample.mp3");
        let cfg = crate::config::Config::default();
        let text = transcribe_mp3(mp3, &cfg).unwrap();
        // a 440Hz sine won't produce words; just assert it returns a String without panicking
        let _ = text;
    }
}
