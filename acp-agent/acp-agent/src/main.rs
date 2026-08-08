//! ACP Agent — CLI entry point
//!
//! Usage:
//!   acp-agent run [--config <path>] [--port <port>]
//!   acp-agent send <target> <payload-json>
//!   acp-agent listen [--poll-interval <secs>]
//!   acp-agent doctor

use clap::{Parser, Subcommand};
use std::process::ExitCode;

mod agent;
mod server;
mod signaling;

#[derive(Parser)]
#[command(name = "acp-agent")]
#[command(about = "ACP Agent — Agent Communication Protocol", version = "0.1.0")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(short, long, global = true)]
    config: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the ACP agent HTTP server
    Run {
        /// Port to listen on
        #[arg(short, long, default_value = "8443")]
        port: u16,

        /// Use signaling (connect to relay)
        #[arg(long)]
        use_signaling: bool,
    },

    /// Send a message to a peer
    Send {
        /// Target agent ID
        target: String,

        /// JSON payload
        payload: String,
    },

    /// Long-poll for incoming messages
    Listen {
        /// Poll interval in seconds
        #[arg(long, default_value = "5")]
        poll_interval: u64,
    },

    /// Diagnose connectivity and configuration
    Doctor {
        /// Target peer to check connectivity
        target: Option<String>,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run { port, use_signaling } => {
            run_agent(cli.config.as_deref(), port, use_signaling).await
        }
        Commands::Send { target, payload } => {
            send_message(cli.config.as_deref(), &target, &payload).await
        }
        Commands::Listen { poll_interval } => {
            listen_messages(cli.config.as_deref(), poll_interval).await
        }
        Commands::Doctor { target } => {
            doctor(cli.config.as_deref(), target.as_deref()).await
        }
    }
}

async fn run_agent(config_path: Option<&str>, port: u16, use_signaling: bool) -> ExitCode {
    tracing::info!("Starting ACP Agent...");

    let agent = match agent::ACPAgent::from_config_file(config_path).await {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("Failed to load config: {}", e);
            return ExitCode::from(1);
        }
    };

    // Register signaling handlers if enabled
    if use_signaling {
        match signaling::SignalingConfig::from_env() {
            Ok(cfg) => {
                tracing::info!("Starting signaling to relay: {}", cfg.relay_url);
                // Spawn background tasks
                let pending_store = std::sync::Arc::new(tokio::sync::RwLock::new(Vec::new()));
                tokio::spawn(signaling::register_loop(cfg.clone()));
                tokio::spawn(signaling::poll_loop(cfg, pending_store));
            }
            Err(e) => {
                tracing::warn!("Signaling disabled: {}", e);
            }
        }
    }

    // Set up default handlers
    agent.on_delegate(|msg| {
        tracing::info!("[handler] Received: {}", msg.envelope.msg_id);
        None
    });

    if let Err(e) = server::start_server(agent, port).await {
        tracing::error!("Server error: {}", e);
        return ExitCode::from(1);
    }

    ExitCode::SUCCESS
}

async fn send_message(config_path: Option<&str>, target: &str, payload_json: &str) -> ExitCode {
    let payload: serde_json::Value = match serde_json::from_str(payload_json) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("Invalid JSON payload: {}", e);
            return ExitCode::from(1);
        }
    };

    let agent = match agent::ACPAgent::from_config_file(config_path).await {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("Failed to load config: {}", e);
            return ExitCode::from(1);
        }
    };

    match agent.send_message(target, payload, None).await {
        Ok(msg) => {
            tracing::info!("Message sent: {} -> {}", msg.envelope.msg_id, target);
            ExitCode::SUCCESS
        }
        Err(e) => {
            tracing::error!("Send failed: {}", e);
            ExitCode::from(1)
        }
    }
}

async fn listen_messages(_config_path: Option<&str>, poll_interval: u64) -> ExitCode {
    tracing::info!("Listening for messages (poll interval: {}s)...", poll_interval);
    // TODO: implement polling loop
    tracing::info!("Listen mode not yet implemented");
    ExitCode::from(1)
}

async fn doctor(config_path: Option<&str>, target: Option<&str>) -> ExitCode {
    tracing::info!("Running ACP doctor check...");

    // Load config
    let agent = match agent::ACPAgent::from_config_file(config_path).await {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("Config error: {}", e);
            return ExitCode::from(1);
        }
    };

    // Check this agent
    tracing::info!("Agent ID: {}", agent.this.agent_id);
    tracing::info!("Machine ID: {:?}", agent.this.machine_id);
    tracing::info!("HTTP Endpoint: {:?}", agent.this.http_endpoint);

    // Check peers
    if let Some(target_id) = target {
        if let Some(peer) = agent.config.get_peer(target_id) {
            tracing::info!("Target peer {}: {}@{}", target_id, peer.agent_id, peer.machine_id);
            tracing::info!("  HTTP: {}", peer.http_endpoint);
        } else {
            tracing::warn!("Peer {} not found in config", target_id);
        }
    }

    // Check env
    if std::env::var("ACP_SHARED_SECRET").is_ok() {
        tracing::info!("ACP_SHARED_SECRET: set");
    } else {
        tracing::warn!("ACP_SHARED_SECRET: not set");
    }

    tracing::info!("Doctor check complete");
    ExitCode::SUCCESS
}
