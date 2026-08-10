//! ACP Relay — binary entry point.
//!
//! Configured entirely from the environment:
//! - `ACP_SHARED_SECRET` (required) — secret every ACP token is signed with
//! - `ACP_DB_PATH` — `SQLite` file, default `/tmp/acp-messages.db`
//! - `ACP_PORT` — listen port, default `8443`

use std::path::PathBuf;
use std::process::ExitCode;

use acp_relay::app::{build_router, AppState};
use acp_relay::store::Store;
use tokio::sync::broadcast;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// The relay's own agent ID, used as the audience of relay-addressed tokens.
const RELAY_AGENT_ID: &str = "acp-relay";

/// Capacity of the live-feed fan-out channel.
const BROADCAST_CAPACITY: usize = 100;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "acp_relay=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    match serve().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn serve() -> anyhow::Result<()> {
    let shared_secret = std::env::var("ACP_SHARED_SECRET")
        .map_err(|_| anyhow::anyhow!("ACP_SHARED_SECRET must be set"))?;
    let db_path =
        std::env::var("ACP_DB_PATH").unwrap_or_else(|_| "/tmp/acp-messages.db".to_string());
    let port: u16 = std::env::var("ACP_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8443);

    let store = Store::new(&PathBuf::from(&db_path))
        .map_err(|e| anyhow::anyhow!("Failed to open DB at {db_path}: {e}"))?;

    let (broadcast_tx, _) = broadcast::channel::<String>(BROADCAST_CAPACITY);

    let app = build_router(AppState {
        store,
        shared_secret,
        this_agent_id: RELAY_AGENT_ID.to_string(),
        broadcast_tx,
    })
    .layer(
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any),
    )
    .layer(TraceLayer::new_for_http());

    let addr = format!("0.0.0.0:{port}");
    tracing::info!("ACP relay starting on {addr}");

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to bind {addr}: {e}"))?;
    axum::serve(listener, app).await?;
    Ok(())
}
