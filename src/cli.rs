use crate::config::{Config, Region, Voice};
use crate::{auth, avs, cache, remote, tts};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::io::{self, Read};

#[derive(Parser, Debug)]
#[command(
    name = "alexa",
    version,
    about = "Round-trip text through Alexa: TTS -> AVS -> Whisper STT"
)]
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
    /// List your Echo devices (unofficial Alexa Remote API)
    Devices,
    /// Push an announcement to your Echo devices
    Announce {
        /// Message to announce
        message: String,
        /// Target a single device by (substring of) its name
        #[arg(long)]
        device: Option<String>,
        /// Announce to every online device
        #[arg(long)]
        all: bool,
        /// Announcement title (default "Announcement")
        #[arg(long)]
        title: Option<String>,
    },
    /// Speak (TTS) a message on a single device
    Say {
        /// Message to speak
        message: String,
        /// Target device by (substring of) its name
        #[arg(long)]
        device: String,
    },
    /// Show recent Alexa voice-history transcripts (text only)
    History {
        /// Max number of entries to show
        #[arg(long, default_value_t = 20)]
        size: usize,
        /// Filter to a device by (substring of) its name
        #[arg(long)]
        device: Option<String>,
    },
    /// Browser-based login to enable announcements (mints a durable refresh token)
    AnnounceLogin,
    /// Store Alexa Remote auth (refresh token and/or cookie string)
    AnnounceAuth {
        /// LWA refresh token (Atzr|...)
        #[arg(long)]
        refresh_token: Option<String>,
        /// Pre-baked website Cookie header string
        #[arg(long)]
        cookie: Option<String>,
    },
}

fn apply_overrides(cli: &Cli, cfg: &mut Config) {
    if let Some(r) = &cli.region {
        cfg.region = Region::from_str_lenient(r);
    }
    if let Some(v) = &cli.voice {
        cfg.voice = if v.eq_ignore_ascii_case("espeak") {
            Voice::Espeak
        } else {
            Voice::Piper
        };
    }
    if let Some(m) = &cli.model {
        cfg.model = m.clone();
    }
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
        Some(Command::Devices) => {
            let mut cfg = Config::load_or_default();
            apply_overrides(&cli, &mut cfg);
            let devices = remote::list_devices(&cfg, cli.verbose).await?;
            if cli.json {
                println!("{}", serde_json::to_string(&devices)?);
            } else if devices.is_empty() {
                println!("No devices found.");
            } else {
                for d in &devices {
                    println!(
                        "{}  [{}]  {}",
                        d.account_name,
                        if d.online { "online" } else { "offline" },
                        d.serial_number
                    );
                }
            }
            return Ok(());
        }
        Some(Command::Announce {
            message,
            device,
            all,
            title,
        }) => {
            let mut cfg = Config::load_or_default();
            apply_overrides(&cli, &mut cfg);
            // Default to all devices when no specific device is requested.
            let use_all = *all || device.is_none();
            let title = title.clone().unwrap_or_else(|| "Announcement".to_string());
            remote::announce(&cfg, message, &title, device.as_deref(), use_all, cli.verbose).await?;
            println!("Announcement sent.");
            return Ok(());
        }
        Some(Command::Say { message, device }) => {
            let mut cfg = Config::load_or_default();
            apply_overrides(&cli, &mut cfg);
            remote::say(&cfg, message, device, cli.verbose).await?;
            println!("Sent.");
            return Ok(());
        }
        Some(Command::History { size, device }) => {
            let mut cfg = Config::load_or_default();
            apply_overrides(&cli, &mut cfg);
            return remote::history(&cfg, *size, device.as_deref(), cli.verbose).await;
        }
        Some(Command::AnnounceLogin) => {
            let mut cfg = Config::load_or_default();
            apply_overrides(&cli, &mut cfg);
            return remote::login(&cfg, cli.verbose).await;
        }
        Some(Command::AnnounceAuth {
            refresh_token,
            cookie,
        }) => {
            remote::set_auth(refresh_token.clone(), cookie.clone())?;
            println!(
                "Saved Alexa Remote auth to {}",
                remote::RemoteState::path().display()
            );
            return Ok(());
        }
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
        println!(
            "{}",
            serde_json::json!({ "success": true, "result": answer })
        );
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

    if v {
        eprintln!("[tts] synthesizing with {:?}", cfg.voice);
    }
    // TTS may download a model and runs heavy synthesis; keep it off the async runtime
    // (reqwest::blocking + CPU work would otherwise panic/stall the Tokio worker).
    let voice = cfg.voice;
    let text_owned = text.to_string();
    let pcm_i16 = tokio::task::spawn_blocking(move || -> Result<Vec<i16>> {
        let backend = tts::backend_for(&voice)?;
        backend.synth(&text_owned)
    })
    .await
    .context("tts task panicked")??;
    let pcm_bytes = crate::audio::i16_to_le_bytes(&pcm_i16);

    if v {
        eprintln!(
            "[avs] sending {} bytes of LPCM to {}",
            pcm_bytes.len(),
            cfg.region.gateway_host()
        );
    }
    let mp3 = match send_recognize(&cfg, &pcm_bytes).await {
        Ok(mp3) => mp3,
        Err(e) => {
            // one retry with a forced token refresh (mirrors the Python tool)
            if v {
                eprintln!("[avs] first attempt failed ({e}); refreshing token + retry");
            }
            let token = auth::access_token(&cfg, true).await?;
            avs::recognize(&cfg, &token, &pcm_bytes).await?
        }
    };

    if cli.keep_artifacts || cli.output.is_some() {
        let dir = cli.output.clone().unwrap_or_else(|| ".".to_string());
        std::fs::create_dir_all(&dir).ok();
        std::fs::write(format!("{dir}/response.mp3"), &mp3).ok();
        std::fs::write(format!("{dir}/request.pcm"), &pcm_bytes).ok();
        if v {
            eprintln!("[artifacts] wrote response.mp3 / request.pcm to {dir}");
        }
    }

    // cache lookup/store
    let key = cache::key_for(&mp3);
    if !cli.no_cache && cfg.save_transcription {
        let c = cache::Cache::load();
        if let Some(hit) = c.get(&key) {
            if v {
                eprintln!("[stt] cache hit");
            }
            return Ok(hit);
        }
    }

    if v {
        eprintln!("[stt] transcribing with whisper {}", cfg.model);
    }
    // Whisper (and a possible model download) is blocking CPU work — run it off-runtime.
    let mp3_for_stt = mp3.clone();
    let cfg_for_stt = cfg.clone();
    let transcript =
        tokio::task::spawn_blocking(move || crate::stt::transcribe_mp3(&mp3_for_stt, &cfg_for_stt))
            .await
            .context("stt task panicked")??;

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
    println!(
        "Configuring AVS credentials (stored in {}).",
        Config::path().display()
    );
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
    Ok(if line.is_empty() {
        current.to_string()
    } else {
        line.to_string()
    })
}

async fn doctor(cli: &Cli) -> Result<()> {
    println!("alexa doctor — validating setup\n");

    let cfg = Config::load_or_default();
    check(
        "config present",
        cfg.is_complete(),
        "run `alexa configure` to set Client ID / Secret / Product ID",
    );
    if !cfg.is_complete() {
        return Ok(());
    }

    match auth::Tokens::load() {
        Ok(_) => check("tokens present", true, ""),
        Err(_) => {
            check("tokens present", false, "run `alexa login`");
            return Ok(());
        }
    }

    match auth::access_token(&cfg, false).await {
        Ok(_) => check("access token valid", true, ""),
        Err(e) => {
            check("access token valid", false, &format!("{e}"));
            return Ok(());
        }
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
