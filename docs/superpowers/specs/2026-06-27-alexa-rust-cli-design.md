# Design: `alexa` — Rust CLI for round-tripping text through Alexa

- **Date:** 2026-06-27
- **Status:** Approved design (pending spec review)
- **Author:** Mark Kockerbeck (with Claude Code)
- **Replaces:** the Python `alexatext` CLI in this repo

## 1. Summary

Rebuild the existing Python `alexatext` tool as a Rust CLI installed as the command
`alexa`. The tool takes text on the command line, synthesizes it to speech, sends that
audio to Amazon's Alexa Voice Service (AVS) as if a user had spoken it, receives Alexa's
spoken response, transcribes it locally with a fast Whisper model, and prints the answer
as text.

```
$ alexa "what time is it"
the time is eleven seventeen p m
```

Scope = **MVP + a few extras**: the core round-trip plus `--verbose`, keep-artifacts /
output-dir, and a transcription cache. Multi-account token keys, Google/DeepSpeech STT,
OPUS, streaming, and multi-turn dialogs are explicitly out of scope.

## 2. Background & constraints

The current Python tool (`alexatext/cli.py`) shells out to `text2wave`/`sox` (TTS),
`curl` (AVS), and `ffmpeg` (audio), then transcribes via Google Cloud or Mozilla
DeepSpeech. It targets the **legacy AVS v1 HTTP/1.1 endpoint**
(`access-alexa-na.amazon.com/v1/avs/speechrecognizer/recognize`), which Amazon has shut
down.

Key constraints established with the user:

- **Target the modern AVS HTTP/2 API (`v20160207`).** The v1 path is dead.
- **Language: Rust.** Installable as `alexa` with first-class `--help`.
- **Fastest practical TTS + STT.** A robotic/"synth" input voice is acceptable, but the
  user chose a neural Piper voice as the default because intelligibility to Alexa's ASR
  is the hard requirement.
- **Region: North America** (default gateway), with a `--region` override.
- **Hardware: CPU only** for STT — no CUDA/Metal dependencies.
- **Credentials: "need to check."** Amazon has closed new AVS product registration, so
  the tool is only viable with credentials from a previously-registered product. The
  design must validate this early and fail loudly (`alexa doctor`).
- **Config continuity:** reuse `~/.alexa/` and import existing `config.json` credentials.

### Feasibility verdict

The HTTP/2 round-trip still works in 2026 **if and only if** valid Login-with-Amazon
credentials (`client_id`, `client_secret`) and a registered `productID` exist. New
registration is closed, so this is the #1 gating risk — addressed by building the
de-risk path (`alexa doctor`) first.

## 3. Architecture

### Data flow

```
"what time is it"  (CLI arg or stdin)
  └─ tts    : Piper-low → f32@16k → i16 mono LPCM   (espeak-ng fallback → resample 22050→16000)
      └─ avs : one h2 connection → downchannel (GET /v20160207/directives, Bearer)
               → System.SynchronizeState → SpeechRecognizer.Recognize (multipart: JSON + raw LPCM)
          └─ response : 200 multipart/related → Speak directive (payload.url = "cid:<id>")
                        → binary part with Content-ID:<id> → Alexa's MP3 answer
              └─ stt   : symphonia decode MP3 → resample 16k mono f32 → whisper-rs → text
                  └─ stdout: "the time is eleven seventeen p m"
```

### Module layout (`src/`)

| Module | Responsibility | Key interface |
|---|---|---|
| `main.rs` | Entry point, tokio runtime, top-level error formatting | — |
| `cli.rs` | `clap` arg/subcommand parsing, `--help`, dispatch | `Cli`, `Command` |
| `config.rs` | Load/save `~/.alexa/config.json`; defaults; legacy import | `Config` |
| `auth.rs` | LWA loopback-redirect login; token cache + refresh | `fn access_token() -> Result<String>` |
| `tts/mod.rs` | `trait TtsBackend`; selects backend by `--voice` | `fn synth(&str) -> Result<Vec<i16>>` (16 kHz mono) |
| `tts/piper.rs` | Default neural backend | impl `TtsBackend` |
| `tts/espeak.rs` | Fast formant fallback (shell-out) | impl `TtsBackend` |
| `avs.rs` | h2 conn, downchannel, multipart encode/decode, directives | `fn recognize(pcm) -> Result<Vec<u8>>` (MP3) |
| `stt.rs` | MP3 decode + resample + `whisper-rs` transcription | `fn transcribe(mp3) -> Result<String>` |
| `cache.rs` | Transcription cache keyed by audio hash | `get`/`put` |

Each module hides its internals behind a small function/trait so it can be tested in
isolation (e.g. `avs` multipart encode/decode tested against a recorded fixture without
a network).

## 4. Component designs

### 4.1 `config` — `~/.alexa/`

Reuse the original location for continuity. Files:

- `~/.alexa/config.json` — `clientId`, `clientSecret`, `programId` (the AVS Product ID),
  `deviceSerialNumber`, `region`, plus new defaults (`voice`, `model`, `saveTranscription`).
  The legacy Python keys (`clientId`, `clientSecret`, `programId`) are read as-is so
  existing credentials carry over with no migration step.
- `~/.alexa/tokens.json` — `access_token`, `refresh_token`, `obtained_at`. (JSON rather
  than the legacy SQLite `tokens.db`; the user will re-login anyway, and this drops the
  `rusqlite` dependency.)
- `~/.alexa/cache.json` — transcription cache (see 4.6).
- `~/.alexa/models/` — downloaded Whisper + Piper model files.

`deviceSerialNumber` is generated once (random UUID) and persisted if absent.

### 4.2 `auth` — Login with Amazon (loopback redirect)

Use the **authorization-code flow with a loopback redirect**, matching the security
profile the user already configured for the Python tool. This avoids requiring
Code-Based Linking to be enabled on the existing profile.

> **Setup gotcha:** the LWA security profile must list the exact redirect URI as an
> Allowed Return URL. The Python tool's docs were inconsistent here (README mentioned
> `http://localhost:8089/auth` while the code used port `8086`). The Rust tool makes the
> loopback **port and redirect path configurable** (default `http://localhost:8086/auth`),
> and `alexa configure`/`alexa doctor` print the exact URL to register so it can't
> silently mismatch.

Flow (`alexa login`):
1. Start a tiny local HTTP server (`tiny_http`) on `localhost:8086`.
2. Open the browser (`webbrowser` crate) to
   `https://www.amazon.com/ap/oa?...&scope=alexa:all&scope_data={productID,deviceSerialNumber}&response_type=code&redirect_uri=http://localhost:8086/auth`.
3. Catch the `code` on `/auth`, exchange it at `https://api.amazon.com/auth/o2/token`
   (`grant_type=authorization_code`) via `reqwest`.
4. Persist `{access_token, refresh_token, obtained_at}` to `tokens.json`; shut the server.

`access_token()` returns a valid token, refreshing via `grant_type=refresh_token` when
older than ~3600 s (mirrors the Python tool). On a 403 during a request, force one
refresh + retry (mirrors `request_from_alexa_retry`).

### 4.3 `tts` — Piper default, espeak fallback

`trait TtsBackend { fn synth(&self, text: &str) -> Result<Vec<i16>>; }` returning
**16 kHz, 16-bit, mono** samples. Backend chosen by `--voice` (default `piper`):

- **Piper (default):** a "low" quality voice (e.g. `en_US-lessac-low`) which is **16 kHz
  native**, so output is f32 → i16 (clamped to `[-32768, 32767]`) with no resampling.
  Primary integration: the `piper-rs` crate (in-process ONNX via `ort`). Voice model
  (`.onnx` + `.onnx.json`) auto-downloaded to `~/.alexa/models/` on first use.
  - *Implementation fallback:* if `piper-rs`/`ort` (currently an `ort` 2.0 release
    candidate) proves unstable to build, shell out to the `piper` binary writing a WAV
    and read it with `hound`. This is an internal detail behind `TtsBackend`.
- **espeak (`--voice espeak`):** shell out to the `espeak-ng` binary
  (`espeak-ng --stdout`), parse the WAV with `hound` (i16 @ 22050), resample to 16 kHz
  with `rubato`. Fastest path; robotic. Requires the system `espeak-ng` package.

Rationale: Piper default minimizes the risk that Alexa's neural ASR mishears the input;
espeak stays available as the fastest option and a no-model fallback. `alexa doctor` can
A/B both against the live service.

### 4.4 `avs` — the HTTP/2 round-trip (highest-risk module)

- **Transport:** the `h2` crate (0.4) directly over `tokio-rustls` (rustls + ALPN `h2`),
  **not `reqwest`**. AVS requires one multiplexed HTTP/2 connection with a long-lived
  downchannel; reqwest's connection pooling would close/replace it. This is the single
  biggest implementation pitfall and the reason this module is hand-rolled and
  fixture-tested.
- **Connection sequence (one connection):**
  1. Connect to the regional gateway (NA default
     `https://alexa.na.gateway.devices.a2z.com`; `--region` → eu/fe).
  2. `GET /v20160207/directives` with `authorization: Bearer <token>`; keep this stream
     open as the downchannel.
  3. `POST /v20160207/events` with a `System.SynchronizeState` event (minimal context).
  4. `POST /v20160207/events` with `SpeechRecognizer.Recognize`.
  5. If a `SetGateway` directive arrives, reconnect to the corrected host and retry.
- **Recognize request:** `content-type: multipart/form-data; boundary=…`
  - `metadata` part (`application/json`):
    `{"event":{"header":{"namespace":"SpeechRecognizer","name":"Recognize","messageId":"<uuid>","dialogRequestId":"<uuid>"},"payload":{"profile":"CLOSE_TALK","format":"AUDIO_L16_RATE_16000_CHANNELS_1","initiator":{"type":"TAP"}}}}`
  - `audio` part (`application/octet-stream`): **raw LPCM bytes, no WAV header**, little-endian.
  - `profile: CLOSE_TALK` suits clean synthesized input; `initiator: TAP` = one-shot.
- **Response handling:** 200 → parse `multipart/related` (hand-rolled boundary parser,
  cross-checked with `multer` if convenient). Find the `SpeechSynthesizer.Speak`
  directive, read its `payload.url` (`cid:<id>`), and return the bytes of the part whose
  `Content-ID` header equals `<id>` (the MP3). Status mapping: `204` → "no response from
  Alexa"; `400` → invalid request (likely audio format); `403` → refresh token + retry
  once, then a clear region/credential error.
- **One-shot limitation:** if the response is `ExpectSpeech` (multi-turn), transcribe
  what we have and print a note; full dialog follow-up is deferred.

### 4.5 `stt` — Whisper (CPU)

- **`whisper-rs` 0.16** (whisper.cpp bindings), CPU build. Default model **`base.en`**
  (~140 MB) for good accuracy on numbers/dates/names; `--model tiny.en` (~75 MB) for
  maximum speed. GGML model auto-downloaded to `~/.alexa/models/` on first use.
- **Decode path:** Alexa returns `AUDIO_MPEG`. Decode the MP3 with `symphonia`
  (pure Rust, mp3 feature), downmix to mono, resample to 16 kHz f32 (`rubato`), feed to
  whisper. Short clips (1–10 s) transcribe in well under a second on CPU; total latency
  is dominated by network + Alexa, not STT.
- *Build note:* whisper.cpp compiles from source — `cmake` and a C/C++ compiler are
  build prerequisites.

### 4.6 `cache` — transcription cache (extra)

Optional cache (on by default; `--no-cache` to skip) keyed by a hash (e.g. blake3/sha256)
of the response MP3 bytes, mapping to the transcript. Stored in `~/.alexa/cache.json`.
Mirrors the Python tool's intent (skip re-transcribing identical audio) without SQLite.

## 5. CLI surface (`clap` derive)

```
alexa "what time is it"        # main path: speak → ask Alexa → transcribe → print
alexa configure                # set clientId / clientSecret / programId (imports legacy ~/.alexa/config.json)
alexa login                    # OAuth; cache tokens
alexa doctor                   # de-risk: check config/tokens, run one live round-trip with diagnostics

Global flags:
  -v, --verbose                Verbose diagnostics (timings, h2 events, intermediate steps)
      --region <na|eu|fe>      Override AVS gateway region (default: na)
      --voice <piper|espeak>   TTS backend (default: piper)
      --model <NAME>           Whisper model (default: base.en; e.g. tiny.en)
      --keep-artifacts         Keep intermediate audio/JSON instead of discarding
  -o, --output <DIR>           Write all artifacts to DIR (implies --keep-artifacts)
      --json                   Print full result as JSON ({success, result, ...})
      --no-cache               Skip the transcription cache
```

Running `alexa` with no text and no subcommand prints help. `alexa doctor` directly
serves the "need to check credentials" situation: it validates config + tokens and
performs one real round-trip, printing exactly where it fails.

## 6. Crate stack

| Concern | Crate | Version | Notes |
|---|---|---|---|
| Async runtime | `tokio` | 1.x | full features |
| HTTP/2 | `h2` | 0.4 | direct, multiplexed; persistent downchannel |
| TLS | `rustls` + `tokio-rustls` | 0.23 / 0.26 | ALPN `h2` |
| OAuth HTTP | `reqwest` | 0.12 | rustls backend; LWA token calls only (HTTP/1.1 fine) |
| Loopback server | `tiny_http` | 0.12 | catch OAuth redirect |
| Open browser | `webbrowser` | 1.x | open Amazon auth URL |
| CLI | `clap` (derive) | 4.x | rich `--help` |
| JSON | `serde` + `serde_json` | 1.x | events/directives/config |
| Multipart parse | `multer` (optional) | 3.x | cross-check hand-rolled parser |
| UUID | `uuid` (v4) | 1.x | messageId / dialogRequestId / DSN |
| TTS (default) | `piper-rs` | 0.2 | Piper neural; `ort` onnxruntime (RC — fallback: shell `piper`) |
| TTS (fallback) | shell `espeak-ng` | system pkg | `--voice espeak` |
| STT | `whisper-rs` | 0.16 | whisper.cpp; `base.en`/`tiny.en` ggml |
| MP3 decode | `symphonia` (mp3) | 0.5 | decode Alexa's AUDIO_MPEG |
| WAV I/O | `hound` | 3.5 | parse espeak / piper-binary WAV |
| Resample | `rubato` | 0.15 | 22050→16000 / MP3 rate→16000 |
| Hashing | `blake3` | 1.x | cache key |
| Config dirs | `dirs` | 5.x | resolve `~/.alexa` |
| Errors | `anyhow` + `thiserror` | 1.x | clear messages |

### System prerequisites
- `cmake` + a C/C++ compiler (to build whisper.cpp via `whisper-rs`).
- `espeak-ng` (only if using `--voice espeak`).
- Network access on first run (model downloads) and at build time (`ort`/onnxruntime,
  `whisper-rs` source).

## 7. Error handling & diagnostics

- Top-level errors via `anyhow` with actionable messages (missing config → "run
  `alexa configure`"; expired/invalid token → "run `alexa login`"; 403 → region/cred hint).
- One automatic token-refresh + retry on 403 (mirrors the Python retry).
- `--verbose` logs each stage with timings; `--keep-artifacts`/`-o` preserves the
  synthesized LPCM, the raw multipart response, the extracted MP3, and the transcript for
  debugging.
- `alexa doctor` is the consolidated de-risk command.

## 8. Testing strategy (TDD)

Write tests first per component:
- **Unit:** f32/i16 + resample conversions; multipart **encode** (byte-exact);
  multipart/related **decode** + `cid:`→`Content-ID` matching against a recorded fixture;
  config load/save + legacy import; cache get/put; CLI parsing.
- **Fixtures:** a captured AVS multipart response (JSON Speak directive + MP3 part) drives
  decode tests offline.
- **Integration:** a `#[ignore]`d live test exercising the real round-trip, surfaced
  through `alexa doctor` for manual runs.

## 9. Build, install, distribution

- `Cargo.toml` with `[[bin]] name = "alexa"`. Crate name `alexa-cli`.
- Install via `cargo install --path .` (later `cargo install alexa-cli`).
- The Python package (`alexatext/`, `setup.py`, `requirements.txt`, etc.) is removed and
  replaced by the Rust project; history remains in git. README rewritten for the Rust
  tool and the new setup steps.

## 10. Scope

**In scope (MVP + extras):** core round-trip; `alexa`/`configure`/`login`/`doctor`;
Piper + espeak TTS; whisper-rs STT; NA default + `--region` + `SetGateway` handling;
token cache/refresh + 403 retry; `--verbose`, `--keep-artifacts`/`-o`, `--json`;
transcription cache; legacy config import.

**Deferred (were in the Python tool):** multi-account token keys (`-k`/`--list`);
Google Cloud + DeepSpeech STT; OPUS audio; streaming mic capture / `StopCapture`;
multi-turn `ExpectSpeech` dialogs; Docker image.

## 11. Implementation order

- **Tier 0 — de-risk (first):** prove one full live round-trip — Piper LPCM → Recognize →
  MP3 → whisper text — against the user's real credentials. Validates the closed-
  registration assumption and ASR recognizability before polishing. Delivered as a first
  cut of `avs` + `tts` + `stt` wired through `alexa doctor`.
- **Tier 1 — lean CLI:** `alexa "<text>"`, `configure`, `login`, token refresh/retry,
  `--help`, clear errors.
- **Tier 2 — extras:** `--verbose`, `--keep-artifacts`/`-o`, `--json`, transcription
  cache, `--voice`/`--model`/`--region` flags, espeak fallback, legacy config import.

## 12. Decisions made

- AVS API: modern HTTP/2 `v20160207` (v1 is dead).
- Transport: `h2` crate directly (not reqwest) for the multiplexed downchannel.
- TTS default: Piper-low (16 kHz native); espeak-ng fallback via `--voice espeak`.
- STT: `whisper-rs` CPU, `base.en` default, `tiny.en` option.
- Region: NA default, `--region` override, honor `SetGateway`.
- Auth: loopback-redirect flow (matches existing security profile config).
- Storage: reuse `~/.alexa/`; import legacy `config.json` credentials; JSON for
  tokens/cache (no SQLite).

## 13. Open risks

- **Credential availability** (closed registration) — gated by `alexa doctor`.
- **ASR recognizability** of synthesized input — mitigated by Piper default; espeak A/B in `doctor`.
- **h2 persistent downchannel** correctness — hand-rolled + fixture-tested.
- **`ort`/`piper-rs` build stability** (RC) — fallback to shelling out to `piper`.
- **Multipart `cid`→`Content-ID` / CRLF** off-by-ones — covered by fixture tests.
- **Program withdrawal** — Amazon could disable the runtime at any time (external risk).
