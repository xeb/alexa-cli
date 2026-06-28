# Alexa CLI — How to Use It

`alexa` is a command-line tool that lets you do two things from your terminal:

1. **Ask Alexa a question** and get the answer back as text.
2. **Control your own Echo (and Sonos / Alexa-built-in) speakers** — send announcements, speak text on a device, list devices, and read your recent voice history.

---

## 1. Install (once)

You need Rust, a C/C++ compiler + `cmake` (the speech engine builds from source), and `tar`.

```bash
cargo install --path .      # or: make install
```

This puts the `alexa` command on your PATH. Check it:

```bash
alexa --version             # -> alexa 1.0.2
alexa --help                # full list of commands and flags
```

The first time you ask a question, it downloads a small speech model (~140 MB) automatically.

---

## 2. First-time setup

There are two separate features, each with its own one-time login.

### A) "Ask Alexa" (needs existing AVS credentials)

```bash
alexa configure     # enter your AVS Client ID, Client Secret, and Product ID
alexa login         # opens a browser to sign in to Amazon, then saves tokens
alexa doctor        # checks everything and runs one live test
```

> In your Amazon developer security profile, make sure `http://localhost:8086/auth`
> is an allowed return URL before running `alexa login`.

### B) "Control your speakers" (announcements, etc.)

One browser login that keeps working afterward:

```bash
alexa announce-login
```

It prints an Amazon sign-in URL. Open it in **any** browser (even on another device),
sign in, and you'll land on a blank "page not found" — that's expected. Copy that page's
full URL and paste it back at the prompt. Done — you won't need to log in again.

---

## 3. Everyday use (with examples)

### Ask Alexa a question

```bash
alexa "what time is it"
alexa "how many days until christmas"
alexa "what's the weather in Seattle"
alexa --json "what is the price of bitcoin"     # machine-readable output
```

### Make an announcement (it speaks on your speakers)

```bash
alexa announce "dinner is ready"                 # all your online speakers (Echos + Sonos)
alexa announce "5 minute warning" --device kitchen   # just one (name match, any substring)
alexa announce "movie time" --title "Heads up"   # custom title on Echo Show screens
```

### Speak text on a single device (plain text-to-speech, no chime)

```bash
alexa say "the laundry is done" --device office
```

### List your devices

```bash
alexa devices                  # name, online/offline, serial
alexa devices --json           # full details
```

### See your recent voice history (text transcripts)

```bash
alexa history                  # last 20 interactions
alexa history --size 50
alexa history --device kitchen
```

Example output:

```
[3m ago] Kitchen
   you:   alexa set a timer for ten minutes
   alexa: ten minutes, starting now
```

---

## 4. Handy flags

These work on any command (put them anywhere):

| Flag | What it does |
|---|---|
| `-v`, `--verbose` | Show what's happening under the hood (useful for troubleshooting) |
| `--device <name>` | Target one device by a piece of its name (e.g. `--device bath`) |
| `--all` | Announce to every online speaker (the default for `announce`) |
| `--title <text>` | Title shown on Echo Show screens for an announcement |
| `--voice <piper\|espeak>` | Voice used to talk *to* Alexa (default: piper) |
| `--model <base.en\|tiny.en>` | Speech-recognition model (default: base.en; tiny.en is faster) |
| `--json` | Print results as JSON |

---

## 5. Tips & gotchas

- **`announce` (no `--device`) goes to all your online speakers** — Echos, Echo Shows, and
  Sonos / Alexa-built-in speakers. It skips Fire TVs, tablets, and phones on purpose.
- **Offline devices won't play a one-off announcement.** If a speaker shows "offline,"
  target it with `alexa announce --all` (it rides along with your online speakers) or wait
  until it's back online. Use `alexa devices` to see who's online.
- **Names are fuzzy.** `--device kitchen` matches "Kitchen", "Kitchen Show", etc. If a name
  matches more than one device, all matches are used. Run `alexa devices` to see exact names.
- **Announcements can't be "read back."** Once sent, there's no history of them — that's an
  Amazon limitation. (`alexa history` shows spoken interactions, not announcements you sent.)
- **If something fails, add `-v`** to see the request and the server's response.

---

## 6. Where your settings live

Everything is stored under `~/.alexa/` on your machine (and never shared):

- `config.json` — your AVS credentials and defaults
- `tokens.json` / `alexa_remote.json` — login tokens (kept fresh automatically)
- `cache.json` — cached transcriptions
- `models/` — downloaded speech models
