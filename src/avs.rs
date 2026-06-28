use anyhow::Result;

pub fn recognize_event_json(message_id: &str, dialog_request_id: &str) -> String {
    serde_json::json!({
        "context": [],
        "event": {
            "header": {
                "namespace": "SpeechRecognizer",
                "name": "Recognize",
                "messageId": message_id,
                "dialogRequestId": dialog_request_id
            },
            "payload": {
                "profile": "CLOSE_TALK",
                "format": "AUDIO_L16_RATE_16000_CHANNELS_1",
                "initiator": { "type": "TAP" }
            }
        }
    })
    .to_string()
}

pub fn synchronize_state_json(message_id: &str) -> String {
    serde_json::json!({
        "context": [],
        "event": {
            "header": {
                "namespace": "System",
                "name": "SynchronizeState",
                "messageId": message_id
            },
            "payload": {}
        }
    })
    .to_string()
}

pub fn build_recognize_multipart(event_json: &str, pcm: &[u8], boundary: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let push = |out: &mut Vec<u8>, s: &str| out.extend_from_slice(s.as_bytes());

    push(&mut out, &format!("--{boundary}\r\n"));
    push(&mut out, "Content-Disposition: form-data; name=\"metadata\"\r\n");
    push(&mut out, "Content-Type: application/json; charset=UTF-8\r\n\r\n");
    push(&mut out, event_json);
    push(&mut out, "\r\n");

    push(&mut out, &format!("--{boundary}\r\n"));
    push(&mut out, "Content-Disposition: form-data; name=\"audio\"\r\n");
    push(&mut out, "Content-Type: application/octet-stream\r\n\r\n");
    out.extend_from_slice(pcm);
    push(&mut out, "\r\n");

    push(&mut out, &format!("--{boundary}--\r\n"));
    out
}

#[derive(Debug, Clone)]
pub struct Part {
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Part {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

fn boundary_from_content_type(content_type: &str) -> Option<String> {
    let lower = content_type.to_ascii_lowercase();
    let idx = lower.find("boundary=")?;
    let raw = &content_type[idx + "boundary=".len()..];
    let raw = raw.trim().trim_matches('"');
    let end = raw.find(';').unwrap_or(raw.len());
    Some(raw[..end].trim().trim_matches('"').to_string())
}

fn find_subsequence(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from > haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

pub fn parse_multipart_related(content_type: &str, body: &[u8]) -> Result<Vec<Part>> {
    let boundary = boundary_from_content_type(content_type)
        .ok_or_else(|| anyhow::anyhow!("no boundary in content-type: {content_type}"))?;
    let delim = format!("--{boundary}");
    let delim_bytes = delim.as_bytes();

    let mut parts = Vec::new();
    let mut cursor = match find_subsequence(body, delim_bytes, 0) {
        Some(p) => p + delim_bytes.len(),
        None => return Ok(parts),
    };

    loop {
        // After a boundary: either "--" (end) or CRLF then part.
        if body[cursor..].starts_with(b"--") {
            break;
        }
        // Skip the CRLF after the boundary.
        if body[cursor..].starts_with(b"\r\n") {
            cursor += 2;
        }
        // Headers end at CRLFCRLF.
        let header_end = find_subsequence(body, b"\r\n\r\n", cursor)
            .ok_or_else(|| anyhow::anyhow!("malformed part: no header terminator"))?;
        let header_blob = String::from_utf8_lossy(&body[cursor..header_end]);
        let headers: Vec<(String, String)> = header_blob
            .split("\r\n")
            .filter(|l| !l.is_empty())
            .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
            .collect();

        let content_start = header_end + 4;
        let next_boundary = find_subsequence(body, delim_bytes, content_start)
            .ok_or_else(|| anyhow::anyhow!("malformed part: no closing boundary"))?;
        // Content runs up to the CRLF that precedes the boundary.
        let mut content_end = next_boundary;
        if body[..content_end].ends_with(b"\r\n") {
            content_end -= 2;
        }
        parts.push(Part {
            headers,
            body: body[content_start..content_end].to_vec(),
        });
        cursor = next_boundary + delim_bytes.len();
    }

    Ok(parts)
}

pub fn extract_speak_audio(parts: &[Part]) -> Result<Vec<u8>> {
    // Find a JSON directive part whose Speak payload.url is "cid:<id>".
    let mut cid: Option<String> = None;
    for p in parts {
        let is_json = p
            .header("Content-Type")
            .map(|c| c.contains("application/json"))
            .unwrap_or(false);
        if !is_json {
            continue;
        }
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&p.body) {
            let d = &v["directive"];
            if d["header"]["name"] == "Speak" {
                if let Some(url) = d["payload"]["url"].as_str() {
                    if let Some(id) = url.strip_prefix("cid:") {
                        cid = Some(id.to_string());
                        break;
                    }
                }
            }
        }
    }
    let cid = cid.ok_or_else(|| anyhow::anyhow!("no SpeechSynthesizer.Speak directive in response"))?;

    for p in parts {
        if let Some(content_id) = p.header("Content-ID") {
            let normalized = content_id.trim().trim_start_matches('<').trim_end_matches('>');
            if normalized == cid {
                return Ok(p.body.clone());
            }
        }
    }
    anyhow::bail!("no audio attachment matching cid:{cid}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognize_event_has_required_fields() {
        let j = recognize_event_json("mid-1", "drid-1");
        let v: serde_json::Value = serde_json::from_str(&j).unwrap();
        assert_eq!(v["event"]["header"]["namespace"], "SpeechRecognizer");
        assert_eq!(v["event"]["header"]["name"], "Recognize");
        assert_eq!(v["event"]["header"]["messageId"], "mid-1");
        assert_eq!(v["event"]["header"]["dialogRequestId"], "drid-1");
        assert_eq!(v["event"]["payload"]["profile"], "CLOSE_TALK");
        assert_eq!(v["event"]["payload"]["format"], "AUDIO_L16_RATE_16000_CHANNELS_1");
        assert_eq!(v["event"]["payload"]["initiator"]["type"], "TAP");
    }

    #[test]
    fn synchronize_state_is_valid() {
        let j = synchronize_state_json("mid-2");
        let v: serde_json::Value = serde_json::from_str(&j).unwrap();
        assert_eq!(v["event"]["header"]["namespace"], "System");
        assert_eq!(v["event"]["header"]["name"], "SynchronizeState");
        assert!(v["context"].is_array());
    }

    #[test]
    fn multipart_body_has_both_parts_and_raw_audio() {
        let body = build_recognize_multipart("{\"event\":true}", &[0xDE, 0xAD, 0xBE, 0xEF], "BOUNDARY");
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("--BOUNDARY\r\n"));
        assert!(text.contains("Content-Disposition: form-data; name=\"metadata\""));
        assert!(text.contains("Content-Type: application/json"));
        assert!(text.contains("Content-Disposition: form-data; name=\"audio\""));
        assert!(text.contains("Content-Type: application/octet-stream"));
        assert!(text.ends_with("--BOUNDARY--\r\n"));
        // raw audio bytes present verbatim:
        assert!(body.windows(4).any(|w| w == [0xDE, 0xAD, 0xBE, 0xEF]));
    }
}

#[cfg(test)]
mod response_tests {
    use super::*;

    fn make_fixture() -> (String, Vec<u8>) {
        let boundary = "RESP";
        let directive = serde_json::json!({
            "directive": {
                "header": { "namespace": "SpeechSynthesizer", "name": "Speak" },
                "payload": { "url": "cid:audio-123", "format": "AUDIO_MPEG" }
            }
        }).to_string();
        let mp3 = vec![0xFF, 0xFB, 0x10, 0x00, 1, 2, 3, 4]; // fake mp3 bytes
        let mut body = Vec::new();
        let push = |b: &mut Vec<u8>, s: &str| b.extend_from_slice(s.as_bytes());
        push(&mut body, &format!("--{boundary}\r\n"));
        push(&mut body, "Content-Type: application/json; charset=UTF-8\r\n\r\n");
        push(&mut body, &directive);
        push(&mut body, "\r\n");
        push(&mut body, &format!("--{boundary}\r\n"));
        push(&mut body, "Content-Type: application/octet-stream\r\n");
        push(&mut body, "Content-ID: audio-123\r\n\r\n");
        body.extend_from_slice(&mp3);
        push(&mut body, "\r\n");
        push(&mut body, &format!("--{boundary}--\r\n"));
        (format!("multipart/related; boundary={boundary}"), body)
    }

    #[test]
    fn parses_all_parts() {
        let (ct, body) = make_fixture();
        let parts = parse_multipart_related(&ct, &body).unwrap();
        assert_eq!(parts.len(), 2);
    }

    #[test]
    fn extracts_speak_mp3_via_cid() {
        let (ct, body) = make_fixture();
        let parts = parse_multipart_related(&ct, &body).unwrap();
        let mp3 = extract_speak_audio(&parts).unwrap();
        assert_eq!(mp3, vec![0xFF, 0xFB, 0x10, 0x00, 1, 2, 3, 4]);
    }

    #[test]
    fn extract_errors_when_no_speak() {
        let parts = vec![Part { headers: vec![], body: b"x".to_vec() }];
        assert!(extract_speak_audio(&parts).is_err());
    }
}
