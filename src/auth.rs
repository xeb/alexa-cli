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
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

pub fn is_expired(obtained_at: u64, now: u64) -> bool {
    now.saturating_sub(obtained_at) >= 3600
}

impl Tokens {
    pub(crate) fn path() -> std::path::PathBuf {
        Config::dir().join("tokens.json")
    }
    pub fn load() -> Result<Tokens> {
        let s = std::fs::read_to_string(Tokens::path()).context("no tokens — run `alexa login`")?;
        Ok(serde_json::from_str(&s)?)
    }
    pub fn save(&self) -> Result<()> {
        std::fs::create_dir_all(Config::dir())?;
        std::fs::write(Tokens::path(), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

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
        let t = Tokens {
            access_token: "a".into(),
            refresh_token: "r".into(),
            obtained_at: 42,
        };
        let s = serde_json::to_string(&t).unwrap();
        let back: Tokens = serde_json::from_str(&s).unwrap();
        assert_eq!(back.access_token, "a");
        assert_eq!(back.refresh_token, "r");
        assert_eq!(back.obtained_at, 42);
    }
}
