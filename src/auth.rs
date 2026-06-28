use crate::config::Config;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
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

fn urlencode(s: &str) -> String {
    // minimal percent-encoding for query values
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
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

async fn exchange(_config: &Config, params: &[(&str, &str)]) -> Result<Tokens> {
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
        access_token: resp["access_token"]
            .as_str()
            .context("no access_token")?
            .to_string(),
        refresh_token: resp["refresh_token"]
            .as_str()
            .context("no refresh_token")?
            .to_string(),
        obtained_at: now_secs(),
    })
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 3 <= bytes.len() => match u8::from_str_radix(&s[i + 1..i + 3], 16) {
                Ok(b) => {
                    out.push(b);
                    i += 3;
                }
                Err(_) => {
                    out.push(b'%');
                    i += 1;
                }
            },
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub async fn login(config: &Config, port: u16) -> Result<()> {
    let redirect_uri = format!("http://localhost:{port}/auth");
    let url = authorize_url(config, &redirect_uri);
    println!("Opening browser to authorize. If it doesn't open, visit:\n{url}");
    let _ = webbrowser::open(&url);

    // Loopback server bound to localhost only; recover the auth code or surface an error.
    let server = tiny_http::Server::http(format!("127.0.0.1:{port}"))
        .map_err(|e| anyhow::anyhow!("failed to bind 127.0.0.1:{port}: {e}"))?;
    let code = loop {
        let request = server.recv()?;
        let url = request.url().to_string();
        let query = url.split_once('?').map(|(_, q)| q).unwrap_or("");

        let mut found_code: Option<String> = None;
        let mut error: Option<String> = None;
        for pair in query.split('&') {
            let (k, val) = pair.split_once('=').unwrap_or((pair, ""));
            match k {
                "code" => found_code = Some(percent_decode(val)),
                "error_description" => error = Some(percent_decode(val)),
                "error" => {
                    if error.is_none() {
                        error = Some(percent_decode(val));
                    }
                }
                _ => {}
            }
        }

        if let Some(c) = found_code.filter(|c| !c.is_empty()) {
            let _ = request.respond(tiny_http::Response::from_string(
                "Authorized. You can close this tab.",
            ));
            break c;
        }
        if let Some(err) = error {
            let _ = request.respond(tiny_http::Response::from_string(format!(
                "Authorization failed: {err}. You can close this tab."
            )));
            anyhow::bail!(
                "authorization failed: {err} (check the LWA security profile's Allowed Return URL is http://localhost:{port}/auth)"
            );
        }
        // Some other request (e.g. /favicon.ico) — keep waiting for the real callback.
        let _ = request.respond(tiny_http::Response::from_string(
            "Waiting for authorization...",
        ));
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
    println!(
        "Login successful — tokens saved to {}",
        Tokens::path().display()
    );
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

    #[test]
    fn authorize_url_contains_scope_and_product() {
        let cfg = Config {
            client_id: "cid".into(),
            product_id: "prod".into(),
            device_serial_number: "dsn".into(),
            ..Default::default()
        };
        let url = authorize_url(&cfg, "http://localhost:8086/auth");
        assert!(url.contains("client_id=cid"));
        assert!(url.contains("scope=alexa%3Aall") || url.contains("alexa:all"));
        assert!(url.contains("prod"));
        assert!(url.contains("dsn"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("redirect_uri="));
    }
}
