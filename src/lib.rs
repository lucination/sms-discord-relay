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
    #[serde(alias = "phoneNumber")]
    sender: String,
    message: String,
    recipient: Option<String>,
    #[serde(rename = "simNumber")]
    sim_number: Option<u32>,
    #[serde(rename = "receivedAt")]
    received_at: String,
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
fn render(sms: Sms) -> DiscordPayload {
    let sms = Sms {
        sender: truncate(&sms.sender, 160),
        recipient: sms.recipient.map(|s| truncate(&s, 160)),
        received_at: truncate(&sms.received_at, 160),
        ..sms
    };
    let mut content = format!("From: {}\n", sms.sender);
    if let Some(recipient) = sms.recipient {
        content.push_str(&format!("To: {recipient}\n"));
    }
    if let Some(sim) = sms.sim_number {
        content.push_str(&format!("SIM: {sim}\n"));
    }
    content.push_str(&format!("Received: {}\n\n{}", sms.received_at, sms.message));
    DiscordPayload {
        content: truncate(&content, 2000),
        allowed_mentions: AllowedMentions { parse: [] },
    }
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
        Envelope::Single(sms) => Delivery::Single(render(sms)),
        Envelope::Batch { messages } => {
            if messages.len() > 100 {
                return Err(InputError::TooMany);
            }
            Delivery::Batch(messages.into_iter().map(render).collect())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn truncates_unicode_including_labels_without_losing_message_to_long_sender() {
        let body=serde_json::to_vec(&serde_json::json!({"event":"sms:received","payload":{
            "sender":"s".repeat(4000),"recipient":"r".repeat(4000),"receivedAt":"t".repeat(4000),"message":"😀".repeat(3000)}})).unwrap();
        let Delivery::Single(p) = transform(&body).unwrap() else {
            panic!()
        };
        assert!(p.content.encode_utf16().count() <= 2000);
        assert!(p.content.contains("😀"));
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
            transform(br#"{"event":"sms:received","payload":{}}"#).unwrap_err(),
            InputError::Invalid
        );
    }

    #[test]
    fn legacy_batch_preserves_recipient() {
        let body = br#"{"event":"sms:batch:received","payload":{"messages":[{"phoneNumber":"old","message":"text","recipient":"me","simNumber":null,"receivedAt":"now"}]}}"#;
        let Delivery::Batch(payloads) = transform(body).unwrap() else {
            panic!("batch expected")
        };
        assert_eq!(payloads.len(), 1);
        assert!(payloads[0].content.contains("From: old\nTo: me"));
    }
    #[test]
    fn current_sms_becomes_safe_discord_payload() {
        let body = br#"{"event":"sms:received","payload":{"messageId":"1","sender":"+123","message":"hello @everyone","recipient":null,"simNumber":1,"receivedAt":"2024-06-22T15:46:11.000+07:00"}}"#;
        let Delivery::Single(payload) = transform(body).unwrap() else {
            panic!("single expected")
        };
        assert!(payload.content.contains("From: +123"));
        assert!(payload.content.contains("hello @everyone"));
        assert!(payload.content.contains("2024-06-22T15:46:11.000+07:00"));
        assert!(payload.content.contains("SIM: 1"));
        assert_eq!(
            serde_json::to_value(payload).unwrap()["allowed_mentions"]["parse"],
            serde_json::json!([])
        );
    }
}
