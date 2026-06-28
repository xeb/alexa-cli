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

fn default_model() -> String {
    "base.en".to_string()
}

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
        // Use default() rather than load_or_default() so the test is independent
        // of any real ~/.alexa/config.json that may exist on the host.
        let mut cfg = Config::default();
        assert!(!cfg.is_complete());
        cfg.client_id = "a".into();
        cfg.client_secret = "b".into();
        cfg.product_id = "c".into();
        assert!(cfg.is_complete());
    }

    #[test]
    fn roundtrip_serialization_uses_legacy_keys() {
        let mut cfg = Config::default();
        cfg.client_id = "id".into();
        let s = serde_json::to_string(&cfg).unwrap();
        assert!(s.contains("\"clientId\":\"id\""));
        let back: Config = serde_json::from_str(&s).unwrap();
        assert_eq!(back.client_id, "id");
    }
}
