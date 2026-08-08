use std::path::PathBuf;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "acp_relay=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let shared_secret = std::env::var("ACP_SHARED_SECRET")
        .expect("ACP_SHARED_SECRET must be set");
    let db_path = std::env::var("ACP_DB_PATH")
        .unwrap_or_else(|_| "/tmp/acp-messages.db".to_string());
    let port: u16 = std::env::var("ACP_PORT")
        .unwrap_or_else(|_| "8443".to_string())
        .parse()
        .unwrap_or(8443);

    let store = acp_relay::store::Store::new(&PathBuf::from(&db_path))
        .expect("Failed to open DB");

    let state = acp_relay::app::AppState {
        store,
        shared_secret,
        this_agent_id: "acp-relay".to_string(),
    };

    let app = acp_relay::app::build_router(state)
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(TraceLayer::new_for_http());

    let addr = format!("0.0.0.0:{}", port);
    tracing::info!("ACP relay starting on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.expect("Server error");
}
