
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::handlers::{self, limits};

use super::MiniAppState;

const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_FUTURE_SKEW: Duration = Duration::from_secs(30);

pub struct InitData {
    pub user_id: i64,
    pub start_param: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AuthError {
    BadFormat,
    BadHash,
    Stale,
    NoUser,
    Misconfigured,
    Clock,
}

pub(super) fn derive_secret_key(token: &str) -> Result<[u8; 32], AuthError> {
    if token.trim().is_empty() {
        return Err(AuthError::Misconfigured);
    }
    let mut mac =
        Hmac::<Sha256>::new_from_slice(b"WebAppData").map_err(|_| AuthError::Misconfigured)?;
    mac.update(token.as_bytes());
    Ok(mac.finalize().into_bytes().into())
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

fn validate(raw: &str, max_age: Duration, secret: &[u8; 32]) -> Result<InitData, AuthError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| AuthError::Clock)?
        .as_secs();
    validate_at(raw, max_age, now, secret)
}

fn validate_at(
    raw: &str,
    max_age: Duration,
    now: u64,
    secret: &[u8; 32],
) -> Result<InitData, AuthError> {
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

    let mut mac = Hmac::<Sha256>::new_from_slice(secret).map_err(|_| AuthError::Misconfigured)?;
    mac.update(data_check_string.as_bytes());
    mac.verify_slice(&expected)
        .map_err(|_| AuthError::BadHash)?;

    let auth_date: u64 = pairs
        .iter()
        .find(|(k, _)| k == "auth_date")
        .and_then(|(_, v)| v.parse().ok())
        .ok_or(AuthError::BadFormat)?;
    let timestamp_is_acceptable = match auth_date.checked_sub(now) {
        Some(ahead) => ahead <= MAX_FUTURE_SKEW.as_secs(),
        None => now
            .checked_sub(auth_date)
            .is_some_and(|age| age <= max_age.as_secs()),
    };
    if !timestamp_is_acceptable {
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
    Unavailable,
    NoChatSelected,
    ChatUnknown,
    NotAdmin,
    SetDenied,
    CaseDenied,
}

impl GateError {
    fn state(self) -> &'static str {
        match self {
            Self::Unauthenticated => "auth_failed",
            Self::Unavailable => "auth_unavailable",
            Self::NoChatSelected => "no_chat_selected",
            Self::ChatUnknown => "chat_unknown",
            Self::NotAdmin => "not_admin",
            Self::SetDenied => "set_denied",
            Self::CaseDenied => "case_denied",
        }
    }

    fn status(self) -> StatusCode {
        match self {
            Self::NoChatSelected => StatusCode::OK,
            Self::Unauthenticated => StatusCode::UNAUTHORIZED,
            Self::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            Self::ChatUnknown => StatusCode::NOT_FOUND,
            Self::NotAdmin | Self::SetDenied | Self::CaseDenied => StatusCode::FORBIDDEN,
        }
    }
}

fn gate_auth_error(error: AuthError) -> GateError {
    match error {
        AuthError::Misconfigured | AuthError::Clock => GateError::Unavailable,
        AuthError::BadFormat | AuthError::BadHash | AuthError::Stale | AuthError::NoUser => {
            GateError::Unauthenticated
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

impl FromRequestParts<MiniAppState> for AdminGate {
    type Rejection = GateError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &MiniAppState,
    ) -> Result<Self, Self::Rejection> {
        let ctx = &state.ctx;
        let header = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("tma "))
            .ok_or(GateError::Unauthenticated)?;
        let data = validate(header, MAX_AGE, &state.secret).map_err(gate_auth_error)?;

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
pub struct CaseGate {
    pub chat: i64,
    pub user: i64,
}

#[derive(Clone, Copy)]
pub struct ViewerGate {
    pub chat: i64,
    pub user: i64,
    pub is_owner: bool,
}

impl From<AdminGate> for ViewerGate {
    fn from(gate: AdminGate) -> Self {
        Self {
            chat: gate.chat,
            user: gate.user,
            is_owner: gate.is_owner,
        }
    }
}

impl FromRequestParts<MiniAppState> for ViewerGate {
    type Rejection = GateError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &MiniAppState,
    ) -> Result<Self, Self::Rejection> {
        let ctx = &state.ctx;
        let header = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("tma "))
            .ok_or(GateError::Unauthenticated)?;
        let data = validate(header, MAX_AGE, &state.secret).map_err(gate_auth_error)?;
        let picked = parts
            .headers
            .get("x-chat")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<i64>().ok());
        let Some(chat) = picked.or_else(|| {
            data.start_param
                .as_deref()
                .and_then(|value| value.parse().ok())
        }) else {
            return Err(GateError::NoChatSelected);
        };
        let Some(chat_ref) = ctx.chat_ref(chat) else {
            return Err(GateError::ChatUnknown);
        };
        if !handlers::is_admin(ctx, chat_ref, chat, data.user_id).await {
            return Err(GateError::NotAdmin);
        }
        Ok(Self {
            chat,
            user: data.user_id,
            is_owner: handlers::owner(ctx, chat) == Some(data.user_id),
        })
    }
}

impl FromRequestParts<MiniAppState> for CaseGate {
    type Rejection = GateError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &MiniAppState,
    ) -> Result<Self, Self::Rejection> {
        let ctx = &state.ctx;
        let header = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("tma "))
            .ok_or(GateError::Unauthenticated)?;
        let data = validate(header, MAX_AGE, &state.secret).map_err(gate_auth_error)?;
        let picked = parts
            .headers
            .get("x-chat")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<i64>().ok());
        let Some(chat) = picked.or_else(|| {
            data.start_param
                .as_deref()
                .and_then(|value| value.parse::<i64>().ok())
        }) else {
            return Err(GateError::NoChatSelected);
        };
        let Some(chat_ref) = ctx.chat_ref(chat) else {
            return Err(GateError::ChatUnknown);
        };
        if !handlers::is_admin(ctx, chat_ref, chat, data.user_id).await {
            return Err(GateError::NotAdmin);
        }
        if !limits::permits(ctx, chat, data.user_id, limits::CASE) {
            return Err(GateError::CaseDenied);
        }
        Ok(CaseGate {
            chat,
            user: data.user_id,
        })
    }
}

#[derive(Clone, Copy)]
pub struct UserGate {
    pub user: i64,
}

impl FromRequestParts<MiniAppState> for UserGate {
    type Rejection = GateError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &MiniAppState,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("tma "))
            .ok_or(GateError::Unauthenticated)?;
        let data = validate(header, MAX_AGE, &state.secret).map_err(gate_auth_error)?;
        Ok(UserGate { user: data.user_id })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_a_correctly_signed_init_data() {
        let auth_date = 1_700_000_000;
        let user = r#"{"id":42,"first_name":"Test"}"#;
        let fields = [
            ("auth_date", auth_date.to_string()),
            ("start_param", "-1001234567890".to_owned()),
            ("user", user.to_owned()),
        ];
        let mut pairs: Vec<(&str, String)> = fields.iter().map(|(k, v)| (*k, v.clone())).collect();
        pairs.sort_by(|a, b| a.0.cmp(b.0));
        let data_check_string = pairs
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("\n");

        let mut secret_mac = Hmac::<Sha256>::new_from_slice(b"WebAppData").unwrap();
        secret_mac.update(b"123456:test-token");
        let secret: [u8; 32] = secret_mac.finalize().into_bytes().into();

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

        let data = validate_at(&raw, MAX_AGE, auth_date, &secret)
            .expect("a correctly signed payload must validate");
        assert_eq!(data.user_id, 42);
        assert_eq!(data.start_param.as_deref(), Some("-1001234567890"));
    }

    #[test]
    fn rejects_a_tampered_field() {
        let secret = derive_secret_key("123456:test-token").unwrap();
        let raw = "auth_date=1&start_param=999&user=%7B%22id%22%3A1%7D&hash=deadbeef";
        assert!(validate_at(raw, MAX_AGE, 1, &secret).is_err());
    }

    #[test]
    fn empty_token_cannot_become_an_authentication_secret() {
        assert_eq!(derive_secret_key(""), Err(AuthError::Misconfigured));
        assert_eq!(derive_secret_key("  \t"), Err(AuthError::Misconfigured));
    }

    #[test]
    fn timestamp_window_has_checked_past_and_future_boundaries() {
        let secret = derive_secret_key("123456:test-token").unwrap();
        let now = 1_700_000_000;
        let old_edge = signed_data(now - MAX_AGE.as_secs(), 42, &secret);
        let too_old = signed_data(now - MAX_AGE.as_secs() - 1, 42, &secret);
        let future_edge = signed_data(now + MAX_FUTURE_SKEW.as_secs(), 42, &secret);
        let too_far_future = signed_data(now + MAX_FUTURE_SKEW.as_secs() + 1, 42, &secret);

        assert!(validate_at(&old_edge, MAX_AGE, now, &secret).is_ok());
        assert!(matches!(
            validate_at(&too_old, MAX_AGE, now, &secret),
            Err(AuthError::Stale)
        ));
        assert!(validate_at(&future_edge, MAX_AGE, now, &secret).is_ok());
        assert!(matches!(
            validate_at(&too_far_future, MAX_AGE, now, &secret),
            Err(AuthError::Stale)
        ));

        let extreme_future = signed_data(u64::MAX, 42, &secret);
        assert!(matches!(
            validate_at(&extreme_future, MAX_AGE, 0, &secret),
            Err(AuthError::Stale)
        ));
        let extreme_old = signed_data(0, 42, &secret);
        assert!(matches!(
            validate_at(&extreme_old, MAX_AGE, u64::MAX, &secret),
            Err(AuthError::Stale)
        ));
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

    fn signed_data(auth_date: u64, user_id: i64, secret: &[u8; 32]) -> String {
        let user = format!(r#"{{"id":{user_id}}}"#);
        let data_check_string = format!("auth_date={auth_date}\nuser={user}");
        let mut mac = Hmac::<Sha256>::new_from_slice(secret).unwrap();
        mac.update(data_check_string.as_bytes());
        let hash = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        format!(
            "auth_date={auth_date}&user={}&hash={hash}",
            urlencode_for_test(&user)
        )
    }
}
