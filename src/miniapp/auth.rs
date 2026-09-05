use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::handlers::{self, limits, Ctx};

const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

pub struct InitData {
    pub user_id: i64,
    pub start_param: Option<String>,
}

#[derive(Debug)]
enum AuthError {
    BadFormat,
    BadHash,
    Stale,
    NoUser,
}

fn secret_key_bytes() -> &'static [u8; 32] {
    static SECRET: OnceLock<[u8; 32]> = OnceLock::new();
    SECRET.get_or_init(|| {
        let token = std::env::var("TG_BOT_TOKEN").unwrap_or_default();
        let mut mac =
            Hmac::<Sha256>::new_from_slice(b"WebAppData").expect("HMAC accepts any key length");
        mac.update(token.as_bytes());
        let digest = mac.finalize().into_bytes();
        let mut out = [0u8; 32];
        out.copy_from_slice(&digest);
        out
    })
}

fn urldecode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut raw = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                raw.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => match u8::from_str_radix(&input[i + 1..i + 3], 16) {
                Ok(byte) => {
                    raw.push(byte);
                    i += 3;
                }
                Err(_) => {
                    raw.push(bytes[i]);
                    i += 1;
                }
            },
            b => {
                raw.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&raw).into_owned()
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

fn validate(raw: &str, max_age: Duration) -> Result<InitData, AuthError> {
    let mut hash = None;
    let mut pairs: Vec<(String, String)> = Vec::new();
    for piece in raw.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = piece.split_once('=').ok_or(AuthError::BadFormat)?;
        let (key, value) = (urldecode(key), urldecode(value));
        if key == "hash" {
            hash = Some(value);
        } else {
            pairs.push((key, value));
        }
    }
    let hash = hash.ok_or(AuthError::BadFormat)?;
    let expected = decode_hex(&hash).ok_or(AuthError::BadFormat)?;

    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    let data_check_string = pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("\n");

    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret_key_bytes()).expect("32-byte key always fits");
    mac.update(data_check_string.as_bytes());
    mac.verify_slice(&expected).map_err(|_| AuthError::BadHash)?;

    let auth_date: i64 = pairs
        .iter()
        .find(|(k, _)| k == "auth_date")
        .and_then(|(_, v)| v.parse().ok())
        .ok_or(AuthError::BadFormat)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if now - auth_date > max_age.as_secs() as i64 {
        return Err(AuthError::Stale);
    }

    let user_id = pairs
        .iter()
        .find(|(k, _)| k == "user")
        .and_then(|(_, v)| serde_json::from_str::<serde_json::Value>(v).ok())
        .and_then(|value| value.get("id").and_then(serde_json::Value::as_i64))
        .ok_or(AuthError::NoUser)?;

    let start_param = pairs
        .into_iter()
        .find(|(k, _)| k == "start_param")
        .map(|(_, v)| v)
        .filter(|v| !v.is_empty());

    Ok(InitData {
        user_id,
        start_param,
    })
}

#[derive(Clone, Copy)]
pub struct AdminGate {
    pub chat: i64,
    pub user: i64,
    pub is_owner: bool,
}

#[derive(Clone, Copy)]
pub enum GateError {
    Unauthenticated,
    NoChatSelected,
    ChatUnknown,
    NotAdmin,
    SetDenied,
}

impl GateError {
    fn state(self) -> &'static str {
        match self {
            Self::Unauthenticated => "auth_failed",
            Self::NoChatSelected => "no_chat_selected",
            Self::ChatUnknown => "chat_unknown",
            Self::NotAdmin => "not_admin",
            Self::SetDenied => "set_denied",
        }
    }

    fn status(self) -> StatusCode {
        match self {
            Self::NoChatSelected => StatusCode::OK,
            Self::Unauthenticated => StatusCode::UNAUTHORIZED,
            Self::ChatUnknown => StatusCode::NOT_FOUND,
            Self::NotAdmin | Self::SetDenied => StatusCode::FORBIDDEN,
        }
    }
}

impl IntoResponse for GateError {
    fn into_response(self) -> Response {
        (
            self.status(),
            axum::Json(serde_json::json!({ "state": self.state() })),
        )
            .into_response()
    }
}

impl FromRequestParts<Arc<Ctx>> for AdminGate {
    type Rejection = GateError;

    async fn from_request_parts(
        parts: &mut Parts,
        ctx: &Arc<Ctx>,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("tma "))
            .ok_or(GateError::Unauthenticated)?;
        let data = validate(header, MAX_AGE).map_err(|_| GateError::Unauthenticated)?;

        let picked = parts
            .headers
            .get("x-chat")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<i64>().ok());
        let Some(chat) = picked.or_else(|| {
            data.start_param
                .as_deref()
                .and_then(|s| s.parse::<i64>().ok())
        }) else {
            return Err(GateError::NoChatSelected);
        };

        let Some(chat_ref) = ctx.chat_ref(chat) else {
            return Err(GateError::ChatUnknown);
        };
        if !handlers::is_admin(ctx, chat_ref, chat, data.user_id).await {
            return Err(GateError::NotAdmin);
        }
        if !limits::permits(ctx, chat, data.user_id, limits::SET) {
            return Err(GateError::SetDenied);
        }

        Ok(AdminGate {
            chat,
            user: data.user_id,
            is_owner: handlers::owner(ctx, chat) == Some(data.user_id),
        })
    }
}

#[derive(Clone, Copy)]
pub struct UserGate {
    pub user: i64,
}

impl FromRequestParts<Arc<Ctx>> for UserGate {
    type Rejection = GateError;

    async fn from_request_parts(
        parts: &mut Parts,
        _ctx: &Arc<Ctx>,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("tma "))
            .ok_or(GateError::Unauthenticated)?;
        let data = validate(header, MAX_AGE).map_err(|_| GateError::Unauthenticated)?;
        Ok(UserGate { user: data.user_id })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_a_correctly_signed_init_data() {
        unsafe {
            std::env::set_var("TG_BOT_TOKEN", "123456:test-token");
        }

        let auth_date = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let user = r#"{"id":42,"first_name":"Test"}"#;
        let fields = [
            ("auth_date", auth_date.to_string()),
            ("start_param", "-1001234567890".to_owned()),
            ("user", user.to_owned()),
        ];
        let mut pairs: Vec<(&str, String)> =
            fields.iter().map(|(k, v)| (*k, v.clone())).collect();
        pairs.sort_by(|a, b| a.0.cmp(b.0));
        let data_check_string = pairs
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("\n");

        let mut secret_mac =
            Hmac::<Sha256>::new_from_slice(b"WebAppData").expect("any key length");
        secret_mac.update(b"123456:test-token");
        let secret = secret_mac.finalize().into_bytes();

        let mut mac = Hmac::<Sha256>::new_from_slice(&secret).expect("32-byte key");
        mac.update(data_check_string.as_bytes());
        let hash = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();

        let raw = format!(
            "auth_date={auth_date}&start_param=-1001234567890&user={}&hash={hash}",
            urlencode_for_test(user)
        );

        let data = validate(&raw, MAX_AGE).expect("a correctly signed payload must validate");
        assert_eq!(data.user_id, 42);
        assert_eq!(data.start_param.as_deref(), Some("-1001234567890"));
    }

    #[test]
    fn rejects_a_tampered_field() {
        unsafe {
            std::env::set_var("TG_BOT_TOKEN", "123456:test-token");
        }
        let raw = "auth_date=1&start_param=999&user=%7B%22id%22%3A1%7D&hash=deadbeef";
        assert!(validate(raw, MAX_AGE).is_err());
    }

    #[test]
    fn hex_decode_roundtrips() {
        assert_eq!(decode_hex("00ff"), Some(vec![0x00, 0xff]));
        assert_eq!(decode_hex("abc"), None);
        assert_eq!(decode_hex("zz"), None);
    }

    fn urlencode_for_test(s: &str) -> String {
        s.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect()
    }
}
