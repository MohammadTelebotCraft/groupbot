
use axum::http::header;
use axum::response::{Html, IntoResponse};

const INDEX_HTML: &str = include_str!("assets/index.html");
const APP_JS: &str = include_str!("assets/app.js");
const APP_CSS: &str = include_str!("assets/app.css");

pub async fn index() -> impl IntoResponse {
    Html(INDEX_HTML)
}

pub async fn app_js() -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        format!(
            "window.MODERATION_ICONS = {};\n{}",
            crate::handlers::premium::web_fallbacks(),
            APP_JS
        ),
    )
}

pub async fn app_css() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], APP_CSS)
}
