# alexa-cli

`alexa` is a Rust command-line tool for Amazon Alexa. It does two distinct things:

1. **Ask Alexa** — round-trips text through the Alexa Voice Service: synthesizes your
   prompt to speech, sends it to Alexa, and transcribes Alexa's spoken reply back to text
   with a local Whisper model.
2. **Control your Echos** — sends announcements / text-to-speech to your own Echo devices,
   lists them, and reads your Alexa voice history (text transcripts).

```console
$ alexa "what time is it"
the time is eleven seventeen p m

$ alexa announce "dinner is ready"
Announced on 8 device(s).

$ alexa say "the build finished" --device office

$ alexa history --size 5
[2m ago] Kitchen   you: alexa set a timer for ten minutes   alexa: ten minutes, starting now
```

This is a complete Rust rewrite of the original Python tool. The "Ask Alexa" path uses the
modern AVS HTTP/2 API (`v20160207`) with **local** speech recognition (whisper.cpp via
`whisper-rs`, no cloud STT). The "Control your Echos" path uses the unofficial
`alexa.amazon.com` behaviors API (the same one Home Assistant's Alexa Media Player uses).

---

## Prerequisites

- **Rust** (stable) and a **C/C++ compiler + `cmake`** — `whisper-rs` builds whisper.cpp
  from source.
- **`tar`** — used to unpack the Piper voice on first run (standard on Linux/macOS).
- **`espeak-ng`** — only if you use `--voice espeak`. The default Piper backend needs no
  extra system packages.

First run downloads the Piper voice (~tens of MB) and the Whisper `base.en` model
(~140 MB) into `~/.alexa/models/`.

## Install

```console
cargo install --path .   # or: make install
```

Installs the `alexa` binary to `~/.cargo/bin`.

---

## Part 1 — Ask Alexa (Voice Service round-trip)

### How it works

1. **TTS** — your text → 16 kHz mono PCM. Default backend is Piper (a VITS voice via
   `sherpa-onnx`); `--voice espeak` shells out to the system `espeak-ng`.
2. **AVS** — the PCM is sent to the Alexa Voice Service over one multiplexed HTTP/2
   connection (the `h2` crate). Alexa replies with an MP3 of its spoken response.
3. **STT** — the MP3 is decoded and transcribed locally with Whisper (`base.en` default,
   `tiny.en` selectable). CPU only.

### Credentials

You need an **existing** AVS product / Login-with-Amazon security profile. Amazon has
closed new AVS device registration, so you must reuse a product you already own. You'll
need its **Client ID**, **Client Secret**, and **Product ID** (the legacy `programId`).

### Setup

```console
alexa configure        # enter Client ID / Secret / Product ID  → ~/.alexa/config.json
```

In the Amazon developer console, under your security profile, add the allowed return URL:

```
http://localhost:8086/auth
```

Then:

```console
alexa login            # loopback OAuth (opens a browser), caches tokens
alexa doctor           # validates credentials + runs one live round-trip
```

Config lives in `~/.alexa/config.json` — the same location and key names (`clientId`,
`clientSecret`, `programId`) the Python tool used, so existing config carries over.

### Usage

```console
alexa "what time is it"
alexa --json "how many days until christmas"
alexa --voice espeak --model tiny.en "what's the weather"
```

Global flags: `--voice <piper|espeak>`, `--model <base.en|tiny.en>`,
`--region <na|eu|fe>`, `--json`, `--keep-artifacts`, `-o/--output <DIR>`, `--no-cache`,
`-v/--verbose`. Transcriptions are cached in `~/.alexa/cache.json`.

---

## Part 2 — Control your Echos (announcements, TTS, history)

This uses the unofficial Alexa web API and a **separate, one-time login** that mints a
durable refresh token (cookies then auto-renew).

### One-time login

```console
alexa announce-login
```

It prints an Amazon sign-in URL. Open it in **any** browser (works headless — the browser
can be on another machine), sign in (MFA/passkey all work since it's Amazon's own page),
and you'll land on a blank `…/ap/maplanding?...` page. Copy that full URL and paste it
back at the prompt. The durable refresh token is saved to `~/.alexa/alexa_remote.json`.

### Commands

```console
alexa devices                                  # list your Echos (name, online, serial)
alexa announce "dinner is ready"               # announce to all online Echo speakers
alexa announce "5 min warning" --device kitchen   # one device (name substring)
alexa announce "heads up" --title "Reminder"   # custom announcement title
alexa say "the deploy finished" --device office   # plain TTS (no chime), one device
alexa history --size 20 --device kitchen       # recent voice transcripts (text only)
```

`announce` (default `--all`) targets only **online, announcement-capable Echo speakers /
Shows** — it skips tablets, Fire TVs, Echo Buds, third-party AVS gear, and other-household
devices (Amazon rejects mixed batches). It groups by device owner, batches each group, and
falls back to per-device on rejection. It also backs off and retries on rate limits (429).

### Limitations (by design / Amazon's API)

- **Announcements are write-only.** There is no API to list announcements you've sent —
  they don't appear in history. `alexa history` shows *spoken* interactions only.
- **Offline / non-Echo targets won't play.** Targeting an offline or third-party device
  prints a note; the API "accepts" it but nothing plays.
- **Official AVS announcements are not used** — that feature is gated behind an Amazon
  partner allowlist that self-registered products can't get. This tool uses the
  unofficial web API instead, which works for your own account.
- The unofficial API is undocumented and can change; the login is the most fragile part,
  but once you have a refresh token the rest is stable.

---

## Files

Everything lives under `~/.alexa/`:

- `config.json` — AVS credentials + defaults (Client ID/Secret, Product ID, region, voice, model).
- `tokens.json` — AVS OAuth tokens (auto-refreshed).
- `alexa_remote.json` — refresh token + cached cookies for the Echo-control features.
- `cache.json` — transcription cache.
- `models/` — downloaded Whisper + Piper models.

## License

MIT. See [LICENSE](LICENSE).
