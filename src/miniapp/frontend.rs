use axum::extract::State;
use axum::http::header;
use axum::response::{Html, IntoResponse};

use super::MiniAppState;

const INDEX_HTML: &str = include_str!("assets/index.html");
const APP_JS: &str = include_str!("assets/app.js");
const APP_CSS: &str = include_str!("assets/app.css");

pub async fn index() -> impl IntoResponse {
    Html(INDEX_HTML)
}

pub async fn app_js(State(state): State<MiniAppState>) -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        format!(
            "window.MODERATION_ICONS = {};\nwindow.GM_SITE = {};\n{}",
            crate::handlers::premium::web_fallbacks(),
            site(&state),
            APP_JS
        ),
    )
}

fn site(state: &MiniAppState) -> String {
    let links: serde_json::Map<String, serde_json::Value> = state
        .ctx
        .public_links()
        .map(|(name, url)| (name.to_owned(), serde_json::Value::from(url)))
        .collect();
    serde_json::json!({
        "bot": state.ctx.bot_username(),
        "app": state.ctx.miniapp_link(),
        "links": links,
    })
    .to_string()
}

pub async fn app_css() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], APP_CSS)
}
