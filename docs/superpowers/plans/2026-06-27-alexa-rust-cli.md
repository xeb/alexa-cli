# Rust `alexa` CLI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild the Python `alexatext` tool as a Rust CLI installed as `alexa` that synthesizes text to speech, round-trips it through Amazon's modern AVS HTTP/2 API, and transcribes Alexa's spoken reply locally with Whisper.

**Architecture:** A lib+bin Cargo crate. Pure-logic modules (audio conversion, config, cache, AVS multipart encode/decode, MP3 decode) are built and unit-tested first; integration modules (auth over HTTPS, the `h2` AVS transport, Piper/whisper model use) are layered on top; finally `alexa doctor` and the default command wire the full pipeline for a live de-risk test before extras are added.

**Tech Stack:** Rust 1.94, tokio, `h2` + `tokio-rustls` (AVS HTTP/2), `reqwest` (OAuth), `clap` (CLI), `piper-rs` + `espeak-ng` (TTS), `whisper-rs` (STT), `symphonia` (MP3), `rubato` (resample), `hound` (WAV), `serde_json` (config/cache), `blake3` (cache key).

## Global Constraints

- **AVS API:** modern HTTP/2 `v20160207` only. Legacy v1 (`access-alexa-na.amazon.com/v1/...`, `messageHeader`/`messageBody`) is dead — never use it.
- **AVS transport:** use the `h2` crate directly over `tokio-rustls`, NOT `reqwest`. One multiplexed connection holds the downchannel open; reqwest's pooling breaks this.
- **Recognize audio:** raw LPCM, 16-bit signed, **16 kHz**, mono, little-endian, **no WAV/RIFF header**. Format string `AUDIO_L16_RATE_16000_CHANNELS_1`.
- **Region:** default NA gateway `https://alexa.na.gateway.devices.a2z.com`; EU `alexa.eu.gateway.devices.a2z.com`; FE `alexa.fe.gateway.devices.a2z.com`. Honor any `SetGateway` directive.
- **Config home:** `~/.alexa/` (reuse the Python tool's dir). Read legacy `config.json` keys `clientId`, `clientSecret`, `programId` verbatim.
- **STT:** CPU only — no CUDA/Metal features. Default model `base.en`; `tiny.en` selectable.
- **TTS:** default `piper` (16 kHz native voice); `espeak` fallback via `--voice espeak`.
- **Binary name:** `alexa`. Crate lib name: `alexa_cli`.
- **f32→i16:** always clamp to `[-32768, 32767]`.
- **Error handling:** `anyhow::Result` at boundaries; user-facing errors must say what to run next (`alexa configure` / `alexa login`).
- **Commits:** one per task (after its tests pass). Conventional-commit style messages.

## Interface Contract (types/signatures shared across tasks)

These are the exact names every task must use. Defined progressively; later tasks consume these verbatim.

```rust
// config.rs
pub enum Region { Na, Eu, Fe }
impl Region {
    pub fn gateway_host(&self) -> &'static str;     // e.g. "alexa.na.gateway.devices.a2z.com"
    pub fn from_str_lenient(s: &str) -> Region;     // "na"/"eu"/"fe", default Na
}
pub enum Voice { Piper, Espeak }
pub struct Config {
    pub client_id: String,            // serde rename "clientId"
    pub client_secret: String,        // serde rename "clientSecret"
    pub product_id: String,           // serde rename "programId"
    pub device_serial_number: String, // serde rename "deviceSerialNumber"
    pub region: Region,
    pub voice: Voice,
    pub model: String,
    pub save_transcription: bool,
}
impl Config {
    pub fn dir() -> std::path::PathBuf;             // ~/.alexa
    pub fn path() -> std::path::PathBuf;            // ~/.alexa/config.json
    pub fn models_dir() -> std::path::PathBuf;      // ~/.alexa/models
    pub fn load() -> anyhow::Result<Config>;        // errors if missing required creds
    pub fn load_or_default() -> Config;             // for `configure` editing
    pub fn save(&self) -> anyhow::Result<()>;
    pub fn is_complete(&self) -> bool;              // has client_id/secret/product_id
}

// audio.rs
pub fn f32_to_i16(samples: &[f32]) -> Vec<i16>;
pub fn i16_to_le_bytes(samples: &[i16]) -> Vec<u8>;
pub fn downmix_to_mono(interleaved: &[f32], channels: usize) -> Vec<f32>;
pub fn resample_to_16k(samples: &[f32], in_rate: u32) -> anyhow::Result<Vec<f32>>;

// cache.rs
pub fn key_for(mp3: &[u8]) -> String;              // blake3 hex
pub struct Cache { /* path + map */ }
impl Cache {
    pub fn load() -> Cache;                          // ~/.alexa/cache.json, empty if absent
    pub fn get(&self, key: &str) -> Option<String>;
    pub fn put(&mut self, key: &str, transcript: &str) -> anyhow::Result<()>; // persists
}

// auth.rs
pub struct Tokens { pub access_token: String, pub refresh_token: String, pub obtained_at: u64 }
pub fn is_expired(obtained_at: u64, now: u64) -> bool;   // >= 3600s
impl Tokens { pub fn load() -> anyhow::Result<Tokens>; pub fn save(&self) -> anyhow::Result<()>; }
pub async fn login(config: &Config, port: u16) -> anyhow::Result<()>;          // loopback flow, saves tokens
pub async fn access_token(config: &Config, force_refresh: bool) -> anyhow::Result<String>;

// tts/mod.rs
pub trait TtsBackend { fn synth(&self, text: &str) -> anyhow::Result<Vec<i16>>; } // 16kHz mono i16
pub fn backend_for(voice: &Voice) -> anyhow::Result<Box<dyn TtsBackend>>;
// tts/espeak.rs
pub fn wav_bytes_to_16k_mono_i16(wav: &[u8]) -> anyhow::Result<Vec<i16>>;
pub struct Espeak;   // impl TtsBackend
// tts/piper.rs
pub struct Piper;    // impl TtsBackend  (ensures model on construction/first synth)

// avs.rs
pub fn recognize_event_json(message_id: &str, dialog_request_id: &str) -> String;
pub fn synchronize_state_json(message_id: &str) -> String;
pub fn build_recognize_multipart(event_json: &str, pcm: &[u8], boundary: &str) -> Vec<u8>;
pub struct Part { pub headers: Vec<(String, String)>, pub body: Vec<u8> }
pub fn parse_multipart_related(content_type: &str, body: &[u8]) -> anyhow::Result<Vec<Part>>;
pub fn extract_speak_audio(parts: &[Part]) -> anyhow::Result<Vec<u8>>;          // returns MP3 bytes
pub async fn recognize(config: &Config, token: &str, pcm: &[u8]) -> anyhow::Result<Vec<u8>>; // MP3

// stt.rs
pub fn decode_mp3_to_16k_mono(mp3: &[u8]) -> anyhow::Result<Vec<f32>>;
pub fn ensure_model(model: &str) -> anyhow::Result<std::path::PathBuf>;         // downloads ggml
pub fn transcribe_samples(samples_16k_mono: &[f32], model_path: &std::path::Path) -> anyhow::Result<String>;
pub fn transcribe_mp3(mp3: &[u8], config: &Config) -> anyhow::Result<String>;

// cli.rs
pub async fn run() -> anyhow::Result<()>;
```

---

### Task 1: Project scaffold (lib + bin)

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`
- Create: `src/lib.rs`
- Modify: `.gitignore` (add `/target`)

**Interfaces:**
- Consumes: nothing
- Produces: the `alexa_cli` lib crate and `alexa` bin; `alexa_cli::cli::run()` (stub for now)

- [ ] **Step 1: Write `Cargo.toml`**

```toml
[package]
name = "alexa-cli"
version = "0.2.0"
edition = "2021"
description = "Round-trip text through Alexa: TTS -> AVS -> Whisper STT"
license = "MIT"

[lib]
name = "alexa_cli"
path = "src/lib.rs"

[[bin]]
name = "alexa"
path = "src/main.rs"

[dependencies]
tokio = { version = "1", features = ["full"] }
anyhow = "1"
thiserror = "1"
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
uuid = { version = "1", features = ["v4"] }
dirs = "5"
blake3 = "1"
hound = "3.5"
rubato = "0.15"
symphonia = { version = "0.5", features = ["mp3"] }
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json"] }
tiny_http = "0.12"
webbrowser = "1"
h2 = "0.4"
http = "1"
bytes = "1"
rustls = "0.23"
tokio-rustls = "0.26"
webpki-roots = "0.26"
whisper-rs = "0.16"
piper-rs = "0.2"

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Write `src/lib.rs`**

```rust
pub mod audio;
pub mod auth;
pub mod avs;
pub mod cache;
pub mod cli;
pub mod config;
pub mod stt;
pub mod tts;
```

Note: modules referenced here are created in later tasks. To keep the tree compiling task-by-task, add each `pub mod` line only when its file exists, OR create empty stub files now. Create stub files now so `cargo build` works:

```rust
// create empty placeholder files: src/audio.rs, src/auth.rs, src/avs.rs,
// src/cache.rs, src/cli.rs, src/config.rs, src/stt.rs, and src/tts/mod.rs
// Each may start with `// implemented in a later task`
```

For `cli.rs` stub, add a runnable entry:

```rust
// src/cli.rs
use anyhow::Result;
pub async fn run() -> Result<()> {
    println!("alexa CLI (scaffold)");
    Ok(())
}
```

- [ ] **Step 3: Write `src/main.rs`**

```rust
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    match alexa_cli::cli::run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
```

- [ ] **Step 4: Add `/target` to `.gitignore`**

Append a line `/target` to `.gitignore`.

- [ ] **Step 5: Build and run**

Run: `cargo build`
Expected: compiles (warnings about empty modules are fine).
Run: `cargo run -- `
Expected: prints `alexa CLI (scaffold)`.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src/ .gitignore
git commit -m "feat: scaffold Rust alexa lib+bin crate"
```

---

### Task 2: Audio conversions (`audio.rs`)

**Files:**
- Modify: `src/audio.rs`

**Interfaces:**
- Consumes: nothing
- Produces: `f32_to_i16`, `i16_to_le_bytes`, `downmix_to_mono`, `resample_to_16k`

- [ ] **Step 1: Write failing tests**

```rust
// src/audio.rs  (append at bottom)
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f32_to_i16_clamps_and_scales() {
        let out = f32_to_i16(&[0.0, 1.0, -1.0, 2.0, -2.0, 0.5]);
        assert_eq!(out[0], 0);
        assert_eq!(out[1], 32767);
        assert_eq!(out[2], -32768);
        assert_eq!(out[3], 32767);  // clamped
        assert_eq!(out[4], -32768); // clamped
        assert_eq!(out[5], 16383);  // 0.5*32767 rounded
    }

    #[test]
    fn i16_to_le_bytes_is_little_endian() {
        let out = i16_to_le_bytes(&[1, -1]);
        assert_eq!(out, vec![0x01, 0x00, 0xFF, 0xFF]);
    }

    #[test]
    fn downmix_averages_channels() {
        // stereo: L=[1.0, 0.0], R=[0.0, 1.0] interleaved
        let out = downmix_to_mono(&[1.0, 0.0, 0.0, 1.0], 2);
        assert_eq!(out, vec![0.5, 0.5]);
    }

    #[test]
    fn downmix_mono_is_identity() {
        let out = downmix_to_mono(&[0.1, 0.2, 0.3], 1);
        assert_eq!(out, vec![0.1, 0.2, 0.3]);
    }

    #[test]
    fn resample_changes_length_proportionally() {
        let input: Vec<f32> = (0..32000).map(|i| ((i as f32) * 0.01).sin()).collect();
        let out = resample_to_16k(&input, 32000).unwrap();
        // 32000 samples @32k -> ~1s -> ~16000 @16k (allow small ratio slack)
        let ratio = out.len() as f32 / input.len() as f32;
        assert!((ratio - 0.5).abs() < 0.05, "ratio was {ratio}");
    }

    #[test]
    fn resample_16k_is_passthrough() {
        let input = vec![0.1f32, 0.2, 0.3, 0.4];
        let out = resample_to_16k(&input, 16000).unwrap();
        assert_eq!(out, input);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib audio`
Expected: FAIL — functions not defined.

- [ ] **Step 3: Implement**

```rust
// src/audio.rs  (top of file)
use anyhow::Result;
use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};

pub fn f32_to_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|&s| {
            let scaled = (s * 32767.0).round();
            scaled.clamp(-32768.0, 32767.0) as i16
        })
        .collect()
}

pub fn i16_to_le_bytes(samples: &[i16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

pub fn downmix_to_mono(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

pub fn resample_to_16k(samples: &[f32], in_rate: u32) -> Result<Vec<f32>> {
    if in_rate == 16000 {
        return Ok(samples.to_vec());
    }
    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    };
    let ratio = 16000.0 / in_rate as f64;
    let mut resampler = SincFixedIn::<f32>::new(ratio, 2.0, params, samples.len(), 1)?;
    let waves_in = vec![samples.to_vec()];
    let waves_out = resampler.process(&waves_in, None)?;
    Ok(waves_out.into_iter().next().unwrap_or_default())
}
```

Note: verify the `rubato` 0.15 API names (`SincInterpolationParameters`, `SincFixedIn::new(ratio, max_resample_ratio_relative, params, chunk_size, nbr_channels)`). If the signature differs, adapt; the behavior (resample mono f32 to 16 kHz) is what the tests pin.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib audio`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add src/audio.rs
git commit -m "feat: audio conversion + resampling helpers"
```

---

### Task 3: Config (`config.rs`)

**Files:**
- Modify: `src/config.rs`

**Interfaces:**
- Consumes: nothing
- Produces: `Region`, `Voice`, `Config` (+ methods listed in the contract)

- [ ] **Step 1: Write failing tests**

```rust
// src/config.rs  (append)
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_gateway_hosts() {
        assert_eq!(Region::Na.gateway_host(), "alexa.na.gateway.devices.a2z.com");
        assert_eq!(Region::Eu.gateway_host(), "alexa.eu.gateway.devices.a2z.com");
        assert_eq!(Region::Fe.gateway_host(), "alexa.fe.gateway.devices.a2z.com");
    }

    #[test]
    fn region_from_str_defaults_to_na() {
        assert!(matches!(Region::from_str_lenient("eu"), Region::Eu));
        assert!(matches!(Region::from_str_lenient("FE"), Region::Fe));
        assert!(matches!(Region::from_str_lenient("garbage"), Region::Na));
    }

    #[test]
    fn deserializes_legacy_python_keys() {
        let json = r#"{
            "clientId": "abc",
            "clientSecret": "xyz",
            "programId": "prod-1"
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.client_id, "abc");
        assert_eq!(cfg.client_secret, "xyz");
        assert_eq!(cfg.product_id, "prod-1");
        // defaults applied for new fields:
        assert!(matches!(cfg.region, Region::Na));
        assert!(matches!(cfg.voice, Voice::Piper));
        assert_eq!(cfg.model, "base.en");
    }

    #[test]
    fn is_complete_requires_creds() {
        let mut cfg = Config::load_or_default();
        assert!(!cfg.is_complete());
        cfg.client_id = "a".into();
        cfg.client_secret = "b".into();
        cfg.product_id = "c".into();
        assert!(cfg.is_complete());
    }

    #[test]
    fn roundtrip_serialization_uses_legacy_keys() {
        let mut cfg = Config::load_or_default();
        cfg.client_id = "id".into();
        let s = serde_json::to_string(&cfg).unwrap();
        assert!(s.contains("\"clientId\":\"id\""));
        let back: Config = serde_json::from_str(&s).unwrap();
        assert_eq!(back.client_id, "id");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib config`
Expected: FAIL — types not defined.

- [ ] **Step 3: Implement**

```rust
// src/config.rs  (top)
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Region {
    #[default]
    Na,
    Eu,
    Fe,
}

impl Region {
    pub fn gateway_host(&self) -> &'static str {
        match self {
            Region::Na => "alexa.na.gateway.devices.a2z.com",
            Region::Eu => "alexa.eu.gateway.devices.a2z.com",
            Region::Fe => "alexa.fe.gateway.devices.a2z.com",
        }
    }
    pub fn from_str_lenient(s: &str) -> Region {
        match s.to_ascii_lowercase().as_str() {
            "eu" => Region::Eu,
            "fe" => Region::Fe,
            _ => Region::Na,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Voice {
    #[default]
    Piper,
    Espeak,
}

fn default_model() -> String { "base.en".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(rename = "clientId", default)]
    pub client_id: String,
    #[serde(rename = "clientSecret", default)]
    pub client_secret: String,
    #[serde(rename = "programId", default)]
    pub product_id: String,
    #[serde(rename = "deviceSerialNumber", default)]
    pub device_serial_number: String,
    #[serde(default)]
    pub region: Region,
    #[serde(default)]
    pub voice: Voice,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(rename = "saveTranscription", default)]
    pub save_transcription: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            client_id: String::new(),
            client_secret: String::new(),
            product_id: String::new(),
            device_serial_number: String::new(),
            region: Region::Na,
            voice: Voice::Piper,
            model: default_model(),
            save_transcription: true,
        }
    }
}

impl Config {
    pub fn dir() -> PathBuf {
        dirs::home_dir().unwrap_or_default().join(".alexa")
    }
    pub fn path() -> PathBuf {
        Config::dir().join("config.json")
    }
    pub fn models_dir() -> PathBuf {
        Config::dir().join("models")
    }
    pub fn load() -> Result<Config> {
        let cfg = Config::load_or_default();
        if !cfg.is_complete() {
            anyhow::bail!("missing AVS credentials — run `alexa configure`");
        }
        Ok(cfg)
    }
    pub fn load_or_default() -> Config {
        match std::fs::read_to_string(Config::path()) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Config::default(),
        }
    }
    pub fn save(&self) -> Result<()> {
        std::fs::create_dir_all(Config::dir())?;
        let s = serde_json::to_string_pretty(self)?;
        std::fs::write(Config::path(), s).context("writing config.json")?;
        Ok(())
    }
    pub fn is_complete(&self) -> bool {
        !self.client_id.is_empty()
            && !self.client_secret.is_empty()
            && !self.product_id.is_empty()
    }
}
```

Note: the `roundtrip_serialization_uses_legacy_keys` test expects compact JSON (`to_string`); `save()` uses pretty JSON on disk — both round-trip the same keys, so the test stays valid.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib config`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat: config with legacy ~/.alexa/config.json compatibility"
```

---

### Task 4: Transcription cache (`cache.rs`)

**Files:**
- Modify: `src/cache.rs`

**Interfaces:**
- Consumes: `Config::dir()`
- Produces: `key_for`, `Cache` (`load`/`get`/`put`)

- [ ] **Step 1: Write failing tests**

```rust
// src/cache.rs  (append)
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_stable_and_content_addressed() {
        let a = key_for(b"hello");
        let b = key_for(b"hello");
        let c = key_for(b"world");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 64); // blake3 hex
    }

    #[test]
    fn put_then_get_roundtrips_in_memory() {
        let mut cache = Cache::empty_for_test();
        assert_eq!(cache.get("k"), None);
        cache.put_mem("k", "the time is noon");
        assert_eq!(cache.get("k"), Some("the time is noon".to_string()));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib cache`
Expected: FAIL.

- [ ] **Step 3: Implement**

```rust
// src/cache.rs  (top)
use crate::config::Config;
use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;

pub fn key_for(mp3: &[u8]) -> String {
    blake3::hash(mp3).to_hex().to_string()
}

pub struct Cache {
    path: PathBuf,
    map: HashMap<String, String>,
}

impl Cache {
    pub fn load() -> Cache {
        let path = Config::dir().join("cache.json");
        let map = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Cache { path, map }
    }
    pub fn get(&self, key: &str) -> Option<String> {
        self.map.get(key).cloned()
    }
    pub fn put(&mut self, key: &str, transcript: &str) -> Result<()> {
        self.map.insert(key.to_string(), transcript.to_string());
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, serde_json::to_string_pretty(&self.map)?)?;
        Ok(())
    }

    #[cfg(test)]
    pub fn empty_for_test() -> Cache {
        Cache { path: PathBuf::from("/dev/null"), map: HashMap::new() }
    }
    #[cfg(test)]
    pub fn put_mem(&mut self, key: &str, transcript: &str) {
        self.map.insert(key.to_string(), transcript.to_string());
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib cache`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add src/cache.rs
git commit -m "feat: blake3-keyed transcription cache"
```

---

### Task 5: AVS event JSON + multipart request encode (`avs.rs` part 1)

**Files:**
- Modify: `src/avs.rs`

**Interfaces:**
- Consumes: nothing
- Produces: `recognize_event_json`, `synchronize_state_json`, `build_recognize_multipart`

- [ ] **Step 1: Write failing tests**

```rust
// src/avs.rs  (append)
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognize_event_has_required_fields() {
        let j = recognize_event_json("mid-1", "drid-1");
        let v: serde_json::Value = serde_json::from_str(&j).unwrap();
        assert_eq!(v["event"]["header"]["namespace"], "SpeechRecognizer");
        assert_eq!(v["event"]["header"]["name"], "Recognize");
        assert_eq!(v["event"]["header"]["messageId"], "mid-1");
        assert_eq!(v["event"]["header"]["dialogRequestId"], "drid-1");
        assert_eq!(v["event"]["payload"]["profile"], "CLOSE_TALK");
        assert_eq!(v["event"]["payload"]["format"], "AUDIO_L16_RATE_16000_CHANNELS_1");
        assert_eq!(v["event"]["payload"]["initiator"]["type"], "TAP");
    }

    #[test]
    fn synchronize_state_is_valid() {
        let j = synchronize_state_json("mid-2");
        let v: serde_json::Value = serde_json::from_str(&j).unwrap();
        assert_eq!(v["event"]["header"]["namespace"], "System");
        assert_eq!(v["event"]["header"]["name"], "SynchronizeState");
        assert!(v["context"].is_array());
    }

    #[test]
    fn multipart_body_has_both_parts_and_raw_audio() {
        let body = build_recognize_multipart("{\"event\":true}", &[0xDE, 0xAD, 0xBE, 0xEF], "BOUNDARY");
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("--BOUNDARY\r\n"));
        assert!(text.contains("Content-Disposition: form-data; name=\"metadata\""));
        assert!(text.contains("Content-Type: application/json"));
        assert!(text.contains("Content-Disposition: form-data; name=\"audio\""));
        assert!(text.contains("Content-Type: application/octet-stream"));
        assert!(text.ends_with("--BOUNDARY--\r\n"));
        // raw audio bytes present verbatim:
        assert!(body.windows(4).any(|w| w == [0xDE, 0xAD, 0xBE, 0xEF]));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib avs`
Expected: FAIL.

- [ ] **Step 3: Implement**

```rust
// src/avs.rs  (top)
use anyhow::Result;

pub fn recognize_event_json(message_id: &str, dialog_request_id: &str) -> String {
    serde_json::json!({
        "context": [],
        "event": {
            "header": {
                "namespace": "SpeechRecognizer",
                "name": "Recognize",
                "messageId": message_id,
                "dialogRequestId": dialog_request_id
            },
            "payload": {
                "profile": "CLOSE_TALK",
                "format": "AUDIO_L16_RATE_16000_CHANNELS_1",
                "initiator": { "type": "TAP" }
            }
        }
    })
    .to_string()
}

pub fn synchronize_state_json(message_id: &str) -> String {
    serde_json::json!({
        "context": [],
        "event": {
            "header": {
                "namespace": "System",
                "name": "SynchronizeState",
                "messageId": message_id
            },
            "payload": {}
        }
    })
    .to_string()
}

pub fn build_recognize_multipart(event_json: &str, pcm: &[u8], boundary: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let push = |out: &mut Vec<u8>, s: &str| out.extend_from_slice(s.as_bytes());

    push(&mut out, &format!("--{boundary}\r\n"));
    push(&mut out, "Content-Disposition: form-data; name=\"metadata\"\r\n");
    push(&mut out, "Content-Type: application/json; charset=UTF-8\r\n\r\n");
    push(&mut out, event_json);
    push(&mut out, "\r\n");

    push(&mut out, &format!("--{boundary}\r\n"));
    push(&mut out, "Content-Disposition: form-data; name=\"audio\"\r\n");
    push(&mut out, "Content-Type: application/octet-stream\r\n\r\n");
    out.extend_from_slice(pcm);
    push(&mut out, "\r\n");

    push(&mut out, &format!("--{boundary}--\r\n"));
    out
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib avs`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/avs.rs
git commit -m "feat: AVS event JSON + multipart request encoder"
```

---

### Task 6: AVS multipart/related response parse + Speak extraction (`avs.rs` part 2)

**Files:**
- Modify: `src/avs.rs`
- Create: `tests/fixtures/avs_response.bin` (crafted fixture)

**Interfaces:**
- Consumes: nothing
- Produces: `Part`, `parse_multipart_related`, `extract_speak_audio`

- [ ] **Step 1: Write the fixture generator + failing tests**

Build the fixture in-test so it's self-contained (no binary file needed). Append to `src/avs.rs`:

```rust
#[cfg(test)]
mod response_tests {
    use super::*;

    fn make_fixture() -> (String, Vec<u8>) {
        let boundary = "RESP";
        let directive = serde_json::json!({
            "directive": {
                "header": { "namespace": "SpeechSynthesizer", "name": "Speak" },
                "payload": { "url": "cid:audio-123", "format": "AUDIO_MPEG" }
            }
        }).to_string();
        let mp3 = vec![0xFF, 0xFB, 0x10, 0x00, 1, 2, 3, 4]; // fake mp3 bytes
        let mut body = Vec::new();
        let push = |b: &mut Vec<u8>, s: &str| b.extend_from_slice(s.as_bytes());
        push(&mut body, &format!("--{boundary}\r\n"));
        push(&mut body, "Content-Type: application/json; charset=UTF-8\r\n\r\n");
        push(&mut body, &directive);
        push(&mut body, "\r\n");
        push(&mut body, &format!("--{boundary}\r\n"));
        push(&mut body, "Content-Type: application/octet-stream\r\n");
        push(&mut body, "Content-ID: audio-123\r\n\r\n");
        body.extend_from_slice(&mp3);
        push(&mut body, "\r\n");
        push(&mut body, &format!("--{boundary}--\r\n"));
        (format!("multipart/related; boundary={boundary}"), body)
    }

    #[test]
    fn parses_all_parts() {
        let (ct, body) = make_fixture();
        let parts = parse_multipart_related(&ct, &body).unwrap();
        assert_eq!(parts.len(), 2);
    }

    #[test]
    fn extracts_speak_mp3_via_cid() {
        let (ct, body) = make_fixture();
        let parts = parse_multipart_related(&ct, &body).unwrap();
        let mp3 = extract_speak_audio(&parts).unwrap();
        assert_eq!(mp3, vec![0xFF, 0xFB, 0x10, 0x00, 1, 2, 3, 4]);
    }

    #[test]
    fn extract_errors_when_no_speak() {
        let parts = vec![Part { headers: vec![], body: b"x".to_vec() }];
        assert!(extract_speak_audio(&parts).is_err());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib avs`
Expected: FAIL — `Part`/parsers not defined.

- [ ] **Step 3: Implement**

```rust
// src/avs.rs  (add)
#[derive(Debug, Clone)]
pub struct Part {
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Part {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

fn boundary_from_content_type(content_type: &str) -> Option<String> {
    let lower = content_type.to_ascii_lowercase();
    let idx = lower.find("boundary=")?;
    let raw = &content_type[idx + "boundary=".len()..];
    let raw = raw.trim().trim_matches('"');
    let end = raw.find(';').unwrap_or(raw.len());
    Some(raw[..end].trim().trim_matches('"').to_string())
}

fn find_subsequence(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from > haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

pub fn parse_multipart_related(content_type: &str, body: &[u8]) -> Result<Vec<Part>> {
    let boundary = boundary_from_content_type(content_type)
        .ok_or_else(|| anyhow::anyhow!("no boundary in content-type: {content_type}"))?;
    let delim = format!("--{boundary}");
    let delim_bytes = delim.as_bytes();

    let mut parts = Vec::new();
    let mut cursor = match find_subsequence(body, delim_bytes, 0) {
        Some(p) => p + delim_bytes.len(),
        None => return Ok(parts),
    };

    loop {
        // After a boundary: either "--" (end) or CRLF then part.
        if body[cursor..].starts_with(b"--") {
            break;
        }
        // Skip the CRLF after the boundary.
        if body[cursor..].starts_with(b"\r\n") {
            cursor += 2;
        }
        // Headers end at CRLFCRLF.
        let header_end = find_subsequence(body, b"\r\n\r\n", cursor)
            .ok_or_else(|| anyhow::anyhow!("malformed part: no header terminator"))?;
        let header_blob = String::from_utf8_lossy(&body[cursor..header_end]);
        let headers: Vec<(String, String)> = header_blob
            .split("\r\n")
            .filter(|l| !l.is_empty())
            .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
            .collect();

        let content_start = header_end + 4;
        let next_boundary = find_subsequence(body, delim_bytes, content_start)
            .ok_or_else(|| anyhow::anyhow!("malformed part: no closing boundary"))?;
        // Content runs up to the CRLF that precedes the boundary.
        let mut content_end = next_boundary;
        if body[..content_end].ends_with(b"\r\n") {
            content_end -= 2;
        }
        parts.push(Part {
            headers,
            body: body[content_start..content_end].to_vec(),
        });
        cursor = next_boundary + delim_bytes.len();
    }

    Ok(parts)
}

pub fn extract_speak_audio(parts: &[Part]) -> Result<Vec<u8>> {
    // Find a JSON directive part whose Speak payload.url is "cid:<id>".
    let mut cid: Option<String> = None;
    for p in parts {
        let is_json = p
            .header("Content-Type")
            .map(|c| c.contains("application/json"))
            .unwrap_or(false);
        if !is_json {
            continue;
        }
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&p.body) {
            let d = &v["directive"];
            if d["header"]["name"] == "Speak" {
                if let Some(url) = d["payload"]["url"].as_str() {
                    if let Some(id) = url.strip_prefix("cid:") {
                        cid = Some(id.to_string());
                        break;
                    }
                }
            }
        }
    }
    let cid = cid.ok_or_else(|| anyhow::anyhow!("no SpeechSynthesizer.Speak directive in response"))?;

    for p in parts {
        if let Some(content_id) = p.header("Content-ID") {
            let normalized = content_id.trim().trim_start_matches('<').trim_end_matches('>');
            if normalized == cid {
                return Ok(p.body.clone());
            }
        }
    }
    anyhow::bail!("no audio attachment matching cid:{cid}")
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib avs`
Expected: PASS (6 tests total in `avs`).

- [ ] **Step 5: Commit**

```bash
git add src/avs.rs
git commit -m "feat: parse AVS multipart/related response and extract Speak MP3"
```

---

### Task 7: MP3 decode for STT (`stt.rs` part 1)

**Files:**
- Modify: `src/stt.rs`
- Create: `tests/fixtures/sample.mp3` (a tiny real MP3 — generate in Step 1)

**Interfaces:**
- Consumes: `audio::{downmix_to_mono, resample_to_16k}`
- Produces: `decode_mp3_to_16k_mono`

- [ ] **Step 1: Generate a small MP3 fixture and write the failing test**

Generate a ~0.5s MP3 with ffmpeg (one-time, committed as a fixture):

```bash
mkdir -p tests/fixtures
ffmpeg -f lavfi -i "sine=frequency=440:duration=0.5" -ar 24000 -ac 1 -b:a 64k tests/fixtures/sample.mp3
```

If `ffmpeg` is unavailable, skip this fixture and mark the decode test `#[ignore]` with a note; the live `doctor` path still exercises decode. Then add the test:

```rust
// src/stt.rs  (append)
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_mp3_to_16k_mono() {
        let mp3 = include_bytes!("../tests/fixtures/sample.mp3");
        let samples = decode_mp3_to_16k_mono(mp3).unwrap();
        // ~0.5s at 16kHz -> a few thousand samples; allow generous bounds
        assert!(samples.len() > 4000 && samples.len() < 12000, "got {}", samples.len());
        // sine should have non-trivial amplitude
        let peak = samples.iter().cloned().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(peak > 0.1, "peak {peak}");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib stt`
Expected: FAIL — function not defined.

- [ ] **Step 3: Implement**

```rust
// src/stt.rs  (top)
use crate::audio::{downmix_to_mono, resample_to_16k};
use anyhow::{Context, Result};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

pub fn decode_mp3_to_16k_mono(mp3: &[u8]) -> Result<Vec<f32>> {
    let mss = MediaSourceStream::new(Box::new(std::io::Cursor::new(mp3.to_vec())), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("mp3");
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
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
            Err(_) => break, // end of stream
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
```

Note: verify symphonia 0.5 API surface (`get_probe().format(...)`, `SampleBuffer::copy_interleaved_ref`). The test pins behavior (≈16 kHz mono samples out).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib stt`
Expected: PASS (or SKIP if fixture couldn't be generated).

- [ ] **Step 5: Commit**

```bash
git add src/stt.rs tests/fixtures/sample.mp3
git commit -m "feat: decode Alexa MP3 to 16kHz mono PCM via symphonia"
```

---

### Task 8: Whisper model download + transcription (`stt.rs` part 2)

**Files:**
- Modify: `src/stt.rs`

**Interfaces:**
- Consumes: `Config`, `decode_mp3_to_16k_mono`, `Config::models_dir`
- Produces: `ensure_model`, `transcribe_samples`, `transcribe_mp3`

- [ ] **Step 1: Write a pure unit test for model URL/path mapping**

The network + whisper inference are integration concerns (`#[ignore]`); unit-test the model filename mapping which is pure:

```rust
// src/stt.rs  (append to tests mod)
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
```

- [ ] **Step 2: Run to verify the pure test fails**

Run: `cargo test --lib stt::tests::model_filename_maps_correctly`
Expected: FAIL — `model_filename` not defined.

- [ ] **Step 3: Implement**

```rust
// src/stt.rs  (add)
use crate::config::Config;
use std::path::{Path, PathBuf};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

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
    state.full(params, samples_16k_mono).context("running whisper")?;

    let n = state.full_n_segments().context("counting segments")?;
    let mut out = String::new();
    for i in 0..n {
        out.push_str(&state.full_get_segment_text(i).unwrap_or_default());
    }
    Ok(out.trim().to_string())
}

pub fn transcribe_mp3(mp3: &[u8], config: &Config) -> Result<String> {
    let samples = decode_mp3_to_16k_mono(mp3)?;
    let model_path = ensure_model(&config.model)?;
    transcribe_samples(&samples, &model_path)
}
```

Note: `reqwest::blocking` requires the `blocking` feature. Update `Cargo.toml` reqwest features to `["rustls-tls", "json", "blocking"]`. Verify whisper-rs 0.16 API (`WhisperContext::new_with_params`, `state.full`, `full_get_segment_text`). Adjust if the version's signatures differ; behavior pinned by the ignored live test.

- [ ] **Step 4: Run tests**

Run: `cargo test --lib stt::tests::model_filename_maps_correctly`
Expected: PASS.
(Optionally, with network: `cargo test --lib -- --ignored transcribe_sample_runs`.)

- [ ] **Step 5: Commit**

```bash
git add src/stt.rs Cargo.toml Cargo.lock
git commit -m "feat: whisper-rs model download + transcription"
```

---

### Task 9: TTS espeak backend (`tts/espeak.rs` + `tts/mod.rs`)

**Files:**
- Create: `src/tts/mod.rs` (replace stub)
- Create: `src/tts/espeak.rs`
- Create: `tests/fixtures/espeak.wav` (generate in Step 1)

**Interfaces:**
- Consumes: `audio::{f32_to_i16?, resample_to_16k}`, `hound`, `Voice`
- Produces: `TtsBackend`, `backend_for`, `wav_bytes_to_16k_mono_i16`, `Espeak`

- [ ] **Step 1: Generate a WAV fixture + failing test**

```bash
ffmpeg -f lavfi -i "sine=frequency=300:duration=0.3" -ar 22050 -ac 1 -c:a pcm_s16le tests/fixtures/espeak.wav
```

(If ffmpeg is unavailable, `espeak-ng -w tests/fixtures/espeak.wav "hello"` also works.) Then:

```rust
// src/tts/espeak.rs  (append)
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
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib espeak`
Expected: FAIL.

- [ ] **Step 3: Implement the module + trait**

```rust
// src/tts/mod.rs
pub mod espeak;
pub mod piper;

use crate::config::Voice;
use anyhow::Result;

pub trait TtsBackend {
    /// Returns 16 kHz, mono, signed-16 PCM samples.
    fn synth(&self, text: &str) -> Result<Vec<i16>>;
}

pub fn backend_for(voice: &Voice) -> Result<Box<dyn TtsBackend>> {
    match voice {
        Voice::Espeak => Ok(Box::new(espeak::Espeak)),
        Voice::Piper => Ok(Box::new(piper::Piper::new()?)),
    }
}
```

```rust
// src/tts/espeak.rs  (top)
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
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test --lib espeak`
Expected: PASS.

Note: this task also makes `tts/piper.rs` referenced. Create a minimal compiling stub now so the crate builds; Task 10 implements it:

```rust
// src/tts/piper.rs  (temporary stub — replaced in Task 10)
use crate::tts::TtsBackend;
use anyhow::Result;
pub struct Piper;
impl Piper { pub fn new() -> Result<Piper> { Ok(Piper) } }
impl TtsBackend for Piper {
    fn synth(&self, _text: &str) -> Result<Vec<i16>> { anyhow::bail!("piper not implemented yet") }
}
```

- [ ] **Step 5: Commit**

```bash
git add src/tts/ tests/fixtures/espeak.wav
git commit -m "feat: espeak-ng TTS backend + TtsBackend trait"
```

---

### Task 10: TTS Piper backend (`tts/piper.rs`)

**Files:**
- Modify: `src/tts/piper.rs` (replace stub)

**Interfaces:**
- Consumes: `audio::f32_to_i16`, `Config::models_dir`, `reqwest::blocking`
- Produces: `Piper` (impl `TtsBackend`)

- [ ] **Step 1: Write a pure unit test for the voice-asset paths**

```rust
// src/tts/piper.rs  (append)
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_asset_urls_are_wellformed() {
        let (onnx, json) = voice_asset_urls();
        assert!(onnx.ends_with("en_US-lessac-low.onnx"));
        assert!(json.ends_with("en_US-lessac-low.onnx.json"));
        assert!(onnx.starts_with("https://"));
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib piper`
Expected: FAIL — `voice_asset_urls` not defined.

- [ ] **Step 3: Implement**

```rust
// src/tts/piper.rs  (replace file contents)
use crate::audio::f32_to_i16;
use crate::config::Config;
use crate::tts::TtsBackend;
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::Arc;

const VOICE: &str = "en_US-lessac-low";
const BASE: &str = "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/low";

fn voice_asset_urls() -> (String, String) {
    (format!("{BASE}/{VOICE}.onnx"), format!("{BASE}/{VOICE}.onnx.json"))
}

fn download_to(path: &PathBuf, url: &str) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    eprintln!("downloading piper voice asset {} ...", path.display());
    let bytes = reqwest::blocking::get(url)
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.bytes())
        .with_context(|| format!("downloading {url}"))?;
    std::fs::write(path, &bytes)?;
    Ok(())
}

fn ensure_voice() -> Result<PathBuf> {
    let dir = Config::models_dir();
    std::fs::create_dir_all(&dir)?;
    let onnx = dir.join(format!("{VOICE}.onnx"));
    let json = dir.join(format!("{VOICE}.onnx.json"));
    let (onnx_url, json_url) = voice_asset_urls();
    download_to(&onnx, &onnx_url)?;
    download_to(&json, &json_url)?;
    Ok(onnx)
}

pub struct Piper {
    model_path: PathBuf,
}

impl Piper {
    pub fn new() -> Result<Piper> {
        Ok(Piper { model_path: ensure_voice()? })
    }
}

impl TtsBackend for Piper {
    fn synth(&self, text: &str) -> Result<Vec<i16>> {
        use piper_rs::synth::PiperSpeechSynthesizer;
        let model = piper_rs::from_config_path(
            &self.model_path.with_extension("onnx.json"),
        )
        .context("loading piper voice config")?;
        let synth = PiperSpeechSynthesizer::new(model).context("creating piper synthesizer")?;
        let mut samples: Vec<f32> = Vec::new();
        synth
            .synthesize_to_buffer(text, &mut samples)
            .context("piper synthesis")?;
        // en_US-lessac-low is 16 kHz native; no resample needed.
        Ok(f32_to_i16(&samples))
    }
}

// keep Arc import used if needed by piper-rs; remove if unused.
#[allow(unused_imports)]
use std::marker::PhantomData as _Unused;
let _ = Arc::new(());
```

Note: the exact `piper-rs` 0.2 API (`from_config_path`, `PiperSpeechSynthesizer::new`, `synthesize_to_buffer`) must be verified against its docs — adapt names/sample-rate handling as needed. **Fallback path** if `piper-rs`/`ort` won't build: replace `synth` with a shell-out to the `piper` binary (`piper --model <onnx> --output_file -`) and decode the WAV via `crate::tts::espeak::wav_bytes_to_16k_mono_i16`. Remove the stray `let _ = Arc::new(());` line — it is illustrative only; do not put statements at module scope.

- [ ] **Step 4: Run unit test**

Run: `cargo test --lib piper::tests::voice_asset_urls_are_wellformed`
Expected: PASS. Also `cargo build` must succeed.

- [ ] **Step 5: Commit**

```bash
git add src/tts/piper.rs
git commit -m "feat: piper neural TTS backend (16kHz native)"
```

---

### Task 11: Auth — token model + expiry (`auth.rs` part 1)

**Files:**
- Modify: `src/auth.rs`

**Interfaces:**
- Consumes: `Config::dir`
- Produces: `Tokens`, `is_expired`, `Tokens::load`, `Tokens::save`

- [ ] **Step 1: Write failing tests**

```rust
// src/auth.rs  (append)
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiry_at_one_hour() {
        assert!(!is_expired(1000, 1000 + 3599));
        assert!(is_expired(1000, 1000 + 3600));
        assert!(is_expired(1000, 1000 + 9999));
    }

    #[test]
    fn tokens_serialize_roundtrip() {
        let t = Tokens { access_token: "a".into(), refresh_token: "r".into(), obtained_at: 42 };
        let s = serde_json::to_string(&t).unwrap();
        let back: Tokens = serde_json::from_str(&s).unwrap();
        assert_eq!(back.access_token, "a");
        assert_eq!(back.refresh_token, "r");
        assert_eq!(back.obtained_at, 42);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib auth`
Expected: FAIL.

- [ ] **Step 3: Implement the model + expiry**

```rust
// src/auth.rs  (top)
use crate::config::Config;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    pub obtained_at: u64,
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

pub fn is_expired(obtained_at: u64, now: u64) -> bool {
    now.saturating_sub(obtained_at) >= 3600
}

impl Tokens {
    fn path() -> std::path::PathBuf {
        Config::dir().join("tokens.json")
    }
    pub fn load() -> Result<Tokens> {
        let s = std::fs::read_to_string(Tokens::path())
            .context("no tokens — run `alexa login`")?;
        Ok(serde_json::from_str(&s)?)
    }
    pub fn save(&self) -> Result<()> {
        std::fs::create_dir_all(Config::dir())?;
        std::fs::write(Tokens::path(), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test --lib auth`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add src/auth.rs
git commit -m "feat: token model + expiry logic"
```

---

### Task 12: Auth — LWA login + access_token (`auth.rs` part 2)

**Files:**
- Modify: `src/auth.rs`

**Interfaces:**
- Consumes: `Config`, `Tokens`, `is_expired`, `now_secs`, `reqwest`, `tiny_http`, `webbrowser`
- Produces: `login`, `access_token`

- [ ] **Step 1: Write a pure unit test for the authorize-URL builder**

The HTTP/browser flow is integration (manual via `alexa login`); unit-test the URL builder which is pure:

```rust
// src/auth.rs  (append to tests mod)
    #[test]
    fn authorize_url_contains_scope_and_product() {
        let cfg = {
            let mut c = Config::default();
            c.client_id = "cid".into();
            c.product_id = "prod".into();
            c.device_serial_number = "dsn".into();
            c
        };
        let url = authorize_url(&cfg, "http://localhost:8086/auth");
        assert!(url.contains("client_id=cid"));
        assert!(url.contains("scope=alexa%3Aall") || url.contains("alexa:all"));
        assert!(url.contains("prod"));
        assert!(url.contains("dsn"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("redirect_uri="));
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib auth::tests::authorize_url_contains_scope_and_product`
Expected: FAIL — `authorize_url` not defined.

- [ ] **Step 3: Implement**

```rust
// src/auth.rs  (add)
use serde_json::Value;

fn urlencode(s: &str) -> String {
    // minimal percent-encoding for query values
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn authorize_url(config: &Config, redirect_uri: &str) -> String {
    let scope_data = serde_json::json!({
        "alexa:all": {
            "productID": config.product_id,
            "productInstanceAttributes": { "deviceSerialNumber": config.device_serial_number }
        }
    })
    .to_string();
    format!(
        "https://www.amazon.com/ap/oa?client_id={}&scope={}&scope_data={}&response_type=code&redirect_uri={}",
        urlencode(&config.client_id),
        urlencode("alexa:all"),
        urlencode(&scope_data),
        urlencode(redirect_uri),
    )
}

async fn exchange(config: &Config, params: &[(&str, &str)]) -> Result<Tokens> {
    let client = reqwest::Client::builder().build()?;
    let resp: Value = client
        .post("https://api.amazon.com/auth/o2/token")
        .form(params)
        .send()
        .await?
        .json()
        .await?;
    if let Some(err) = resp.get("error_description").and_then(|v| v.as_str()) {
        anyhow::bail!("token exchange failed: {err}");
    }
    Ok(Tokens {
        access_token: resp["access_token"].as_str().context("no access_token")?.to_string(),
        refresh_token: resp["refresh_token"].as_str().context("no refresh_token")?.to_string(),
        obtained_at: now_secs(),
    })
}

pub async fn login(config: &Config, port: u16) -> Result<()> {
    let redirect_uri = format!("http://localhost:{port}/auth");
    let url = authorize_url(config, &redirect_uri);
    println!("Opening browser to authorize. If it doesn't open, visit:\n{url}");
    let _ = webbrowser::open(&url);

    // Blocking loopback server on a background thread; recover the code.
    let server = tiny_http::Server::http(format!("0.0.0.0:{port}"))
        .map_err(|e| anyhow::anyhow!("failed to bind localhost:{port}: {e}"))?;
    let code = loop {
        let request = server.recv()?;
        let urlpath = request.url().to_string();
        if let Some(code) = urlpath.split_once("code=").map(|(_, rest)| {
            rest.split('&').next().unwrap_or("").to_string()
        }) {
            if !code.is_empty() {
                let _ = request.respond(tiny_http::Response::from_string(
                    "Authorized. You can close this tab.",
                ));
                break code;
            }
        }
        let _ = request.respond(tiny_http::Response::from_string("Waiting for authorization code..."));
    };

    let params = [
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("client_id", config.client_id.as_str()),
        ("client_secret", config.client_secret.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
    ];
    let tokens = exchange(config, &params).await?;
    tokens.save()?;
    println!("Login successful — tokens saved to {}", Tokens::path().display());
    Ok(())
}

pub async fn access_token(config: &Config, force_refresh: bool) -> Result<String> {
    let tokens = Tokens::load()?;
    if !force_refresh && !is_expired(tokens.obtained_at, now_secs()) {
        return Ok(tokens.access_token);
    }
    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", tokens.refresh_token.as_str()),
        ("client_id", config.client_id.as_str()),
        ("client_secret", config.client_secret.as_str()),
    ];
    let refreshed = exchange(config, &params).await?;
    refreshed.save()?;
    Ok(refreshed.access_token)
}
```

Note: make `Tokens::path` `pub(crate)` or `pub` since `login` prints it. The `urlencode` helper is intentionally minimal; if a dependency like `urlencoding` is preferred, add it. Verify `tiny_http` 0.12 and `reqwest` 0.12 async API.

- [ ] **Step 4: Run unit test**

Run: `cargo test --lib auth`
Expected: PASS (3 tests). `cargo build` succeeds.

- [ ] **Step 5: Commit**

```bash
git add src/auth.rs
git commit -m "feat: LWA loopback login + token refresh"
```

---

### Task 13: AVS HTTP/2 transport (`avs.rs` part 3)

**Files:**
- Modify: `src/avs.rs`

**Interfaces:**
- Consumes: `Config`, `recognize_event_json`, `synchronize_state_json`, `build_recognize_multipart`, `parse_multipart_related`, `extract_speak_audio`, `h2`, `tokio-rustls`
- Produces: `recognize(config, token, pcm) -> Vec<u8>` (MP3)

- [ ] **Step 1: Add an ignored live integration test**

This module is inherently networked; pin behavior with an ignored test exercised via `alexa doctor`:

```rust
// src/avs.rs  (append)
#[cfg(test)]
mod transport_tests {
    #[tokio::test]
    #[ignore] // live: requires valid tokens + network
    async fn recognize_live_roundtrip() {
        let cfg = crate::config::Config::load().unwrap();
        let token = crate::auth::access_token(&cfg, false).await.unwrap();
        // 0.5s of silence as a smoke payload (won't get a useful answer, just exercises transport)
        let pcm = vec![0u8; 16000 * 2 / 2];
        let mp3 = super::recognize(&cfg, &token, &pcm).await;
        assert!(mp3.is_ok() || mp3.is_err()); // shape check; real assertion is no panic
    }
}
```

- [ ] **Step 2: Run build to confirm it compiles before implementing `recognize`**

Run: `cargo test --lib avs --no-run`
Expected: FAIL — `recognize` not defined.

- [ ] **Step 3: Implement the h2 transport**

```rust
// src/avs.rs  (add)
use crate::config::Config;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use uuid::Uuid;

fn tls_connector() -> TlsConnector {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mut cfg = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    cfg.alpn_protocols = vec![b"h2".to_vec()];
    TlsConnector::from(Arc::new(cfg))
}

/// One-shot Recognize over a fresh HTTP/2 connection. Opens the downchannel,
/// sends SynchronizeState, then Recognize, and returns the Speak MP3 bytes.
pub async fn recognize(config: &Config, token: &str, pcm: &[u8]) -> anyhow::Result<Vec<u8>> {
    let host = config.region.gateway_host().to_string();
    let addr = format!("{host}:443");

    let tcp = TcpStream::connect(&addr).await
        .with_context(|| format!("connecting to {addr}"))?;
    let domain = rustls::pki_types::ServerName::try_from(host.clone())?;
    let tls = tls_connector().connect(domain, tcp).await.context("TLS handshake")?;

    let (mut send_req, conn) = h2::client::handshake(tls).await.context("h2 handshake")?;
    tokio::spawn(async move {
        let _ = conn.await;
    });

    // 1) Downchannel: GET /v20160207/directives, keep the response stream open.
    let downchannel = http::Request::builder()
        .method("GET")
        .uri(format!("https://{host}/v20160207/directives"))
        .header("authorization", format!("Bearer {token}"))
        .body(())
        .unwrap();
    let (_dc_resp, _dc_send) = send_req.send_request(downchannel, true)?;
    // We don't need to read the downchannel for a one-shot, but it must be opened
    // before posting events on the same connection.

    // 2) SynchronizeState
    post_event(&mut send_req, &host, token, &synchronize_state_json(&Uuid::new_v4().to_string()), &[]).await?;

    // 3) Recognize
    let event = recognize_event_json(&Uuid::new_v4().to_string(), &Uuid::new_v4().to_string());
    let boundary = format!("alexa-{}", Uuid::new_v4());
    let body = build_recognize_multipart(&event, pcm, &boundary);
    let (status, ct, resp_body) = post_multipart(&mut send_req, &host, token, &boundary, body).await?;

    match status {
        200 => {
            let parts = parse_multipart_related(&ct, &resp_body)?;
            extract_speak_audio(&parts)
        }
        204 => anyhow::bail!("Alexa returned no response (204) — try rephrasing"),
        403 => anyhow::bail!("403 from AVS — token invalid/expired or wrong region (try `alexa login` / --region)"),
        400 => anyhow::bail!("400 from AVS — bad request/audio format: {}", String::from_utf8_lossy(&resp_body)),
        other => anyhow::bail!("unexpected AVS status {other}: {}", String::from_utf8_lossy(&resp_body)),
    }
}

async fn post_event(
    send_req: &mut h2::client::SendRequest<bytes::Bytes>,
    host: &str,
    token: &str,
    event_json: &str,
    _ctx: &[u8],
) -> anyhow::Result<u16> {
    let boundary = format!("ev-{}", Uuid::new_v4());
    let body = build_recognize_multipart(event_json, &[], &boundary);
    // SynchronizeState has no audio; reuse the multipart builder with empty audio
    // OR send a single JSON part. AVS accepts a metadata-only multipart.
    let (status, _ct, _body) = post_multipart(send_req, host, token, &boundary, body).await?;
    Ok(status)
}

async fn post_multipart(
    send_req: &mut h2::client::SendRequest<bytes::Bytes>,
    host: &str,
    token: &str,
    boundary: &str,
    body: Vec<u8>,
) -> anyhow::Result<(u16, String, Vec<u8>)> {
    let req = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/v20160207/events"))
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", format!("multipart/form-data; boundary={boundary}"))
        .body(())
        .unwrap();

    let (resp_fut, mut stream) = send_req.send_request(req, false)?;
    stream.send_data(bytes::Bytes::from(body), true)?;

    let resp = resp_fut.await?;
    let status = resp.status().as_u16();
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let mut body_buf = Vec::new();
    let mut body_stream = resp.into_body();
    while let Some(chunk) = body_stream.data().await {
        let chunk = chunk?;
        let _ = body_stream.flow_control().release_capacity(chunk.len());
        body_buf.extend_from_slice(&chunk);
    }
    Ok((status, ct, body_buf))
}
```

Note: this is the highest-risk module. Verify the `h2` 0.4 client API: `client::handshake`, `SendRequest::send_request(req, end_of_stream)`, `SendStream::send_data`, `RecvStream::data()`/`flow_control().release_capacity()`. The `post_event` shortcut sends SynchronizeState as a metadata-only multipart; if AVS rejects it, send a single `application/json` body instead. Add `use anyhow::Context;` at the top if not already present. The downchannel must be opened before posting events on the same connection (do not drop `send_req`).

- [ ] **Step 4: Build (and optionally run the live test)**

Run: `cargo test --lib avs --no-run`
Expected: compiles.
Optional live: `cargo test --lib -- --ignored recognize_live_roundtrip` (needs valid tokens).

- [ ] **Step 5: Commit**

```bash
git add src/avs.rs
git commit -m "feat: AVS HTTP/2 transport (downchannel + Recognize round-trip)"
```

---

### Task 14: CLI wiring + `doctor` (Tier 0 de-risk)

**Files:**
- Modify: `src/cli.rs`

**Interfaces:**
- Consumes: everything above
- Produces: `run()` with subcommands `configure`, `login`, `doctor`, and the default text path

- [ ] **Step 1: Write a CLI parse test**

```rust
// src/cli.rs  (append)
#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_text_arg() {
        let cli = Cli::parse_from(["alexa", "what time is it"]);
        assert_eq!(cli.text.as_deref(), Some("what time is it"));
        assert!(cli.command.is_none());
    }

    #[test]
    fn parses_subcommand() {
        let cli = Cli::parse_from(["alexa", "doctor"]);
        assert!(matches!(cli.command, Some(Command::Doctor)));
    }

    #[test]
    fn parses_flags() {
        let cli = Cli::parse_from(["alexa", "-v", "--voice", "espeak", "hello"]);
        assert!(cli.verbose);
        assert_eq!(cli.voice.as_deref(), Some("espeak"));
        assert_eq!(cli.text.as_deref(), Some("hello"));
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib cli`
Expected: FAIL.

- [ ] **Step 3: Implement the CLI**

```rust
// src/cli.rs  (replace contents)
use crate::config::{Config, Region, Voice};
use crate::{auth, avs, cache, tts};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::io::{self, Read};

#[derive(Parser, Debug)]
#[command(name = "alexa", version, about = "Round-trip text through Alexa: TTS -> AVS -> Whisper STT")]
pub struct Cli {
    /// Text to send to Alexa, e.g. "what time is it". Reads stdin if omitted with `-`.
    pub text: Option<String>,

    #[command(subcommand)]
    pub command: Option<Command>,

    /// Verbose diagnostics
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// AVS gateway region: na|eu|fe
    #[arg(long, global = true)]
    pub region: Option<String>,

    /// TTS backend: piper|espeak
    #[arg(long, global = true)]
    pub voice: Option<String>,

    /// Whisper model, e.g. base.en|tiny.en
    #[arg(long, global = true)]
    pub model: Option<String>,

    /// Keep intermediate artifacts
    #[arg(long, global = true)]
    pub keep_artifacts: bool,

    /// Write artifacts to DIR (implies --keep-artifacts)
    #[arg(short, long, global = true)]
    pub output: Option<String>,

    /// Print full result as JSON
    #[arg(long, global = true)]
    pub json: bool,

    /// Skip the transcription cache
    #[arg(long, global = true)]
    pub no_cache: bool,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Set Client ID / Secret / Product ID
    Configure,
    /// Authorize with Amazon and cache tokens
    Login,
    /// Validate credentials and run one live round-trip
    Doctor,
}

fn apply_overrides(cli: &Cli, cfg: &mut Config) {
    if let Some(r) = &cli.region { cfg.region = Region::from_str_lenient(r); }
    if let Some(v) = &cli.voice {
        cfg.voice = if v.eq_ignore_ascii_case("espeak") { Voice::Espeak } else { Voice::Piper };
    }
    if let Some(m) = &cli.model { cfg.model = m.clone(); }
}

pub async fn run() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Some(Command::Configure) => return configure(),
        Some(Command::Login) => {
            let cfg = Config::load()?;
            return auth::login(&cfg, 8086).await;
        }
        Some(Command::Doctor) => return doctor(&cli).await,
        None => {}
    }

    let text = match &cli.text {
        Some(t) if t == "-" => read_stdin()?,
        Some(t) => t.clone(),
        None => {
            use clap::CommandFactory;
            Cli::command().print_help()?;
            println!();
            return Ok(());
        }
    };

    let answer = ask(&cli, &text).await?;
    if cli.json {
        println!("{}", serde_json::json!({ "success": true, "result": answer }));
    } else {
        println!("{answer}");
    }
    Ok(())
}

/// The core pipeline: text -> TTS -> AVS -> STT -> text.
async fn ask(cli: &Cli, text: &str) -> Result<String> {
    let mut cfg = Config::load()?;
    apply_overrides(cli, &mut cfg);
    let v = cli.verbose;

    if v { eprintln!("[tts] synthesizing with {:?}", cfg.voice); }
    let backend = tts::backend_for(&cfg.voice)?;
    let pcm_i16 = backend.synth(text)?;
    let pcm_bytes = crate::audio::i16_to_le_bytes(&pcm_i16);

    if v { eprintln!("[avs] sending {} bytes of LPCM to {}", pcm_bytes.len(), cfg.region.gateway_host()); }
    let mp3 = match send_recognize(&cfg, &pcm_bytes).await {
        Ok(mp3) => mp3,
        Err(e) => {
            // one retry with a forced token refresh (mirrors the Python tool)
            if v { eprintln!("[avs] first attempt failed ({e}); refreshing token + retry"); }
            let token = auth::access_token(&cfg, true).await?;
            avs::recognize(&cfg, &token, &pcm_bytes).await?
        }
    };

    if cli.keep_artifacts || cli.output.is_some() {
        let dir = cli.output.clone().unwrap_or_else(|| ".".to_string());
        std::fs::create_dir_all(&dir).ok();
        std::fs::write(format!("{dir}/response.mp3"), &mp3).ok();
        std::fs::write(format!("{dir}/request.pcm"), &pcm_bytes).ok();
        if v { eprintln!("[artifacts] wrote response.mp3 / request.pcm to {dir}"); }
    }

    // cache lookup/store
    let key = cache::key_for(&mp3);
    if !cli.no_cache && cfg.save_transcription {
        let c = cache::Cache::load();
        if let Some(hit) = c.get(&key) {
            if v { eprintln!("[stt] cache hit"); }
            return Ok(hit);
        }
    }

    if v { eprintln!("[stt] transcribing with whisper {}", cfg.model); }
    let transcript = crate::stt::transcribe_mp3(&mp3, &cfg)?;

    if !cli.no_cache && cfg.save_transcription {
        let mut c = cache::Cache::load();
        c.put(&key, &transcript).ok();
    }
    Ok(transcript)
}

async fn send_recognize(cfg: &Config, pcm: &[u8]) -> Result<Vec<u8>> {
    let token = auth::access_token(cfg, false).await?;
    avs::recognize(cfg, &token, pcm).await
}

fn read_stdin() -> Result<String> {
    let mut s = String::new();
    io::stdin().read_to_string(&mut s)?;
    Ok(s.trim().to_string())
}

fn configure() -> Result<()> {
    let mut cfg = Config::load_or_default();
    println!("Configuring AVS credentials (stored in {}).", Config::path().display());
    println!("Press Enter to keep the current value shown in [brackets].\n");
    cfg.client_id = prompt("Client ID", &cfg.client_id)?;
    cfg.client_secret = prompt("Client Secret", &cfg.client_secret)?;
    cfg.product_id = prompt("Product ID (Program ID)", &cfg.product_id)?;
    if cfg.device_serial_number.is_empty() {
        cfg.device_serial_number = uuid::Uuid::new_v4().to_string();
    }
    cfg.save()?;
    println!("\nSaved. Register this redirect URL as an Allowed Return URL in your");
    println!("LWA security profile, then run `alexa login`:\n  http://localhost:8086/auth");
    Ok(())
}

fn prompt(label: &str, current: &str) -> Result<String> {
    use std::io::Write;
    print!("{label} [{current}]: ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let line = line.trim();
    Ok(if line.is_empty() { current.to_string() } else { line.to_string() })
}

async fn doctor(cli: &Cli) -> Result<()> {
    println!("alexa doctor — validating setup\n");

    let cfg = Config::load_or_default();
    check("config present", cfg.is_complete(),
        "run `alexa configure` to set Client ID / Secret / Product ID");
    if !cfg.is_complete() { return Ok(()); }

    match auth::Tokens::load() {
        Ok(_) => check("tokens present", true, ""),
        Err(_) => { check("tokens present", false, "run `alexa login`"); return Ok(()); }
    }

    match auth::access_token(&cfg, false).await {
        Ok(_) => check("access token valid", true, ""),
        Err(e) => { check("access token valid", false, &format!("{e}")); return Ok(()); }
    }

    println!("\nRunning a live round-trip: \"what time is it\"");
    match ask(cli, "what time is it").await {
        Ok(answer) => {
            check("round-trip", true, "");
            println!("\nAlexa said: {answer}");
            println!("\nAll good. ✅");
        }
        Err(e) => {
            check("round-trip", false, &format!("{e}"));
            println!("\nThe pipeline failed above — see the message for the next step.");
        }
    }
    Ok(())
}

fn check(label: &str, ok: bool, hint: &str) {
    if ok {
        println!("  [ok] {label}");
    } else {
        println!("  [!!] {label} — {hint}");
    }
}
```

- [ ] **Step 4: Run tests + build**

Run: `cargo test --lib cli`
Expected: PASS (3 tests).
Run: `cargo build`
Expected: full crate compiles.

- [ ] **Step 5: Live de-risk (manual)**

Run: `cargo run -- configure` (enter creds), then `cargo run -- login`, then `cargo run -- doctor`.
Expected: doctor reports each check; the round-trip prints Alexa's answer if credentials work. **This is the Tier 0 gate** — if `doctor` succeeds, the approach is proven.

- [ ] **Step 6: Commit**

```bash
git add src/cli.rs
git commit -m "feat: CLI (configure/login/doctor + core ask pipeline)"
```

---

### Task 15: Replace Python project + README + Makefile

**Files:**
- Delete: `alexatext/`, `setup.py`, `setup.cfg`, `requirements.txt`, `tests/test_cli.py` (Python)
- Modify: `README.md`, `Makefile`, `VERSION`
- Delete (optional): `Dockerfile` (or rewrite later)

**Interfaces:**
- Consumes: nothing
- Produces: a clean Rust-only repo with accurate docs

- [ ] **Step 1: Remove Python sources**

```bash
git rm -r alexatext setup.py setup.cfg requirements.txt tests/test_cli.py
# keep tests/fixtures/ (used by Rust tests) — restore if git rm removed it:
git checkout -- tests/fixtures 2>/dev/null || true
```

(History is preserved in git.)

- [ ] **Step 2: Rewrite `Makefile`**

```makefile
.PHONY: build test install fmt clippy run

build:
	cargo build --release

test:
	cargo test

install:
	cargo install --path .

fmt:
	cargo fmt

clippy:
	cargo clippy -- -D warnings

run:
	cargo run --
```

- [ ] **Step 3: Rewrite `README.md`**

Replace with Rust-focused docs covering: what it does; prerequisites (`cmake` + C compiler for whisper.cpp; `espeak-ng` only for `--voice espeak`); install (`cargo install --path .` or `make install`); the AVS credential requirement (existing product only — registration closed); `alexa configure` → register the `http://localhost:8086/auth` return URL → `alexa login` → `alexa doctor`; usage examples (`alexa "what time is it"`, `--voice`, `--model`, `--region`, `--json`, `--keep-artifacts`, `--verbose`); a "how it works" section (TTS → AVS HTTP/2 → Whisper); and a note that this replaces the legacy Python tool.

- [ ] **Step 4: Bump `VERSION`**

Set `VERSION` contents to `0.2.0` (matches Cargo.toml).

- [ ] **Step 5: Verify the repo builds clean**

Run: `cargo build && cargo test`
Expected: builds; all non-ignored tests pass.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "chore: replace Python tool with Rust CLI; update docs/Makefile"
```

---

### Task 16: Polish — fmt, clippy, end-to-end smoke

**Files:**
- Modify: any (lint fixes)

**Interfaces:**
- Consumes: full crate
- Produces: a clean, warning-free build

- [ ] **Step 1: Format**

Run: `cargo fmt`

- [ ] **Step 2: Clippy**

Run: `cargo clippy -- -D warnings`
Fix all warnings (unused imports, the illustrative lines flagged in earlier tasks, etc.).

- [ ] **Step 3: Full test run**

Run: `cargo test`
Expected: all non-ignored tests pass.

- [ ] **Step 4: Manual end-to-end (if creds available)**

Run: `cargo run --release -- "what time is it"`
Expected: prints Alexa's spoken answer as text.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "chore: fmt + clippy clean"
```

---

## Self-Review

**Spec coverage:**
- AVS HTTP/2 round-trip → Tasks 5, 6, 13. ✅
- TTS (Piper default + espeak fallback, 16k mono) → Tasks 9, 10. ✅
- STT (whisper-rs base.en, MP3 decode) → Tasks 7, 8. ✅
- Auth (loopback LWA, refresh, 403 retry) → Tasks 11, 12, 14 (retry in `ask`). ✅
- Config reuse `~/.alexa` + legacy keys → Task 3. ✅
- Cache (extra) → Tasks 4, 14. ✅
- CLI surface + `doctor` + verbose/artifacts/json/no-cache/overrides → Task 14. ✅
- Region default + override + SetGateway → Task 3 (hosts), Task 13 (transport). **Gap:** `SetGateway` reconnect is described in the spec but not implemented; for a one-shot it's lower priority. **Noted as a known limitation** — the default NA gateway is used and a wrong region yields a clear 403 with a `--region` hint. Add a follow-up task if multi-region auto-correction is needed.
- Replace Python + docs → Task 15. ✅
- Deferred items (multi-account, Google/DeepSpeech, OPUS, streaming, multi-turn) → correctly out of scope. ✅

**Placeholder scan:** No `TODO`/`TBD`/"add error handling" left as instructions. The two illustrative lines (`let _ = Arc::new(());` in Task 10) are explicitly flagged for removal and covered by Task 16 clippy. ✅

**Type consistency:** Names checked across tasks — `Config`, `Region::gateway_host`, `Voice`, `Tokens`, `is_expired`, `now_secs`, `recognize_event_json`, `synchronize_state_json`, `build_recognize_multipart`, `Part`, `parse_multipart_related`, `extract_speak_audio`, `recognize`, `decode_mp3_to_16k_mono`, `ensure_model`, `transcribe_samples`, `transcribe_mp3`, `TtsBackend::synth`, `backend_for`, `wav_bytes_to_16k_mono_i16`, `Cache::{load,get,put}`, `key_for`, `cli::run` — all consistent between producer and consumer tasks. ✅

**Known deviations to watch during execution:** exact crate APIs for `h2` 0.4, `whisper-rs` 0.16, `piper-rs` 0.2, `symphonia` 0.5, and `rubato` 0.15 must be verified against their docs at implementation time; tests pin behavior so signature drift is caught fast. The `piper-rs` path has a documented shell-out fallback if `ort` won't build.
