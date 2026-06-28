# CLAUDE.md

Guidance for working in this repo with Claude Code.

## What this is

`alexa-cli` — a Rust CLI (`alexa`) for Amazon Alexa, a complete rewrite of the original
Python tool. Two capabilities:

1. **Ask Alexa** — text → TTS → Alexa Voice Service (modern HTTP/2 `v20160207`) → MP3
   reply → local Whisper STT → text.
2. **Control your Echos** — announcements / TTS / device list / voice history via the
   unofficial `alexa.amazon.com` behaviors + privacy APIs.

Design + plan docs: `docs/superpowers/specs/` and `docs/superpowers/plans/`.

## Build / test / run

```bash
cargo build
cargo test --lib              # 51 unit tests; pure logic only, no network
cargo clippy -- -D warnings   # keep clean
cargo run -- "what time is it"
make install                  # = cargo install --path .
```

- The **first** build compiles whisper.cpp from source (cmake + C/C++ compiler) and
  downloads the sherpa-onnx prebuilt lib — it takes several minutes. Use a long timeout.
- Two tests are `#[ignore]`d (live network: AVS round-trip, whisper transcription). Do
  **not** run `--ignored` without credentials.

## Module map (`src/`)

- `cli.rs` — clap parsing + dispatch (`run()`); subcommands: (default text), `configure`,
  `login`, `doctor`, `devices`, `announce`, `say`, `history`, `announce-login`, `announce-auth`.
- `config.rs` — `~/.alexa/config.json` (legacy `clientId`/`clientSecret`/`programId` keys).
- `auth.rs` — AVS Login-with-Amazon (loopback redirect on :8086), token cache/refresh.
- `tts/` — `TtsBackend` trait; `piper` (default, via `sherpa-onnx`) + `espeak` (shell-out).
- `avs.rs` — AVS HTTP/2 transport (the `h2` crate), multipart encode/decode, Recognize round-trip.
- `stt.rs` — MP3 decode (`symphonia`) + Whisper (`whisper-rs`, CPU, `base.en`).
- `remote.rs` — unofficial Echo control: announce/say/devices/history + browser device-reg login.
- `cache.rs` / `audio.rs` — transcription cache; PCM/resample helpers.

## Hard-won gotchas (don't regress these)

- **`git config core.fileMode false`** is set: the `/media/...` mount reports every file
  as mode 755, so without this git shows phantom diffs on every file (and merges abort).
- **`piper-rs` does not build here** — its `espeak-rs-sys` fails to compile espeak-ng
  (glibc `_FORTIFY_SOURCE` abort). Piper runs via **`sherpa-onnx`** (prebuilt lib). Do not
  switch back to `piper-rs`.
- **AVS transport uses `h2` directly, not `reqwest`** — one multiplexed connection holds
  the downchannel open; reqwest's pooling breaks it.
- **rustls 0.23 needs a crypto provider**: `tls_connector()` installs aws-lc-rs before
  `ClientConfig::builder()`, else the live TLS path panics.
- **`reqwest::blocking` must not run inside the tokio runtime** — TTS synth + Whisper
  transcribe (which may download models) run in `tokio::task::spawn_blocking`.
- **`announce-login` OAuth**: `openid.ns.oa2` must be the literal `http://www.amazon.com/ap/ext/oauth/2`
  (http, fixed host) — https or a tld-template makes Amazon drop the auth code.
- **Voice history (`remote.rs`)**: the privacy API needs a *second* token,
  `anti-csrftoken-a2z`, scraped from the activity page `<meta name="csrf-token">`, AND the
  request must send `Accept-Encoding: identity` (no gzip/brotli decoder is linked, so a
  compressed body is unreadable).
- **Announcements are write-only** — no API lists them; `history` shows spoken
  interactions only. `announce --all` filters to online Echo speakers and skips
  tablets/Fire TVs/Buds/third-party/offline devices.

## Secrets

All credentials live under **`~/.alexa/`** (home, gitignored — `config.json`,
`tokens.json`, `alexa_remote.json`). Never commit secrets. The repo's `.gitignore` covers
these names; keep it that way.

## Conventions

- Match existing style: `anyhow::Result` at boundaries, `serde_json`, small focused
  functions, unit-test pure logic (parsers/builders) and leave network paths to live use.
- Commit per logical change; keep `cargo test --lib` + `cargo clippy -- -D warnings` green.
