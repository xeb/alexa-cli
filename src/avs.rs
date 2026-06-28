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
