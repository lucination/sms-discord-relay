pub mod logging;
pub mod server;
use serde::Serialize;

pub struct SigningKey(Vec<u8>);
#[derive(Debug, PartialEq)]
pub struct AuthError;
impl SigningKey {
    pub fn new(value: &str) -> Result<Self, AuthError> {
        if value.trim().is_empty() {
            return Err(AuthError);
        }
        Ok(Self(value.as_bytes().to_vec()))
    }
    pub fn verify(
        &self,
        body: &[u8],
        timestamp: &str,
        signature: &str,
        now: u64,
    ) -> Result<(), AuthError> {
        if timestamp.is_empty() || !timestamp.bytes().all(|b| b.is_ascii_digit()) {
            return Err(AuthError);
        }
        let seconds: u64 = timestamp.parse().map_err(|_| AuthError)?;
        if seconds.abs_diff(now) > 300 {
            return Err(AuthError);
        }
        use hmac::{Hmac, Mac};
        let mut mac = Hmac::<sha2::Sha256>::new_from_slice(&self.0).map_err(|_| AuthError)?;
        mac.update(body);
        mac.update(timestamp.as_bytes());
        let mut signature_bytes = [0u8; 32];
        hex::decode_to_slice(signature, &mut signature_bytes).map_err(|_| AuthError)?;
        mac.verify_slice(&signature_bytes).map_err(|_| AuthError)
    }
}

#[derive(Debug, Serialize)]
pub struct DiscordPayload {
    content: String,
    allowed_mentions: AllowedMentions,
}
#[derive(Debug, Serialize)]
struct AllowedMentions {
    parse: [String; 0],
}
#[derive(Debug)]
pub enum Delivery {
    Single(DiscordPayload),
    Batch(Vec<DiscordPayload>),
    Ignored,
}
#[derive(Debug, PartialEq)]
pub enum InputError {
    Invalid,
    TooMany,
}
#[derive(serde::Deserialize)]
struct Sms {
    sender: Option<String>,
    #[serde(rename = "phoneNumber")]
    phone_number: Option<String>,
    #[serde(default)]
    message: String,
    #[serde(rename = "recipient")]
    _recipient: Option<String>,
    #[serde(rename = "simNumber")]
    _sim_number: Option<u32>,
    #[serde(default, rename = "receivedAt")]
    received_at: Option<String>,
}
fn truncate(text: &str, max: usize) -> String {
    if text.encode_utf16().count() <= max {
        return text.to_owned();
    }
    let mut result = String::new();
    let mut units = 0;
    for c in text.chars() {
        if units + c.len_utf16() > max - 1 {
            break;
        }
        result.push(c);
        units += c.len_utf16();
    }
    result.push('…');
    result
}
fn render(sms: Sms) -> Result<DiscordPayload, InputError> {
    if sms.sender.is_none() && sms.phone_number.is_none() {
        return Err(InputError::Invalid);
    }
    let sender = sms
        .sender
        .as_deref()
        .or(sms.phone_number.as_deref())
        .filter(|s| !s.is_empty())
        .unwrap_or("unknown sender");
    let received_at = sms.received_at.as_deref().unwrap_or("");
    let mut content = format!("📱 **New SMS from {sender}**\n{}", sms.message);
    if !received_at.is_empty() {
        content.push_str(&format!("\n-# {received_at}"));
    }
    Ok(DiscordPayload {
        content: truncate(&content, 2000),
        allowed_mentions: AllowedMentions { parse: [] },
    })
}
pub fn transform(body: &[u8]) -> Result<Delivery, InputError> {
    #[derive(serde::Deserialize)]
    #[serde(tag = "event", content = "payload")]
    enum Envelope {
        #[serde(rename = "sms:received")]
        Single(Sms),
        #[serde(rename = "sms:batch:received")]
        Batch { messages: Vec<Sms> },
    }
    #[derive(serde::Deserialize)]
    struct EventKind {
        event: String,
    }
    let kind: EventKind = serde_json::from_slice(body).map_err(|_| InputError::Invalid)?;
    if !matches!(kind.event.as_str(), "sms:received" | "sms:batch:received") {
        return Ok(Delivery::Ignored);
    }
    let event: Envelope = serde_json::from_slice(body).map_err(|_| InputError::Invalid)?;
    Ok(match event {
        Envelope::Single(sms) => Delivery::Single(render(sms)?),
        Envelope::Batch { messages } => {
            if messages.len() > 100 {
                return Err(InputError::TooMany);
            }
            Delivery::Batch(messages.into_iter().map(render).collect::<Result<_, _>>()?)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_current_delivery_with_both_sender_fields() {
        for (sender, expected) in [
            (serde_json::json!("current"), "current"),
            (serde_json::Value::Null, "legacy"),
        ] {
            let body = serde_json::to_vec(&serde_json::json!({
                "event": "sms:received", "deviceId":"device", "id":"delivery", "webhookId":"hook", "scheme":"https",
                "payload":{"messageId":"message", "message":"text", "phoneNumber":"legacy", "sender":sender,
                    "recipient":null, "simNumber":1, "receivedAt":"now"}
            })).unwrap();
            let Delivery::Single(payload) = transform(&body).unwrap() else {
                panic!()
            };
            assert_eq!(
                payload.content,
                format!("📱 **New SMS from {expected}**\ntext\n-# now")
            );
        }
    }
    #[test]
    fn rejects_missing_sender_fields_in_single_and_batch() {
        for payload in [
            serde_json::json!({"message":"text"}),
            serde_json::json!({"sender":null,"phoneNumber":null}),
        ] {
            for body in [
                serde_json::json!({"event":"sms:received","payload":payload.clone()}),
                serde_json::json!({"event":"sms:batch:received","payload":{"messages":[{"sender":"valid"},payload.clone()]}}),
            ] {
                assert_eq!(
                    transform(&serde_json::to_vec(&body).unwrap()).unwrap_err(),
                    InputError::Invalid
                );
            }
        }
    }
    #[test]
    fn python_format_defaults_and_optional_footer() {
        for payload in [
            serde_json::json!({"sender":""}),
            serde_json::json!({"sender":"","receivedAt":null}),
            serde_json::json!({"sender":"","receivedAt":"","message":""}),
        ] {
            let body =
                serde_json::to_vec(&serde_json::json!({"event":"sms:received","payload":payload}))
                    .unwrap();
            let Delivery::Single(p) = transform(&body).unwrap() else {
                panic!()
            };
            assert_eq!(p.content, "📱 **New SMS from unknown sender**\n");
        }
        assert_eq!(
            transform(br#"{"event":"sms:received","payload":{"message":null}}"#).unwrap_err(),
            InputError::Invalid
        );
    }
    #[test]
    fn rejects_empty_key_and_expired_timestamp() {
        use hmac::{Hmac, Mac};
        assert!(SigningKey::new("").is_err());
        assert!(SigningKey::new("   ").is_err());
        let key = SigningKey::new("key").unwrap();
        for (ts, valid) in [
            ("700", true),
            ("1300", true),
            ("699", false),
            ("1301", false),
            ("-1", false),
            ("1.0", false),
            ("", false),
            ("+1000", false),
        ] {
            let mut mac = Hmac::<sha2::Sha256>::new_from_slice(b"key").unwrap();
            mac.update(ts.as_bytes());
            let sig = hex::encode(mac.finalize().into_bytes());
            assert_eq!(key.verify(b"", ts, &sig, 1000).is_ok(), valid, "{ts}");
        }
    }

    #[test]
    fn authenticates_exact_raw_bytes_then_timestamp() {
        use hmac::{Hmac, Mac};
        let key = SigningKey::new("text-key").unwrap();
        let body = b"{  raw bytes }";
        let timestamp = "0001000";
        let mut hmac = Hmac::<sha2::Sha256>::new_from_slice(b"text-key").unwrap();
        hmac.update(body);
        hmac.update(timestamp.as_bytes());
        let signature = hex::encode(hmac.finalize().into_bytes());
        assert_eq!(key.verify(body, timestamp, &signature, 1000), Ok(()));
        assert_eq!(
            key.verify(b"changed", timestamp, &signature, 1000),
            Err(AuthError)
        );
        assert_eq!(key.verify(body, "1000", &signature, 1000), Err(AuthError));
        assert_eq!(
            key.verify(body, timestamp, &signature.to_uppercase(), 1000),
            Ok(())
        );
        assert_eq!(key.verify(body, timestamp, "invalid", 1000), Err(AuthError));
    }

    #[test]
    fn truncates_whole_content_at_utf16_scalar_boundary() {
        let body=serde_json::to_vec(&serde_json::json!({"event":"sms:received","payload":{
            "sender":"s".repeat(4000),"recipient":"r".repeat(4000),"receivedAt":"t".repeat(4000),"message":"😀".repeat(3000)}})).unwrap();
        let Delivery::Single(p) = transform(&body).unwrap() else {
            panic!()
        };
        assert!(p.content.encode_utf16().count() <= 2000);
        assert!(p.content.starts_with("📱 **New SMS from "));
        assert!(p.content.ends_with('…'));
    }

    #[test]
    fn limits_batch_to_100() {
        let sms = serde_json::json!({"sender":"x", "message":"x", "receivedAt":"x"});
        let body = |n| {
            serde_json::to_vec(&serde_json::json!({"event":"sms:batch:received","payload":{"messages":vec![sms.clone();n]}})).unwrap()
        };
        assert!(matches!(transform(&body(100)), Ok(Delivery::Batch(v)) if v.len()==100));
        assert_eq!(transform(&body(101)).unwrap_err(), InputError::TooMany);
    }

    #[test]
    fn ignores_other_events_without_parsing_sms() {
        assert!(matches!(
            transform(br#"{"event":"sms:sent","payload":{"unrelated":true}}"#),
            Ok(Delivery::Ignored)
        ));
        assert!(matches!(
            transform(br#"{"event":"sms:sent"}"#),
            Ok(Delivery::Ignored)
        ));
        assert_eq!(transform(b"not JSON").unwrap_err(), InputError::Invalid);
        assert_eq!(
            transform(br#"{"event":"sms:received","payload":{"message":42}}"#).unwrap_err(),
            InputError::Invalid
        );
    }

    #[test]
    fn legacy_batch_uses_sender_without_recipient_label() {
        let body = br#"{"event":"sms:batch:received","payload":{"messages":[{"phoneNumber":"old","message":"text","recipient":"me","simNumber":null,"receivedAt":"now"}]}}"#;
        let Delivery::Batch(payloads) = transform(body).unwrap() else {
            panic!("batch expected")
        };
        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0].content, "📱 **New SMS from old**\ntext\n-# now");
    }
    #[test]
    fn current_sms_becomes_safe_discord_payload() {
        let body = br#"{"event":"sms:received","payload":{"messageId":"1","sender":"+123","message":"hello @everyone","recipient":null,"simNumber":1,"receivedAt":"2024-06-22T15:46:11.000+07:00"}}"#;
        let Delivery::Single(payload) = transform(body).unwrap() else {
            panic!("single expected")
        };
        assert_eq!(
            payload.content,
            "📱 **New SMS from +123**\nhello @everyone\n-# 2024-06-22T15:46:11.000+07:00"
        );
        assert_eq!(
            serde_json::to_value(payload).unwrap()["allowed_mentions"]["parse"],
            serde_json::json!([])
        );
    }
}
