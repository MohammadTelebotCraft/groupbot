
mod api;
mod auth;
mod cases;
mod dashboard;
mod filters;
mod frontend;
mod lists;
mod locks;
mod misc;
mod overview;
mod response_policy;
mod settings;
mod voice;

use std::sync::Arc;

use axum::Router;
use axum::extract::FromRef;
use axum::routing::get;

use crate::handlers::Ctx;

const DEFAULT_CONCURRENCY: usize = 64;
const MAX_CONCURRENCY: usize = 1024;

pub struct Config {
    link: String,
    bot_username: String,
    bind: String,
    concurrency: usize,
    secret: [u8; 32],
}

impl Config {
    pub fn link(&self) -> &str {
        &self.link
    }

    pub fn validate_bot_username(&self, actual: Option<&str>) -> Result<(), String> {
        let actual = actual.ok_or_else(|| {
            "MINIAPP_LINK is configured, but the authenticated bot has no username".to_owned()
        })?;
        if self.bot_username.eq_ignore_ascii_case(actual) {
            Ok(())
        } else {
            Err(format!(
                "MINIAPP_LINK names bot {:?}, but Telegram authenticated {:?}",
                self.bot_username, actual
            ))
        }
    }
}

pub struct Bound {
    listener: tokio::net::TcpListener,
    concurrency: usize,
    secret: [u8; 32],
}

#[derive(Clone)]
pub(super) struct MiniAppState {
    ctx: Arc<Ctx>,
    secret: [u8; 32],
}

impl FromRef<MiniAppState> for Arc<Ctx> {
    fn from_ref(state: &MiniAppState) -> Self {
        Arc::clone(&state.ctx)
    }
}

fn parse_concurrency(value: Option<&str>) -> Result<usize, String> {
    let concurrency = match value {
        Some(value) => value.parse::<usize>().map_err(|_| {
            format!(
                "MINIAPP_CONCURRENCY must be an integer in 1..={MAX_CONCURRENCY}, got {value:?}"
            )
        })?,
        None => DEFAULT_CONCURRENCY,
    };
    if !(1..=MAX_CONCURRENCY).contains(&concurrency) {
        return Err(format!(
            "MINIAPP_CONCURRENCY must be in 1..={MAX_CONCURRENCY}, got {concurrency}"
        ));
    }
    Ok(concurrency)
}

fn parse_link(value: &str) -> Result<(String, String), String> {
    if value.is_empty() || value != value.trim() || value.chars().any(char::is_whitespace) {
        return Err(
            "MINIAPP_LINK must not contain leading, trailing, or embedded whitespace".to_owned(),
        );
    }
    let value = value.trim_end_matches('/');
    let rest = value
        .strip_prefix("https://")
        .ok_or_else(|| "MINIAPP_LINK must use https://".to_owned())?;
    if value.contains('?') || value.contains('#') {
        return Err("MINIAPP_LINK must not include a query or fragment".to_owned());
    }
    let mut parts = rest.split('/');
    let (Some("t.me"), Some(bot), Some(short_name), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(
            "MINIAPP_LINK must have the form https://t.me/<bot_username>/<short_name>".to_owned(),
        );
    };
    let bot_valid = (5..=32).contains(&bot.len())
        && bot.to_ascii_lowercase().ends_with("bot")
        && bot
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_');
    let short_valid = (1..=64).contains(&short_name.len())
        && short_name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_');
    if !bot_valid || !short_valid {
        return Err(
            "MINIAPP_LINK contains an invalid bot username or Mini App short name".to_owned(),
        );
    }
    Ok((value.to_owned(), bot.to_owned()))
}

pub fn config(bot_token: &str) -> Result<Option<Config>, String> {
    let link = match std::env::var("MINIAPP_LINK") {
        Ok(link) => parse_link(&link)?,
        Err(std::env::VarError::NotPresent) => return Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err("MINIAPP_LINK is not valid Unicode".to_owned());
        }
    };
    let concurrency = match std::env::var("MINIAPP_CONCURRENCY") {
        Ok(value) => parse_concurrency(Some(&value))?,
        Err(std::env::VarError::NotPresent) => parse_concurrency(None)?,
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err("MINIAPP_CONCURRENCY is not valid Unicode".to_owned());
        }
    };
    let bind = match std::env::var("MINIAPP_BIND") {
        Ok(bind) if !bind.trim().is_empty() => bind,
        Ok(_) => return Err("MINIAPP_BIND must not be empty".to_owned()),
        Err(std::env::VarError::NotPresent) => "127.0.0.1:8787".to_owned(),
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err("MINIAPP_BIND is not valid Unicode".to_owned());
        }
    };
    let secret = auth::derive_secret_key(bot_token)
        .map_err(|_| "TG_BOT_TOKEN cannot be used as a Mini App authentication key".to_owned())?;
    Ok(Some(Config {
        link: link.0,
        bot_username: link.1,
        bind,
        concurrency,
        secret,
    }))
}

fn router(ctx: Arc<Ctx>, concurrency: usize, secret: [u8; 32]) -> Router {
    Router::new()
        .route("/", get(frontend::index))
        .route("/app.js", get(frontend::app_js))
        .route("/app.css", get(frontend::app_css))
        .nest("/api", api::router())
        .layer(tower::limit::ConcurrencyLimitLayer::new(concurrency))
        .with_state(MiniAppState { ctx, secret })
}

pub async fn bind(config: Config) -> std::io::Result<Bound> {
    let listener = tokio::net::TcpListener::bind(&config.bind).await?;
    println!("miniapp: listening on {}", config.bind);
    Ok(Bound {
        listener,
        concurrency: config.concurrency,
        secret: config.secret,
    })
}

pub async fn serve(
    ctx: Arc<Ctx>,
    bound: Bound,
    mut stopping: tokio::sync::watch::Receiver<bool>,
) -> std::io::Result<()> {
    let shutdown = async move {
        if *stopping.borrow() {
            return;
        }
        while stopping.changed().await.is_ok() {
            if *stopping.borrow() {
                return;
            }
        }
    };
    axum::serve(bound.listener, router(ctx, bound.concurrency, bound.secret))
        .with_graceful_shutdown(shutdown)
        .await
}

#[cfg(test)]
mod tests {
    use super::{Config, config, parse_concurrency, parse_link};

    #[test]
    fn concurrency_is_strict_instead_of_clamped_or_defaulted() {
        assert_eq!(parse_concurrency(None).unwrap(), 64);
        assert_eq!(parse_concurrency(Some("32")).unwrap(), 32);
        assert!(parse_concurrency(Some("garbage")).is_err());
        assert!(parse_concurrency(Some("0")).is_err());
        assert!(parse_concurrency(Some("1025")).is_err());
    }

    #[test]
    fn enabled_miniapp_rejects_an_empty_bot_token() {
        let _: fn(&str) -> Result<Option<super::Config>, String> = config;
        assert!(super::auth::derive_secret_key("").is_err());
    }

    #[test]
    fn miniapp_link_is_safe_to_extend_with_a_start_parameter() {
        assert_eq!(
            parse_link("https://t.me/example_bot/app_1/").unwrap(),
            (
                "https://t.me/example_bot/app_1".to_owned(),
                "example_bot".to_owned()
            )
        );
        assert!(parse_link("t.me/example_bot/app").is_err());
        assert!(parse_link("https://example.com/app?x=1").is_err());
        assert!(parse_link("https://example.com/app #fragment").is_err());
        assert!(parse_link("https:///app").is_err());
        assert!(parse_link("http://t.me/example_bot/app").is_err());
        assert!(parse_link("https://t.me/example/app").is_err());
        assert!(parse_link("https://t.me/example_bot/app/more").is_err());
        assert!(parse_link(" https://t.me/example_bot/app").is_err());
        assert!(parse_link("").is_err());
    }

    #[test]
    fn miniapp_link_must_name_the_authenticated_bot() {
        let config = Config {
            link: "https://t.me/example_bot/app".to_owned(),
            bot_username: "example_bot".to_owned(),
            bind: "127.0.0.1:0".to_owned(),
            concurrency: 1,
            secret: [0; 32],
        };
        assert!(config.validate_bot_username(Some("EXAMPLE_BOT")).is_ok());
        assert!(config.validate_bot_username(Some("another_bot")).is_err());
        assert!(config.validate_bot_username(None).is_err());
    }
}
