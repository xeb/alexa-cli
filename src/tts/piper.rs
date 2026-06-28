use crate::audio::{f32_to_i16, resample_to_16k};
use crate::config::Config;
use crate::tts::TtsBackend;
use anyhow::{Context, Result};
use sherpa_onnx::{
    GenerationConfig, OfflineTts, OfflineTtsConfig, OfflineTtsModelConfig, OfflineTtsVitsModelConfig,
};
use std::path::{Path, PathBuf};
use std::process::Command;

const VOICE: &str = "vits-piper-en_US-lessac-low";
const TARBALL_BASE: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models";

fn voice_tarball_url() -> String {
    format!("{TARBALL_BASE}/{VOICE}.tar.bz2")
}

struct VoiceAssets {
    model: String,
    tokens: String,
    data_dir: String,
}

fn voice_assets(dir: &Path) -> VoiceAssets {
    VoiceAssets {
        model: dir
            .join("en_US-lessac-low.onnx")
            .to_string_lossy()
            .into_owned(),
        tokens: dir.join("tokens.txt").to_string_lossy().into_owned(),
        data_dir: dir.join("espeak-ng-data").to_string_lossy().into_owned(),
    }
}

fn ensure_voice() -> Result<PathBuf> {
    let models = Config::models_dir();
    let dir = models.join(VOICE);
    if dir.join("en_US-lessac-low.onnx").exists() {
        return Ok(dir);
    }
    std::fs::create_dir_all(&models)?;
    let url = voice_tarball_url();
    eprintln!("downloading piper voice {VOICE} ...");
    let bytes = reqwest::blocking::get(&url)
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.bytes())
        .with_context(|| format!("downloading {url}"))?;
    let tarball = models.join(format!("{VOICE}.tar.bz2"));
    std::fs::write(&tarball, &bytes)?;
    let status = Command::new("tar")
        .arg("xjf")
        .arg(&tarball)
        .arg("-C")
        .arg(&models)
        .status()
        .context("extracting voice tarball (need the `tar` command)")?;
    if !status.success() {
        anyhow::bail!("tar extraction failed for {VOICE}");
    }
    let _ = std::fs::remove_file(&tarball);
    Ok(dir)
}

pub struct Piper {
    dir: PathBuf,
}

impl Piper {
    pub fn new() -> Result<Piper> {
        Ok(Piper {
            dir: ensure_voice()?,
        })
    }
}

impl TtsBackend for Piper {
    fn synth(&self, text: &str) -> Result<Vec<i16>> {
        let a = voice_assets(&self.dir);
        let config = OfflineTtsConfig {
            model: OfflineTtsModelConfig {
                vits: OfflineTtsVitsModelConfig {
                    model: Some(a.model),
                    tokens: Some(a.tokens),
                    data_dir: Some(a.data_dir),
                    ..Default::default()
                },
                num_threads: 1,
                ..Default::default()
            },
            max_num_sentences: 1,
            ..Default::default()
        };
        let tts = OfflineTts::create(&config)
            .context("creating sherpa OfflineTts (check voice assets exist)")?;
        let audio = tts
            .generate_with_config(
                text,
                &GenerationConfig {
                    sid: 0,
                    speed: 1.0,
                    ..Default::default()
                },
                None::<fn(&[f32], f32) -> bool>,
            )
            .context("sherpa/piper synthesis returned no audio")?;
        let rate = audio.sample_rate() as u32;
        let samples = if rate == 16000 {
            audio.samples().to_vec()
        } else {
            resample_to_16k(audio.samples(), rate)?
        };
        Ok(f32_to_i16(&samples))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn voice_tarball_url_wellformed() {
        let url = voice_tarball_url();
        assert!(url.starts_with("https://"));
        assert!(url.ends_with("vits-piper-en_US-lessac-low.tar.bz2"));
    }

    #[test]
    fn voice_asset_layout() {
        let a = voice_assets(&PathBuf::from("/models/vits-piper-en_US-lessac-low"));
        assert!(a.model.ends_with("en_US-lessac-low.onnx"));
        assert!(a.tokens.ends_with("tokens.txt"));
        assert!(a.data_dir.ends_with("espeak-ng-data"));
    }
}
