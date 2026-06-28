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
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use rand::RngCore;
use reqwest::cookie::{CookieStore, Jar};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
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
    pub device_family: String,
    pub account_name: String,
    pub customer_id: String,
    pub online: bool,
}

/// Whether a device can actually receive an announcement: Echo speakers and
/// Echo Show / Spot screens. Excludes Fire TV, tablets, Auto, Buds(non-speaker),
/// third-party AVS gear, and the virtual AVS "devices" — including those in an
/// announcement batch makes Amazon reject the whole request.
fn is_announceable(d: &Device) -> bool {
    matches!(
        d.device_family.to_ascii_uppercase().as_str(),
        "ECHO" | "KNIGHT" | "ROOK"
    ) && !d.device_type.is_empty()
        && !d.serial_number.is_empty()
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
                device_family: d
                    .get("deviceFamily")
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
// Browser-based device registration (durable refresh token)
// ---------------------------------------------------------------------------
//
// One-time OAuth "device registration" against Login-with-Amazon, mirroring the
// flow used by the `audible` Python library and Apollon77/alexa-cookie. The user
// signs in in their real browser (MFA/CAPTCHA/passkey safe), lands on the fixed
// /ap/maplanding redirect, and pastes the final URL back. The authorization code
// from that URL is exchanged at /auth/register (with a PKCE code_verifier) for a
// durable `Atnr|...` refresh token that drives the behaviors API.

/// The Alexa app device type registered against (iOS "Project Dee").
const REG_DEVICE_TYPE: &str = "A2IVLV5VM2W81";
/// App version reported during registration (matches APP_USER_AGENT).
const REG_APP_VERSION: &str = "2.2.651540.0";

/// PKCE code challenge for a verifier: base64url-no-pad(SHA256(verifier bytes)).
pub fn pkce_challenge(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

/// Build the OAuth `client_id` device id: lowercase hex of the ASCII bytes of
/// `"<serial>#A2IVLV5VM2W81"`, so it decodes back to `device:<serial>#<type>`.
pub fn device_id_for(serial: &str) -> String {
    hex::encode(format!("{serial}#{REG_DEVICE_TYPE}").as_bytes())
}

/// `openid.assoc_handle`/`pageId` suffix for non-`.com` marketplaces.
fn assoc_handle_suffix(tld: &str) -> &'static str {
    match tld {
        "co.uk" => "_uk",
        "co.jp" => "_jp",
        "de" => "_de",
        _ => "", // ".com" (tested NA path) and unknowns
    }
}

/// Build the Amazon `/ap/signin` device-registration OAuth URL (PKCE, S256).
/// All query *values* are percent-encoded; keys are left as-is.
pub fn build_signin_url(tld: &str, device_id: &str, code_challenge: &str, locale: &str) -> String {
    let base = format!("https://www.amazon.{tld}");
    let handle = format!("amzn_dp_project_dee_ios{}", assoc_handle_suffix(tld));
    let params: Vec<(&str, String)> = vec![
        ("openid.return_to", format!("{base}/ap/maplanding")),
        ("openid.assoc_handle", handle.clone()),
        (
            "openid.identity",
            "http://specs.openid.net/auth/2.0/identifier_select".to_string(),
        ),
        ("pageId", handle),
        ("accountStatusPolicy", "P1".to_string()),
        (
            "openid.claimed_id",
            "http://specs.openid.net/auth/2.0/identifier_select".to_string(),
        ),
        ("openid.mode", "checkid_setup".to_string()),
        // OpenID namespace identifier — an EXACT-match string, NOT a real endpoint.
        // Amazon's is the literal `http://...` (http, fixed www.amazon.com host). Using
        // https or a tld-templated host makes Amazon ignore the oauth2 extension and
        // return a plain id_res with no authorization_code.
        (
            "openid.ns.oa2",
            "http://www.amazon.com/ap/ext/oauth/2".to_string(),
        ),
        ("openid.oa2.client_id", format!("device:{device_id}")),
        (
            "openid.ns.pape",
            "http://specs.openid.net/extensions/pape/1.0".to_string(),
        ),
        ("openid.oa2.response_type", "code".to_string()),
        ("openid.ns", "http://specs.openid.net/auth/2.0".to_string()),
        ("openid.pape.max_auth_age", "0".to_string()),
        ("openid.oa2.scope", "device_auth_access".to_string()),
        ("openid.oa2.code_challenge_method", "S256".to_string()),
        ("openid.oa2.code_challenge", code_challenge.to_string()),
        ("language", locale.to_string()),
    ];
    let query = params
        .iter()
        .map(|(k, v)| format!("{k}={}", urlencoding::encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{base}/ap/signin?{query}")
}

/// Extract `openid.oa2.authorization_code` from a pasted maplanding URL (or bare
/// query string), percent-decoded. Returns None if the parameter is absent.
pub fn extract_auth_code(pasted_url: &str) -> Option<String> {
    let query = pasted_url
        .split_once('?')
        .map(|(_, q)| q)
        .unwrap_or(pasted_url);
    let query = query.split('#').next().unwrap_or(query);
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if k == "openid.oa2.authorization_code" && !v.is_empty() {
            return Some(
                urlencoding::decode(v)
                    .map(|c| c.into_owned())
                    .unwrap_or_else(|_| v.to_string()),
            );
        }
    }
    None
}

/// Fill `n` bytes from the thread RNG.
fn random_bytes(n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut buf);
    buf
}

/// The `map-md` device-descriptor cookie value (standard base64 of a small JSON).
fn map_md_cookie() -> String {
    let md = json!({
        "device_user_dictionary": [],
        "device_registration_data": { "software_version": "1" },
        "app_identifier": {
            "app_version": REG_APP_VERSION,
            "bundle_id": "com.amazon.echo"
        }
    });
    STANDARD.encode(serde_json::to_vec(&md).unwrap_or_default())
}

/// POST /auth/register: exchange the OAuth authorization code (+ PKCE verifier)
/// for a durable refresh token, returning `(refresh_token, website_cookie_header)`.
#[allow(clippy::too_many_arguments)]
async fn register(
    tld: &str,
    device_id: &str,
    serial: &str,
    authorization_code: &str,
    code_verifier: &str,
    frc: &str,
    locale: &str,
    verbose: bool,
) -> Result<(String, Option<String>)> {
    let url = format!("https://api.amazon.{tld}/auth/register");
    let cookie_header = format!("frc={frc}; map-md={}", map_md_cookie());
    let body = json!({
        "requested_extensions": ["device_info", "customer_info"],
        "cookies": { "website_cookies": [], "domain": format!(".amazon.{tld}") },
        "registration_data": {
            "domain": "Device",
            "app_version": REG_APP_VERSION,
            "device_type": REG_DEVICE_TYPE,
            "device_name": "alexa-cli",
            "os_version": "18.3.1",
            "device_serial": serial,
            "device_model": "iPhone",
            "app_name": "alexa-cli",
            "software_version": "1"
        },
        "auth_data": {
            "client_id": device_id,
            "authorization_code": authorization_code,
            "code_verifier": code_verifier,
            "code_algorithm": "SHA-256",
            "client_domain": "DeviceLegacy"
        },
        "user_context_map": { "frc": frc },
        "requested_token_type": ["bearer", "mac_dms", "website_cookies"]
    });
    if verbose {
        eprintln!("[remote] POST {url}");
    }
    let client = reqwest::Client::builder().build()?;
    let resp = client
        .post(&url)
        .header("User-Agent", APP_USER_AGENT)
        .header("x-amzn-identity-auth-domain", format!("api.amazon.{tld}"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("Accept-Language", locale)
        .header("Cookie", cookie_header)
        .body(serde_json::to_vec(&body)?)
        .send()
        .await
        .context("posting to auth/register")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if verbose {
        eprintln!("[remote] register status={status} body={}", snippet(&text));
    }
    let parsed: Value = serde_json::from_str(&text).map_err(|e| {
        anyhow!(
            "device registration returned non-JSON (status {status}): {} ({e})",
            snippet(&text)
        )
    })?;
    let tokens = parsed
        .get("response")
        .and_then(|r| r.get("success"))
        .and_then(|s| s.get("tokens"));
    let refresh_token = tokens
        .and_then(|t| t.get("bearer"))
        .and_then(|b| b.get("refresh_token"))
        .and_then(|x| x.as_str())
        .ok_or_else(|| {
            anyhow!(
                "device registration did not return a refresh token (status {status}); \
                 the authorization code may be stale or the pasted URL incomplete: {}",
                snippet(&text)
            )
        })?
        .to_string();
    let cookies = tokens
        .and_then(|t| t.get("website_cookies"))
        .and_then(extract_cookies);
    Ok((refresh_token, cookies))
}

/// Browser-based login: mint a durable refresh token and cache it in
/// `~/.alexa/alexa_remote.json` so announcements/say work without manual tokens.
pub async fn login(cfg: &Config, verbose: bool) -> Result<()> {
    use std::io::Write;

    let tld = tld_for(cfg.region).to_string();
    let locale = "en_US";

    let serial = hex::encode_upper(random_bytes(16));
    let code_verifier = URL_SAFE_NO_PAD.encode(random_bytes(32));
    let code_challenge = pkce_challenge(&code_verifier);
    let frc = STANDARD.encode(random_bytes(313));
    let device_id = device_id_for(&serial);
    let url = build_signin_url(&tld, &device_id, &code_challenge, locale);

    println!(
        "Opening your browser to sign in to Amazon. After you log in you will land on a\n\
         BLANK or 'page not found' page — that is expected. Copy the FULL URL from the\n\
         address bar and paste it here.\n"
    );
    println!("If the browser doesn't open, visit this URL manually:\n{url}\n");
    let _ = webbrowser::open(&url);

    print!("Paste the full URL here: ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .context("reading pasted URL from stdin")?;
    let pasted = line.trim();

    let auth_code = extract_auth_code(pasted).ok_or_else(|| {
        anyhow!(
            "no authorization code found in the pasted URL — copy the FULL address-bar URL \
             from the maplanding page (it must contain 'openid.oa2.authorization_code=')"
        )
    })?;

    let (refresh_token, cookies) = register(
        &tld,
        &device_id,
        &serial,
        &auth_code,
        &code_verifier,
        &frc,
        locale,
        verbose,
    )
    .await?;

    let mut state = RemoteState::load();
    state.refresh_token = Some(refresh_token);
    state.tld = tld;
    if cookies.is_some() {
        state.cookies = cookies;
    }
    // A fresh login invalidates any previously cached csrf.
    state.csrf = None;
    state.save()?;

    println!(
        "\nLogin successful — saved a durable refresh token to {}",
        RemoteState::path().display()
    );
    println!("Run `alexa devices` to verify.");

    // Best-effort verification; never fail the login if this errors.
    match get_devices(&mut state, verbose).await {
        Ok(devices) => println!("Verified: found {} Echo device(s).", devices.len()),
        Err(e) => {
            if verbose {
                eprintln!("[remote] device verification skipped: {e}");
            }
        }
    }
    Ok(())
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

/// POST a behavior sequence. Re-authenticates once on 401/403, and backs off and
/// retries on 429 (1→2→4→8s). On persistent throttling returns a `RATE_LIMITED`
/// error so callers can avoid hammering with a per-device fan-out.
async fn post_behavior(state: &mut RemoteState, sequence_json: &str, verbose: bool) -> Result<()> {
    let mut reauthed = false;
    let mut backoffs = 0u32;
    loop {
        let resp = send_behavior(state, sequence_json, verbose).await?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        if (status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN) && !reauthed {
            if verbose {
                eprintln!("[remote] behaviors returned {status}; re-authenticating and retrying");
            }
            state.cookies = None;
            state.csrf = None;
            get_devices(state, verbose).await?;
            reauthed = true;
            continue;
        }
        if status == StatusCode::TOO_MANY_REQUESTS && backoffs < 4 {
            let secs = 1u64 << backoffs; // 1, 2, 4, 8
            if verbose {
                eprintln!("[remote] 429 rate-limited; backing off {secs}s then retrying");
            }
            tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
            backoffs += 1;
            continue;
        }
        let body = resp.text().await.unwrap_or_default();
        if verbose {
            eprintln!("[remote] behaviors error body: {}", snippet(&body));
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            bail!("RATE_LIMITED: still throttled after retries: {}", snippet(&body));
        }
        bail!("behaviors/preview failed: {status}: {}", snippet(&body));
    }
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

    // Choose targets. A named target matches by substring; otherwise (--all, the
    // default) we select only online, announcement-capable Echo speakers — never
    // the tablets / Fire TVs / Buds / third-party / virtual devices in the account.
    let targets: Vec<Device> = if all || name.is_none() {
        let capable: Vec<Device> = devices
            .iter()
            .filter(|d| d.online && is_announceable(d))
            .cloned()
            .collect();
        if capable.is_empty() {
            bail!("no online, announcement-capable Echo devices found — run `alexa devices`");
        }
        capable
    } else {
        let matched = resolve_devices(&devices, name, false)?;
        // The API returns 200 even for offline / non-Echo targets, which then play
        // nothing. Warn so a silent "success" isn't mysterious.
        for d in &matched {
            if !d.online {
                eprintln!(
                    "note: \"{}\" is offline — it can't play an announcement until it reconnects.",
                    d.account_name
                );
            } else if !is_announceable(d) {
                eprintln!(
                    "note: \"{}\" is a {} device; only Echo speakers/Shows reliably play announcements.",
                    d.account_name, d.device_family
                );
            }
        }
        matched
    };

    // An announcement call carries a single customerId, so group by owner. Batch
    // each group; if Amazon rejects a batch, fall back to per-device so one
    // incompatible device can't sink the rest.
    let mut groups: std::collections::BTreeMap<String, Vec<Device>> =
        std::collections::BTreeMap::new();
    for d in targets {
        groups.entry(d.customer_id.clone()).or_default().push(d);
    }

    let mut sent = 0usize;
    let mut failed: Vec<String> = Vec::new();
    for (_customer, group) in groups {
        let (s, f) = announce_resilient(&mut state, message, title, &group, verbose).await;
        sent += s;
        failed.extend(f);
    }

    if sent == 0 {
        bail!(
            "announcement rejected for all {} target device(s) — try `--device <name>` for one Echo, or `-v` for details",
            failed.len()
        );
    }
    println!("Announced on {sent} device(s).");
    if !failed.is_empty() {
        eprintln!(
            "Skipped {} device(s) that rejected the announcement: {}",
            failed.len(),
            failed.join(", ")
        );
    }
    Ok(())
}

/// Announce to a set of same-customer devices: try one batch call, and if Amazon
/// rejects it, retry each device individually (skipping failures). Returns
/// (devices_announced, names_skipped).
async fn announce_resilient(
    state: &mut RemoteState,
    message: &str,
    title: &str,
    devices: &[Device],
    verbose: bool,
) -> (usize, Vec<String>) {
    let batch = build_announcement_sequence_json(message, title, devices);
    match post_behavior(state, &batch, verbose).await {
        Ok(()) => return (devices.len(), Vec::new()),
        Err(e) if e.to_string().contains("RATE_LIMITED") => {
            // Throttled even after backoff — fanning out per-device would only make
            // it worse. Report the group as skipped rather than hammering.
            if verbose {
                eprintln!("[remote] batch still rate-limited after backoff; not fanning out");
            }
            return (0, devices.iter().map(|d| d.account_name.clone()).collect());
        }
        Err(e) => {
            if verbose {
                eprintln!("[remote] batch rejected ({e}); retrying per device");
            }
        }
    }
    let mut sent = 0usize;
    let mut failed: Vec<String> = Vec::new();
    for d in devices {
        let seq = build_announcement_sequence_json(message, title, std::slice::from_ref(d));
        match post_behavior(state, &seq, verbose).await {
            Ok(()) => sent += 1,
            Err(e) => {
                if verbose {
                    eprintln!("[remote] {} rejected: {e}", d.account_name);
                }
                failed.push(d.account_name.clone());
            }
        }
        // Gentle pacing so a fan-out of per-device calls isn't rate-limited.
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    }
    (sent, failed)
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
// Voice history (text transcripts) — Alexa Privacy customer-history API
// ---------------------------------------------------------------------------
//
// Unlike the behaviors API (cookie `csrf` only), the privacy history endpoint
// needs a SECOND token, `anti-csrftoken-a2z`, scraped from the activity page's
// <meta name="csrf-token">. NOTE: announcements pushed via behaviors/preview do
// NOT appear here — this feed only records spoken ("Alexa, …") interactions.

#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub timestamp_ms: i64,
    pub device: String,
    pub transcript: String, // what the user said
    pub response: String,   // what Alexa said back
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Human "3m ago" / "2h ago" / "1d ago" relative time (no extra deps).
fn ago(ts_ms: i64, now_ms: i64) -> String {
    let secs = (now_ms - ts_ms).max(0) / 1000;
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}

/// Read a `name="value"` / `name='value'` attribute out of an HTML fragment.
fn find_attr(fragment: &str, attr: &str) -> Option<String> {
    for q in ['"', '\''] {
        let key = format!("{attr}={q}");
        if let Some(p) = fragment.find(&key) {
            let rest = &fragment[p + key.len()..];
            if let Some(end) = rest.find(q) {
                let val = &rest[..end];
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
    }
    None
}

/// Pull the anti-csrf token out of the activity page's
/// `<meta name="csrf-token" content="...">`. Anchors on the `csrf-token` name
/// attribute (the page also contains similar-looking JS token names) and reads
/// `content=` from a window around it, tolerant of attribute order/quoting.
pub fn extract_meta_csrf(html: &str) -> Option<String> {
    for anchor in [r#"name="csrf-token""#, r#"name='csrf-token'"#] {
        if let Some(i) = html.find(anchor) {
            // Window around the anchor, snapped outward to char boundaries so
            // slicing never panics on multi-byte UTF-8.
            let mut start = i.saturating_sub(200);
            while start > 0 && !html.is_char_boundary(start) {
                start -= 1;
            }
            let mut end = html.len().min(i + 300);
            while end < html.len() && !html.is_char_boundary(end) {
                end += 1;
            }
            if let Some(v) = find_attr(&html[start..end], "content") {
                return Some(v);
            }
        }
    }
    None
}

/// Parse the customer-history-records response into transcript entries.
pub fn parse_history(v: &Value) -> Vec<HistoryEntry> {
    let recs = match v
        .get("customerHistoryRecords")
        .and_then(|x| x.as_array())
    {
        Some(r) => r,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for r in recs {
        let timestamp_ms = r.get("timestamp").and_then(|x| x.as_i64()).unwrap_or(0);
        let device = r
            .get("device")
            .and_then(|d| d.get("deviceName"))
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let mut transcript = String::new();
        let mut response = String::new();
        if let Some(items) = r
            .get("voiceHistoryRecordItems")
            .and_then(|x| x.as_array())
        {
            for it in items {
                let kind = it
                    .get("recordItemType")
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                let text = it
                    .get("transcriptText")
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                if text.is_empty() {
                    continue;
                }
                let dst = match kind {
                    "CUSTOMER_TRANSCRIPT" | "ASR_REPLACEMENT_TEXT" => &mut transcript,
                    "ALEXA_RESPONSE" | "TTS_REPLACEMENT_TEXT" => &mut response,
                    _ => continue,
                };
                if !dst.is_empty() {
                    dst.push(' ');
                }
                dst.push_str(text);
            }
        }
        if transcript.is_empty() && response.is_empty() {
            continue;
        }
        out.push(HistoryEntry {
            timestamp_ms,
            device,
            transcript,
            response,
        });
    }
    out
}

/// GET the activity page and scrape the `anti-csrftoken-a2z` meta token.
async fn fetch_anti_csrf(state: &RemoteState, verbose: bool) -> Result<String> {
    let tld = &state.tld;
    let url = format!("https://www.amazon.{tld}/alexa-privacy/apd/activity?ref=activityHistory");
    if verbose {
        eprintln!("[remote] GET {url} (anti-csrf token)");
    }
    let client = reqwest::Client::builder().build()?;
    let resp = client
        .get(&url)
        .header("Cookie", full_cookie_header(state))
        .header("User-Agent", BROWSER_USER_AGENT)
        .header("Accept", "text/html,application/xhtml+xml")
        // We don't link gzip/brotli decoders, so insist on an uncompressed body.
        .header("Accept-Encoding", "identity")
        .header("DNT", "1")
        .send()
        .await
        .context("fetching activity page")?;
    let status = resp.status();
    let html = resp.text().await.unwrap_or_default();
    if verbose {
        eprintln!(
            "[remote] activity page status={status} len={} (token found={})",
            html.len(),
            html.contains("name=\"csrf-token\"")
        );
    }
    extract_meta_csrf(&html).ok_or_else(|| {
        anyhow!("could not find the anti-csrf token on the activity page (cookies may be expired — re-run `alexa announce-login`)")
    })
}

/// POST the customer-history-records query and return the parsed JSON.
async fn fetch_history_json(state: &RemoteState, anti_csrf: &str, verbose: bool) -> Result<Value> {
    let tld = &state.tld;
    let url = format!(
        "https://www.amazon.{tld}/alexa-privacy/apd/rvh/customer-history-records-v2?startTime=0&endTime=2147483647000&pageType=VOICE_HISTORY"
    );
    if verbose {
        eprintln!("[remote] POST {url}");
    }
    let client = reqwest::Client::builder().build()?;
    let resp = client
        .post(&url)
        .header("Content-Type", "application/json; charset=UTF-8")
        .header("csrf", state.csrf.clone().unwrap_or_default())
        .header("anti-csrftoken-a2z", anti_csrf)
        .header("Cookie", full_cookie_header(state))
        .header("User-Agent", BROWSER_USER_AGENT)
        .header("Referer", format!("https://www.amazon.{tld}/alexa-privacy/apd/activity"))
        .header("Accept-Encoding", "identity")
        .header("DNT", "1")
        .body(serde_json::to_vec(&json!({ "previousRequestToken": null }))?)
        .send()
        .await
        .context("posting to customer-history-records")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        if verbose {
            eprintln!("[remote] history error body: {}", snippet(&text));
        }
        bail!("voice history request failed: {status}: {}", snippet(&text));
    }
    serde_json::from_str(&text).context("parsing voice history JSON")
}

/// Fetch recent voice-history transcripts, newest first.
pub async fn history(
    cfg: &Config,
    size: usize,
    device_filter: Option<&str>,
    verbose: bool,
) -> Result<()> {
    let mut state = RemoteState::load();
    state.tld = tld_for(cfg.region).to_string();
    // Warm cookies + cookie-csrf (also re-auths if the session expired).
    let _ = get_devices(&mut state, verbose).await?;

    let anti_csrf = fetch_anti_csrf(&state, verbose).await?;
    let json = fetch_history_json(&state, &anti_csrf, verbose).await?;

    let mut entries = parse_history(&json);
    if let Some(f) = device_filter {
        let needle = f.to_lowercase();
        entries.retain(|e| e.device.to_lowercase().contains(&needle));
    }
    entries.truncate(size);

    if entries.is_empty() {
        println!("No voice history found.");
        return Ok(());
    }
    let now = now_ms();
    for e in &entries {
        println!("[{}] {}", ago(e.timestamp_ms, now), e.device);
        if !e.transcript.is_empty() {
            println!("   you:   {}", e.transcript);
        }
        if !e.response.is_empty() {
            println!("   alexa: {}", e.response);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests (pure builders / resolvers / parsers only — no network)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str, serial: &str, dtype: &str, cust: &str, online: bool) -> Device {
        dev_fam(name, serial, dtype, cust, online, "ECHO")
    }

    fn dev_fam(
        name: &str,
        serial: &str,
        dtype: &str,
        cust: &str,
        online: bool,
        family: &str,
    ) -> Device {
        Device {
            serial_number: serial.into(),
            device_type: dtype.into(),
            device_family: family.into(),
            account_name: name.into(),
            customer_id: cust.into(),
            online,
        }
    }

    #[test]
    fn is_announceable_only_echo_speakers() {
        assert!(is_announceable(&dev_fam("Kitchen", "S", "T", "C", true, "ECHO")));
        assert!(is_announceable(&dev_fam("Show", "S", "T", "C", true, "KNIGHT")));
        assert!(!is_announceable(&dev_fam("Tablet", "S", "T", "C", true, "TABLET")));
        assert!(!is_announceable(&dev_fam("FireTV", "S", "T", "C", true, "FIRE_TV")));
        assert!(!is_announceable(&dev_fam("CLI", "S", "T", "C", true, "UNKNOWN")));
        // Echo family but missing type/serial is not targetable.
        assert!(!is_announceable(&dev_fam("Bad", "", "", "C", true, "ECHO")));
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

    #[test]
    fn pkce_challenge_matches_rfc7636_vector() {
        // RFC 7636 Appendix B test vector.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            pkce_challenge(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn device_id_for_is_hex_of_serial_plus_type() {
        // "FF" -> 0x46,0x46 ; "#A2IVLV5VM2W81" -> known hex suffix.
        assert_eq!(
            device_id_for("FF"),
            "464623413249564c5635564d32573831"
        );
        // Decodes back to "<serial>#A2IVLV5VM2W81".
        let decoded = hex::decode(device_id_for("ABCD")).unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), "ABCD#A2IVLV5VM2W81");
    }

    #[test]
    fn build_signin_url_has_key_params() {
        let url = build_signin_url("com", "deadbeef", "CHALLENGE123", "en_US");
        assert!(url.starts_with("https://www.amazon.com/ap/signin?"));
        assert!(url.contains(
            "openid.return_to=https%3A%2F%2Fwww.amazon.com%2Fap%2Fmaplanding"
        ));
        assert!(url.contains("openid.oa2.scope=device_auth_access"));
        assert!(url.contains("openid.oa2.code_challenge_method=S256"));
        assert!(url.contains("openid.oa2.code_challenge=CHALLENGE123"));
        assert!(url.contains("openid.oa2.response_type=code"));
        assert!(url.contains("openid.oa2.client_id=device%3Adeadbeef"));
        assert!(url.contains("openid.assoc_handle=amzn_dp_project_dee_ios"));
        assert!(url.contains("language=en_US"));
        // Regression: the oauth2 namespace must be the exact http identifier, never
        // https (else Amazon drops the oa2 extension and returns no authorization_code).
        assert!(url.contains("openid.ns.oa2=http%3A%2F%2Fwww.amazon.com%2Fap%2Fext%2Foauth%2F2"));
        assert!(!url.contains("openid.ns.oa2=https"));
    }

    #[test]
    fn build_signin_url_non_com_handle_suffix() {
        let url = build_signin_url("co.uk", "abc", "C", "en_GB");
        assert!(url.starts_with("https://www.amazon.co.uk/ap/signin?"));
        assert!(url.contains("openid.assoc_handle=amzn_dp_project_dee_ios_uk"));
        assert!(url.contains("pageId=amzn_dp_project_dee_ios_uk"));
    }

    #[test]
    fn extract_auth_code_from_maplanding_url() {
        let url = "https://www.amazon.com/ap/maplanding?openid.oa2.authorization_code=ANabc123def&\
                   openid.assoc_handle=amzn_dp_project_dee_ios&openid.mode=id_res";
        assert_eq!(
            extract_auth_code(url),
            Some("ANabc123def".to_string())
        );
        // Bare query string also works, and percent-decoding is applied.
        assert_eq!(
            extract_auth_code("openid.oa2.authorization_code=AN%2Bcode"),
            Some("AN+code".to_string())
        );
        // Absent parameter -> None.
        assert!(extract_auth_code("https://www.amazon.com/ap/maplanding?foo=bar").is_none());
    }

    #[test]
    fn extract_meta_csrf_parses_token() {
        let html = r#"<html><head><meta name="csrf-token" content="abc123=="></head></html>"#;
        assert_eq!(extract_meta_csrf(html), Some("abc123==".to_string()));
        // content-before-name order is handled too.
        let single = "<meta content='tok-9' name='csrf-token'>";
        assert_eq!(extract_meta_csrf(single), Some("tok-9".to_string()));
        // similar-looking JS names must not match.
        let decoy = r#"<script>var newCSRFToken="x"; "anti-csrftoken-a2z":"y"</script>"#;
        assert!(extract_meta_csrf(decoy).is_none());
        assert!(extract_meta_csrf("<html>no token here</html>").is_none());
    }

    #[test]
    fn ago_formats_relative_time() {
        let now = 1_000_000_000_000i64;
        assert_eq!(ago(now - 5_000, now), "5s ago");
        assert_eq!(ago(now - 120_000, now), "2m ago");
        assert_eq!(ago(now - 7_200_000, now), "2h ago");
        assert_eq!(ago(now - 172_800_000, now), "2d ago");
    }

    #[test]
    fn parse_history_extracts_transcripts() {
        let v = serde_json::json!({
            "customerHistoryRecords": [
                {
                    "timestamp": 1700000000000i64,
                    "device": { "deviceName": "Kitchen" },
                    "voiceHistoryRecordItems": [
                        { "recordItemType": "CUSTOMER_TRANSCRIPT", "transcriptText": "what time is it" },
                        { "recordItemType": "ALEXA_RESPONSE", "transcriptText": "It is five p m" }
                    ]
                },
                {
                    "timestamp": 1700000001000i64,
                    "device": { "deviceName": "Office" },
                    "voiceHistoryRecordItems": [
                        { "recordItemType": "DEVICE_ARBITRATION", "transcriptText": "" }
                    ]
                }
            ]
        });
        let entries = parse_history(&v);
        assert_eq!(entries.len(), 1); // the empty/arbitration-only record is dropped
        assert_eq!(entries[0].device, "Kitchen");
        assert_eq!(entries[0].transcript, "what time is it");
        assert_eq!(entries[0].response, "It is five p m");
        assert!(parse_history(&serde_json::json!({})).is_empty());
    }
}
