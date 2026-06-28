//! Unofficial Alexa Remote "behaviors" API: push Announcements / TTS to the
//! user's own Echo devices via the alexa.amazon.{tld} website endpoints
//! (cookie + csrf, NOT AVS). This is the same mechanism used by
//! Home Assistant's alexa_media_player / alexapy / alexa_remote_control.sh.
//!
//! Auth is headless: a Login-with-Amazon refresh token is exchanged for
//! website cookies, and a `csrf` token is harvested from a Set-Cookie on the
//! first authenticated GET. State is cached in ~/.alexa/alexa_remote.json.

use crate::auth::Tokens;
use crate::config::{Config, Region};
use anyhow::{anyhow, bail, Context, Result};
use reqwest::cookie::{CookieStore, Jar};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

/// User-Agent used for the LWA cookie-exchange call (mimics the iOS Alexa app).
const APP_USER_AGENT: &str = "AmazonWebView/Amazon Alexa/2.2.651540.0/iOS/18.3.1/iPhone";
/// Browser-ish User-Agent used against the alexa.amazon.{tld} website API.
const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120 Safari/537.36";

// ---------------------------------------------------------------------------
// Persistent state (~/.alexa/alexa_remote.json)
// ---------------------------------------------------------------------------

fn default_tld() -> String {
    "com".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteState {
    /// Refresh token to exchange for website cookies. Falls back to the AVS
    /// refresh token in ~/.alexa/tokens.json when unset.
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Amazon TLD, e.g. "com", "co.uk", "co.jp".
    #[serde(default = "default_tld")]
    pub tld: String,
    /// Account owner customer id (learned from the device list).
    #[serde(default)]
    pub customer_id: Option<String>,
    /// Serialized website Cookie header, e.g. "at-main=...; sess-at-main=...".
    #[serde(default)]
    pub cookies: Option<String>,
    /// The `csrf` token harvested from the website.
    #[serde(default)]
    pub csrf: Option<String>,
}

impl Default for RemoteState {
    fn default() -> Self {
        RemoteState {
            refresh_token: None,
            tld: default_tld(),
            customer_id: None,
            cookies: None,
            csrf: None,
        }
    }
}

impl RemoteState {
    pub fn path() -> PathBuf {
        Config::dir().join("alexa_remote.json")
    }
    pub fn load() -> RemoteState {
        match std::fs::read_to_string(RemoteState::path()) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => RemoteState::default(),
        }
    }
    pub fn save(&self) -> Result<()> {
        std::fs::create_dir_all(Config::dir())?;
        std::fs::write(RemoteState::path(), serde_json::to_string_pretty(self)?)
            .context("writing alexa_remote.json")?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Region / host mapping
// ---------------------------------------------------------------------------

/// Map an AVS region to the Amazon website TLD used by the unofficial API.
pub fn tld_for(region: Region) -> &'static str {
    match region {
        Region::Na => "com",
        Region::Eu => "co.uk",
        Region::Fe => "co.jp",
    }
}

// ---------------------------------------------------------------------------
// Device model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub serial_number: String,
    pub device_type: String,
    pub account_name: String,
    pub customer_id: String,
    pub online: bool,
}

/// Parse the `devices-v2/device` response. Entries without a serial number
/// (e.g. the virtual "this app" device) are skipped.
fn parse_devices(v: &Value) -> Vec<Device> {
    let arr = match v.get("devices").and_then(|d| d.as_array()) {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter()
        .filter_map(|d| {
            let serial = d.get("serialNumber").and_then(|x| x.as_str())?.to_string();
            if serial.is_empty() {
                return None;
            }
            Some(Device {
                serial_number: serial,
                device_type: d
                    .get("deviceType")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                account_name: d
                    .get("accountName")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                customer_id: d
                    .get("deviceOwnerCustomerId")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                online: d.get("online").and_then(|x| x.as_bool()).unwrap_or(false),
            })
        })
        .collect()
}

/// Resolve which devices to target. `all` -> every ONLINE device; otherwise a
/// case-insensitive substring match on the account (friendly) name.
pub fn resolve_devices(all_devices: &[Device], name: Option<&str>, all: bool) -> Result<Vec<Device>> {
    if all {
        let online: Vec<Device> = all_devices.iter().filter(|d| d.online).cloned().collect();
        if online.is_empty() {
            bail!("no online Echo devices found — run `alexa devices` to list");
        }
        return Ok(online);
    }
    if let Some(n) = name {
        let needle = n.to_lowercase();
        let matches: Vec<Device> = all_devices
            .iter()
            .filter(|d| d.account_name.to_lowercase().contains(&needle))
            .cloned()
            .collect();
        if matches.is_empty() {
            bail!("no device matches name '{n}' — run `alexa devices` to list available names");
        }
        return Ok(matches);
    }
    bail!("specify a device with --device <name> or target everything with --all");
}

// ---------------------------------------------------------------------------
// Pure payload builders (unit tested)
// ---------------------------------------------------------------------------

/// Build the stringified `sequenceJson` for an AlexaAnnouncement to one or more
/// devices. All targets share the account owner's customer id (first target's).
pub fn build_announcement_sequence_json(text: &str, title: &str, targets: &[Device]) -> String {
    let customer_id = targets
        .first()
        .map(|d| d.customer_id.clone())
        .unwrap_or_default();
    let devices: Vec<Value> = targets
        .iter()
        .map(|d| {
            json!({
                "deviceSerialNumber": d.serial_number,
                "deviceTypeId": d.device_type,
            })
        })
        .collect();
    let sequence = json!({
        "@type": "com.amazon.alexa.behaviors.model.Sequence",
        "startNode": {
            "@type": "com.amazon.alexa.behaviors.model.OpaquePayloadOperationNode",
            "type": "AlexaAnnouncement",
            "operationPayload": {
                "expireAfter": "PT5S",
                "content": [{
                    "locale": "en-US",
                    "display": { "title": title, "body": text },
                    "speak": { "type": "text", "value": text }
                }],
                "target": {
                    "customerId": customer_id,
                    "devices": devices
                },
                "skillId": "amzn1.ask.1p.routines.messaging"
            }
        }
    });
    serde_json::to_string(&sequence).unwrap_or_default()
}

/// Build the stringified `sequenceJson` for an Alexa.Speak (TTS) to one device.
pub fn build_speak_sequence_json(text: &str, device: &Device) -> String {
    let sequence = json!({
        "@type": "com.amazon.alexa.behaviors.model.Sequence",
        "startNode": {
            "@type": "com.amazon.alexa.behaviors.model.OpaquePayloadOperationNode",
            "type": "Alexa.Speak",
            "operationPayload": {
                "deviceType": device.device_type,
                "deviceSerialNumber": device.serial_number,
                "customerId": device.customer_id,
                "locale": "en-US",
                "textToSpeak": text
            },
            "skillId": "amzn1.ask.1p.saysomething"
        }
    });
    serde_json::to_string(&sequence).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// JSON / cookie helpers
// ---------------------------------------------------------------------------

/// Truncate a body to a short, UTF-8-safe snippet for error/diagnostic output.
fn snippet(s: &str) -> String {
    let trimmed = s.trim();
    let short: String = trimmed.chars().take(400).collect();
    if short.len() < trimmed.len() {
        format!("{short}…")
    } else {
        short
    }
}

/// Walk arbitrary JSON, collecting any objects (inside arrays) that carry both
/// a "Name" and a "Value" string field. Used to extract website cookies from
/// the cookie-exchange response defensively (its exact shape may vary).
fn collect_cookies(v: &Value, out: &mut Vec<(String, String)>) {
    match v {
        Value::Array(arr) => {
            for item in arr {
                match (
                    item.get("Name").and_then(|x| x.as_str()),
                    item.get("Value").and_then(|x| x.as_str()),
                ) {
                    (Some(name), Some(val)) => out.push((name.to_string(), val.to_string())),
                    _ => collect_cookies(item, out),
                }
            }
        }
        Value::Object(map) => {
            for val in map.values() {
                collect_cookies(val, out);
            }
        }
        _ => {}
    }
}

/// Build a "Name1=Value1; Name2=Value2" Cookie header from the exchange JSON.
fn extract_cookies(v: &Value) -> Option<String> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    collect_cookies(v, &mut pairs);
    if pairs.is_empty() {
        return None;
    }
    Some(
        pairs
            .iter()
            .map(|(n, val)| format!("{n}={val}"))
            .collect::<Vec<_>>()
            .join("; "),
    )
}

/// Pull the `csrf` value out of a Cookie-style header string.
fn extract_csrf(cookie_header: &str) -> Option<String> {
    for part in cookie_header.split(';') {
        if let Some(v) = part.trim().strip_prefix("csrf=") {
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// x-www-form-urlencoded body builder.
fn form_encode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// The full Cookie header used for behavior POSTs: website cookies + csrf.
fn full_cookie_header(state: &RemoteState) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(c) = state.cookies.as_deref().filter(|s| !s.is_empty()) {
        parts.push(c.to_string());
    }
    if let Some(csrf) = &state.csrf {
        parts.push(format!("csrf={csrf}"));
    }
    parts.join("; ")
}

// ---------------------------------------------------------------------------
// Auth flow
// ---------------------------------------------------------------------------

/// Pick the refresh token: explicit RemoteState value, else the AVS one.
fn refresh_token(state: &RemoteState) -> Result<String> {
    if let Some(rt) = state.refresh_token.as_deref().filter(|s| !s.is_empty()) {
        return Ok(rt.to_string());
    }
    match Tokens::load() {
        Ok(t) if !t.refresh_token.is_empty() => Ok(t.refresh_token),
        _ => bail!(
            "no Alexa Remote auth configured — run `alexa announce-auth --refresh-token <Atzr|...>` \
             (or complete `alexa login` so the AVS refresh token can be reused)"
        ),
    }
}

/// Exchange a refresh token for website auth cookies (headless login).
async fn exchange_token_for_cookies(
    refresh_token: &str,
    tld: &str,
    verbose: bool,
) -> Result<String> {
    let url = format!("https://www.amazon.{tld}/ap/exchangetoken/cookies");
    let body = form_encode(&[
        ("source_token_type", "refresh_token"),
        ("source_token", refresh_token),
        ("requested_token_type", "auth_cookies"),
        ("domain", &format!(".amazon.{tld}")),
        ("app_name", "Amazon Alexa"),
    ]);
    if verbose {
        eprintln!("[remote] POST {url}");
    }
    let client = reqwest::Client::builder().build()?;
    let resp = client
        .post(&url)
        .header("User-Agent", APP_USER_AGENT)
        .header("x-amzn-identity-auth-domain", format!("api.amazon.{tld}"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .context("posting to exchangetoken/cookies")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if verbose {
        eprintln!("[remote] exchangetoken status={status} body={}", snippet(&text));
    }
    let parsed: Value = serde_json::from_str(&text).map_err(|e| {
        anyhow!(
            "cookie exchange returned non-JSON (status {status}): {} ({e})",
            snippet(&text)
        )
    })?;
    extract_cookies(&parsed).ok_or_else(|| {
        anyhow!(
            "could not find website cookies in exchange response (status {status}); \
             the refresh token may be invalid or expired: {}",
            snippet(&text)
        )
    })
}

/// Ensure we have website cookies, exchanging the refresh token if needed.
async fn ensure_cookies(state: &mut RemoteState, verbose: bool) -> Result<()> {
    if state.cookies.as_deref().map(|s| !s.is_empty()).unwrap_or(false) {
        return Ok(());
    }
    let rt = refresh_token(state)?;
    let cookies = exchange_token_for_cookies(&rt, &state.tld, verbose).await?;
    state.cookies = Some(cookies);
    state.save()?;
    Ok(())
}

/// Read the csrf token back out of a cookie jar for the given base URL.
fn read_csrf(jar: &Jar, url: &reqwest::Url) -> Option<String> {
    let header = jar.cookies(url)?;
    extract_csrf(header.to_str().ok()?)
}

/// GET the device list, harvesting the `csrf` token from Set-Cookie. Returns
/// the csrf, the parsed devices, and the learned account customer id.
async fn fetch_csrf_and_devices(
    state: &RemoteState,
    verbose: bool,
) -> Result<(String, Vec<Device>, Option<String>)> {
    let tld = &state.tld;
    let alexa_host = format!("alexa.amazon.{tld}");
    let base_url: reqwest::Url = format!("https://{alexa_host}/").parse()?;
    let cookies = state
        .cookies
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("no website cookies; run `alexa announce-auth --refresh-token ...`"))?;

    // Seed a jar with the website cookies, then let reqwest capture the csrf
    // Set-Cookie automatically.
    let jar = Arc::new(Jar::default());
    for part in cookies.split("; ") {
        if !part.is_empty() {
            jar.add_cookie_str(part, &base_url);
        }
    }
    let client = reqwest::Client::builder()
        .cookie_provider(jar.clone())
        .build()?;

    let devices_url = format!("https://{alexa_host}/api/devices-v2/device?cached=false");
    if verbose {
        eprintln!("[remote] GET {devices_url}");
    }
    let mut req = client
        .get(&devices_url)
        .header("User-Agent", BROWSER_USER_AGENT)
        .header("DNT", "1");
    if let Some(csrf) = &state.csrf {
        req = req.header("csrf", csrf.clone());
    }
    let resp = req.send().await.context("fetching device list")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if verbose {
        eprintln!("[remote] devices status={status}");
    }
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        bail!(
            "device list returned {status}; website cookies look expired — re-run \
             `alexa announce-auth --refresh-token <Atzr|...>`"
        );
    }

    // csrf is usually set on the devices GET; if not, poke a couple of endpoints.
    let mut csrf = read_csrf(&jar, &base_url);
    if csrf.is_none() {
        for path in ["/api/bootstrap?version=0", "/api/language"] {
            let url = format!("https://{alexa_host}{path}");
            if verbose {
                eprintln!("[remote] GET {url} (csrf)");
            }
            let _ = client
                .get(&url)
                .header("User-Agent", BROWSER_USER_AGENT)
                .header("DNT", "1")
                .send()
                .await;
            csrf = read_csrf(&jar, &base_url);
            if csrf.is_some() {
                break;
            }
        }
    }
    let csrf = csrf.ok_or_else(|| {
        anyhow!(
            "could not obtain a csrf token from {alexa_host} (status {status}); \
             cookies may be expired — re-run `alexa announce-auth --refresh-token <Atzr|...>`"
        )
    })?;

    let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let devices = parse_devices(&parsed);
    let customer_id = devices
        .first()
        .map(|d| d.customer_id.clone())
        .filter(|s| !s.is_empty());
    Ok((csrf, devices, customer_id))
}

/// Fetch the device list, transparently (re)acquiring cookies + csrf. Retries
/// once with fresh cookies if the first attempt fails (e.g. expired session).
async fn get_devices(state: &mut RemoteState, verbose: bool) -> Result<Vec<Device>> {
    ensure_cookies(state, verbose).await?;
    match fetch_csrf_and_devices(state, verbose).await {
        Ok((csrf, devices, customer_id)) => {
            state.csrf = Some(csrf);
            if customer_id.is_some() {
                state.customer_id = customer_id;
            }
            state.save()?;
            Ok(devices)
        }
        Err(e) => {
            if verbose {
                eprintln!("[remote] device fetch failed ({e}); refreshing cookies and retrying");
            }
            state.cookies = None;
            state.csrf = None;
            ensure_cookies(state, verbose).await?;
            let (csrf, devices, customer_id) = fetch_csrf_and_devices(state, verbose).await?;
            state.csrf = Some(csrf);
            if customer_id.is_some() {
                state.customer_id = customer_id;
            }
            state.save()?;
            Ok(devices)
        }
    }
}

// ---------------------------------------------------------------------------
// Behavior POST
// ---------------------------------------------------------------------------

async fn send_behavior(
    state: &RemoteState,
    sequence_json: &str,
    verbose: bool,
) -> Result<reqwest::Response> {
    let tld = &state.tld;
    let alexa_host = format!("alexa.amazon.{tld}");
    let url = format!("https://{alexa_host}/api/behaviors/preview");
    let body = json!({
        "behaviorId": "PREVIEW",
        "status": "ENABLED",
        "sequenceJson": sequence_json,
    });
    let csrf = state.csrf.clone().unwrap_or_default();
    if verbose {
        eprintln!("[remote] POST {url}");
    }
    let client = reqwest::Client::builder().build()?;
    let resp = client
        .post(&url)
        .header("Content-Type", "application/json; charset=UTF-8")
        .header("csrf", csrf)
        .header("Cookie", full_cookie_header(state))
        .header("User-Agent", BROWSER_USER_AGENT)
        .header("DNT", "1")
        .body(serde_json::to_vec(&body)?)
        .send()
        .await
        .context("posting to behaviors/preview")?;
    Ok(resp)
}

async fn check_behavior_response(resp: reqwest::Response, verbose: bool) -> Result<()> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let body = resp.text().await.unwrap_or_default();
    if verbose {
        eprintln!("[remote] behaviors error body: {}", snippet(&body));
    }
    bail!("behaviors/preview failed: {status}: {}", snippet(&body));
}

/// POST a behavior sequence, re-authenticating once on 401/403.
async fn post_behavior(state: &mut RemoteState, sequence_json: &str, verbose: bool) -> Result<()> {
    let resp = send_behavior(state, sequence_json, verbose).await?;
    let status = resp.status();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        if verbose {
            eprintln!("[remote] behaviors returned {status}; re-authenticating and retrying once");
        }
        state.cookies = None;
        state.csrf = None;
        // Refresh cookies + csrf (also re-saves state).
        let _ = get_devices(state, verbose).await?;
        let resp2 = send_behavior(state, sequence_json, verbose).await?;
        return check_behavior_response(resp2, verbose).await;
    }
    check_behavior_response(resp, verbose).await
}

// ---------------------------------------------------------------------------
// High-level operations
// ---------------------------------------------------------------------------

/// List the account's Echo devices.
pub async fn list_devices(cfg: &Config, verbose: bool) -> Result<Vec<Device>> {
    let mut state = RemoteState::load();
    state.tld = tld_for(cfg.region).to_string();
    get_devices(&mut state, verbose).await
}

/// Send an AlexaAnnouncement to the matching / all online devices.
pub async fn announce(
    cfg: &Config,
    message: &str,
    title: &str,
    name: Option<&str>,
    all: bool,
    verbose: bool,
) -> Result<()> {
    let mut state = RemoteState::load();
    state.tld = tld_for(cfg.region).to_string();
    let devices = get_devices(&mut state, verbose).await?;
    let targets = resolve_devices(&devices, name, all)?;
    let sequence = build_announcement_sequence_json(message, title, &targets);
    post_behavior(&mut state, &sequence, verbose).await
}

/// Speak (TTS) a message on a single device, selected by name.
pub async fn say(cfg: &Config, message: &str, name: &str, verbose: bool) -> Result<()> {
    let mut state = RemoteState::load();
    state.tld = tld_for(cfg.region).to_string();
    let devices = get_devices(&mut state, verbose).await?;
    let targets = resolve_devices(&devices, Some(name), false)?;
    let device = targets
        .first()
        .cloned()
        .context("no matching device for `say`")?;
    let sequence = build_speak_sequence_json(message, &device);
    post_behavior(&mut state, &sequence, verbose).await
}

/// Persist auth material (refresh token and/or a pre-baked cookie string).
pub fn set_auth(refresh_token: Option<String>, cookie: Option<String>) -> Result<()> {
    let mut state = RemoteState::load();
    if refresh_token.is_some() {
        state.refresh_token = refresh_token;
    }
    if cookie.is_some() {
        state.cookies = cookie;
        // A new cookie set invalidates any cached csrf.
        state.csrf = None;
    }
    state.save()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests (pure builders / resolvers / parsers only — no network)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str, serial: &str, dtype: &str, cust: &str, online: bool) -> Device {
        Device {
            serial_number: serial.into(),
            device_type: dtype.into(),
            account_name: name.into(),
            customer_id: cust.into(),
            online,
        }
    }

    #[test]
    fn tld_mapping() {
        assert_eq!(tld_for(Region::Na), "com");
        assert_eq!(tld_for(Region::Eu), "co.uk");
        assert_eq!(tld_for(Region::Fe), "co.jp");
    }

    #[test]
    fn announcement_sequence_shape() {
        let targets = vec![
            dev("Kitchen", "SN1", "TYPE1", "CUST", true),
            dev("Bedroom", "SN2", "TYPE2", "CUST", true),
        ];
        let s = build_announcement_sequence_json("hello world", "My Title", &targets);
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["@type"], "com.amazon.alexa.behaviors.model.Sequence");
        assert_eq!(
            v["startNode"]["@type"],
            "com.amazon.alexa.behaviors.model.OpaquePayloadOperationNode"
        );
        assert_eq!(v["startNode"]["type"], "AlexaAnnouncement");
        let payload = &v["startNode"]["operationPayload"];
        assert_eq!(payload["content"][0]["speak"]["value"], "hello world");
        assert_eq!(payload["content"][0]["display"]["title"], "My Title");
        assert_eq!(payload["content"][0]["display"]["body"], "hello world");
        assert_eq!(payload["target"]["customerId"], "CUST");
        assert_eq!(payload["skillId"], "amzn1.ask.1p.routines.messaging");
        let serials: Vec<&str> = payload["target"]["devices"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["deviceSerialNumber"].as_str().unwrap())
            .collect();
        assert!(serials.contains(&"SN1"));
        assert!(serials.contains(&"SN2"));
    }

    #[test]
    fn speak_sequence_shape() {
        let d = dev("Office", "SNX", "TYPEX", "CUSTX", true);
        let s = build_speak_sequence_json("speak this", &d);
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["@type"], "com.amazon.alexa.behaviors.model.Sequence");
        assert_eq!(v["startNode"]["type"], "Alexa.Speak");
        assert_eq!(v["startNode"]["skillId"], "amzn1.ask.1p.saysomething");
        let payload = &v["startNode"]["operationPayload"];
        assert_eq!(payload["textToSpeak"], "speak this");
        assert_eq!(payload["deviceSerialNumber"], "SNX");
        assert_eq!(payload["deviceType"], "TYPEX");
        assert_eq!(payload["customerId"], "CUSTX");
        assert_eq!(payload["locale"], "en-US");
    }

    #[test]
    fn resolve_all_returns_online_only() {
        let devices = vec![
            dev("Kitchen", "SN1", "T", "C", true),
            dev("Bedroom", "SN2", "T", "C", false),
        ];
        let r = resolve_devices(&devices, None, true).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].serial_number, "SN1");
    }

    #[test]
    fn resolve_by_name_is_case_insensitive_substring() {
        let devices = vec![
            dev("Kitchen Echo", "SN1", "T", "C", true),
            dev("Bedroom Dot", "SN2", "T", "C", true),
        ];
        let r = resolve_devices(&devices, Some("kitchen"), false).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].serial_number, "SN1");
        assert!(resolve_devices(&devices, Some("garage"), false).is_err());
    }

    #[test]
    fn resolve_without_name_or_all_errors() {
        let devices = vec![dev("Kitchen", "SN1", "T", "C", true)];
        assert!(resolve_devices(&devices, None, false).is_err());
    }

    #[test]
    fn parse_devices_skips_serial_less_entries() {
        let json = json!({
            "devices": [
                { "serialNumber": "SN1", "deviceType": "T1", "accountName": "Kitchen",
                  "deviceOwnerCustomerId": "CUST", "online": true },
                { "serialNumber": null, "deviceType": "APP", "accountName": "App", "online": false }
            ]
        });
        let devs = parse_devices(&json);
        assert_eq!(devs.len(), 1);
        assert_eq!(devs[0].account_name, "Kitchen");
        assert_eq!(devs[0].customer_id, "CUST");
        assert!(devs[0].online);
    }

    #[test]
    fn extract_cookies_walks_nested_json() {
        let json = json!({
            "response": { "tokens": { "cookies": { ".amazon.com": [
                {"Name":"at-main","Value":"AAA"},
                {"Name":"sess-at-main","Value":"BBB"}
            ]}}}
        });
        let c = extract_cookies(&json).unwrap();
        assert!(c.contains("at-main=AAA"));
        assert!(c.contains("sess-at-main=BBB"));
    }

    #[test]
    fn extract_cookies_none_when_absent() {
        let json = json!({ "response": { "error": "bad token" } });
        assert!(extract_cookies(&json).is_none());
    }

    #[test]
    fn extract_csrf_finds_token() {
        assert_eq!(
            extract_csrf("foo=bar; csrf=12345; baz=qux"),
            Some("12345".to_string())
        );
        assert_eq!(extract_csrf("foo=bar"), None);
    }

    #[test]
    fn form_encode_percent_encodes() {
        let s = form_encode(&[("app_name", "Amazon Alexa"), ("domain", ".amazon.com")]);
        assert!(s.contains("app_name=Amazon%20Alexa"));
        assert!(s.contains("domain=.amazon.com"));
    }
}
