mod api;
mod auth;
mod dashboard;
mod filters;
mod frontend;
mod lists;
mod locks;
mod misc;
mod overview;
mod settings;
mod throttle;
mod voice;

use std::sync::Arc;

use axum::Router;
use axum::routing::get;

use crate::handlers::Ctx;

fn configured() -> bool {
    std::env::var("MINIAPP_LINK")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

fn concurrency() -> usize {
    std::env::var("MINIAPP_CONCURRENCY")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(64)
        .clamp(1, 1024)
}

fn router(ctx: Arc<Ctx>) -> Router {
    Router::new()
        .route("/", get(frontend::index))
        .route("/app.js", get(frontend::app_js))
        .route("/app.css", get(frontend::app_css))
        .nest("/api", api::router())
        .layer(tower::limit::ConcurrencyLimitLayer::new(concurrency()))
        .with_state(ctx)
}

pub async fn spawn(ctx: Arc<Ctx>) {
    if !configured() {
        return;
    }
    let bind = std::env::var("MINIAPP_BIND").unwrap_or_else(|_| "127.0.0.1:8787".to_owned());
    let listener = match tokio::net::TcpListener::bind(&bind).await {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("miniapp: could not bind {bind}: {e}");
            return;
        }
    };
    println!("miniapp: listening on {bind}");
    if let Err(e) = axum::serve(listener, router(ctx)).await {
        eprintln!("miniapp: server exited: {e}");
    }
}
