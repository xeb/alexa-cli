# Alexa CLI

`alexa` is a command-line tool that round-trips text through Alexa: it
synthesizes your prompt to speech, sends that audio to the Alexa Voice
Service (AVS), and transcribes Alexa's spoken reply back to text using a
local Whisper model.

```
$ alexa "what time is it"
the time is eleven seventeen p m

$ alexa "how many days until christmas"
there are 181 days until christmas day
```

This is a complete Rust rewrite that replaces the legacy Python tool. It
talks to the modern AVS HTTP/2 API (`v20160207`) and does all speech
recognition locally with whisper.cpp (via `whisper-rs`) — no cloud STT.

## How it works

1. **TTS** — your text is synthesized to 16 kHz mono PCM. The default
   backend is Piper (a VITS voice run through `sherpa-onnx`); `--voice
   espeak` shells out to the system `espeak-ng` instead.
2. **AVS** — the PCM is streamed to the Alexa Voice Service over a single
   multiplexed HTTP/2 connection. Alexa returns an MP3 of its spoken
   response.
3. **STT** — the MP3 is decoded and transcribed locally with Whisper
   (`base.en` by default, `tiny.en` selectable). CPU only.

## Prerequisites

- A C compiler and `cmake` (whisper.cpp is built from source by
  `whisper-rs`).
- `espeak-ng` — only required if you use `--voice espeak`. The default
  Piper backend needs no extra system packages.

## Install

```
cargo install --path .
# or
make install
```

This installs the `alexa` binary.

## AVS credentials

You need an existing AVS product / security profile. Amazon has closed new
AVS device registrations, so you must reuse credentials from a product you
already own. You will need the **Client ID**, **Client Secret**, and
**Product ID** (the legacy `programId`).

## Setup

1. **Configure credentials:**

   ```
   alexa configure
   ```

   Enter your Client ID, Client Secret, and Product ID. Configuration is
   stored in `~/.alexa/config.json` (the same directory the Python tool
   used; legacy `clientId` / `clientSecret` / `programId` keys are read
   verbatim).

2. **Register the return URL.** In the Amazon developer console, under your
   security profile, add this allowed return URL:

   ```
   http://localhost:8086/auth
   ```

3. **Log in:**

   ```
   alexa login
   ```

   This runs a loopback OAuth flow (Login with Amazon) and caches the
   resulting tokens.

4. **Verify everything works:**

   ```
   alexa doctor
   ```

   Validates your credentials and runs one live round-trip.

## Usage

```
alexa "what time is it"
```

Flags (all global):

- `--voice <piper|espeak>` — TTS backend (default `piper`).
- `--model <base.en|tiny.en>` — Whisper model (default `base.en`).
- `--region <na|eu|fe>` — AVS gateway region (default `na`).
- `--json` — print the full result as JSON.
- `--keep-artifacts` — keep intermediate audio artifacts.
- `-o, --output <DIR>` — write artifacts to `DIR` (implies
  `--keep-artifacts`).
- `--no-cache` — skip the transcription cache.
- `-v, --verbose` — verbose diagnostics.

Subcommands:

- `alexa configure` — set Client ID / Secret / Product ID.
- `alexa login` — authorize with Amazon and cache tokens.
- `alexa doctor` — validate credentials and run one live round-trip.

Transcriptions are cached in `~/.alexa/` so repeated identical Alexa
responses are not re-transcribed.

## License

MIT. See [LICENSE](LICENSE).
