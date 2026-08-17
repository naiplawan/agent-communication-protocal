//! ACP Agent — CLI entry point
//!
//! ```text
//! acp-agent run     [--config <path>] [--port <port>] [--use-signaling]
//! acp-agent send    <target> <payload-json>
//! acp-agent listen  [--poll-interval <secs>]
//! acp-agent doctor  [<target>]
//! ```

use std::process::ExitCode;
use std::sync::Arc;

use acp_agent::agent::ACPAgent;
use acp_agent::{server, signaling};
use acp_core::config::Peer;
use acp_core::protocol::{Envelope, Message};
use acp_core::transport::MessageAck;
use clap::{Parser, Subcommand};
use tokio::sync::RwLock;

/// The peer entry a relay-backed agent polls for messages.
const RELAY_AGENT_ID: &str = "acp-relay";

#[derive(Parser)]
#[command(name = "acp-agent")]
#[command(about = "ACP Agent — Agent Communication Protocol", version = "0.1.0")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Path to acp-peers.yaml; falls back to `ACP_PEERS_PATH` and the defaults
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

        /// Register with the relay and poll it for messages
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

    /// Long-poll the relay for incoming messages
    Listen {
        /// Poll interval in seconds
        #[arg(long, default_value = "5")]
        poll_interval: u64,
    },

    /// Diagnose connectivity and configuration
    Doctor {
        /// Peer to check connectivity against
        target: Option<String>,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let config_path = cli.config.as_deref();

    let outcome = match cli.command {
        Commands::Run {
            port,
            use_signaling,
        } => run_agent(config_path, port, use_signaling).await,
        Commands::Send { target, payload } => send_message(config_path, &target, &payload).await,
        Commands::Listen { poll_interval } => listen_messages(config_path, poll_interval).await,
        Commands::Doctor { target } => doctor(config_path, target.as_deref()),
    };

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e:#}");
            ExitCode::FAILURE
        }
    }
}

fn load_agent(config_path: Option<&str>) -> anyhow::Result<ACPAgent> {
    ACPAgent::from_config_file(config_path)
        .map_err(|e| anyhow::anyhow!("Failed to load config: {e}"))
}

async fn run_agent(
    config_path: Option<&str>,
    port: u16,
    use_signaling: bool,
) -> anyhow::Result<()> {
    tracing::info!("Starting ACP Agent...");
    let agent = load_agent(config_path)?;

    if use_signaling {
        start_signaling();
    }

    agent
        .on_delegate(|msg| {
            tracing::info!("[handler] Received: {}", msg.envelope.msg_id);
            None
        })
        .await;

    server::start_server(agent, port)
        .await
        .map_err(|e| anyhow::anyhow!("Server error: {e}"))
}

/// Spawn relay registration and polling, or log why signaling stays off.
fn start_signaling() {
    match signaling::SignalingConfig::from_env() {
        Ok(cfg) => {
            tracing::info!("Starting signaling to relay: {}", cfg.relay_url);
            let pending_store = Arc::new(RwLock::new(Vec::new()));
            tokio::spawn(signaling::register_loop(cfg.clone()));
            tokio::spawn(signaling::poll_loop(cfg, pending_store));
        }
        Err(e) => tracing::warn!("Signaling disabled: {e}"),
    }
}

async fn send_message(
    config_path: Option<&str>,
    target: &str,
    payload_json: &str,
) -> anyhow::Result<()> {
    let payload: serde_json::Value = serde_json::from_str(payload_json)
        .map_err(|e| anyhow::anyhow!("Invalid JSON payload: {e}"))?;

    let agent = load_agent(config_path)?;
    let msg = agent
        .send_message(target, payload, None)
        .await
        .map_err(|e| anyhow::anyhow!("Send failed: {e}"))?;

    tracing::info!("Message sent: {} -> {target}", msg.envelope.msg_id);
    Ok(())
}

async fn listen_messages(config_path: Option<&str>, poll_interval: u64) -> anyhow::Result<()> {
    tracing::info!("Listening for messages (poll interval: {poll_interval}s)...");
    let agent = load_agent(config_path)?;

    agent
        .on_delegate(|msg| {
            tracing::info!(
                "[RECV] From: {}@{:?} | msg_id: {} | payload: {:?}",
                msg.envelope.sender.agent_id,
                msg.envelope.sender.machine_id,
                msg.envelope.msg_id,
                msg.payload,
            );
            Some(serde_json::json!({"status": "received"}))
        })
        .await;

    let relay = agent
        .config
        .peers
        .iter()
        .find(|p| p.agent_id == RELAY_AGENT_ID)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("No '{RELAY_AGENT_ID}' peer found in config"))?;

    tracing::info!("Polling relay at {}", relay.http_endpoint);

    loop {
        match agent.client.poll_pending(&relay).await {
            Ok(resp) => drain_pending(&agent, &relay, &resp).await,
            Err(e) => tracing::warn!("Poll error: {e}"),
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(poll_interval)).await;
    }
}

/// Process every message in a `pending` response and acknowledge each one.
async fn drain_pending(agent: &ACPAgent, relay: &Peer, resp: &serde_json::Value) {
    let Some(messages) = resp.get("messages").and_then(|m| m.as_array()) else {
        return;
    };
    if messages.is_empty() {
        return;
    }

    tracing::info!("Received {} message(s)", messages.len());
    for raw in messages {
        let Some(message) = parse_pending(raw) else {
            tracing::warn!("Skipping unreadable pending message: {raw}");
            continue;
        };

        let msg_id = message.envelope.msg_id.clone();
        agent.handle_incoming(message).await;
        if let Err(e) = agent
            .client
            .ack_message(relay, &msg_id, MessageAck::processed("hop_ack"))
            .await
        {
            tracing::warn!("[ACK] Failed for {msg_id}: {e}");
        }
    }
}

fn parse_pending(raw: &serde_json::Value) -> Option<Message> {
    let envelope: Envelope = serde_json::from_value(raw.get("envelope")?.clone()).ok()?;
    Some(Message {
        envelope,
        payload: raw.get("payload").cloned(),
    })
}

fn doctor(config_path: Option<&str>, target: Option<&str>) -> anyhow::Result<()> {
    tracing::info!("Running ACP doctor check...");
    let agent = load_agent(config_path)?;

    tracing::info!("Agent ID: {}", agent.this.agent_id);
    tracing::info!("Machine ID: {:?}", agent.this.machine_id);
    tracing::info!("HTTP Endpoint: {:?}", agent.this.http_endpoint);

    if let Some(target_id) = target {
        if let Some(peer) = agent.config.get_peer(target_id) {
            tracing::info!(
                "Target peer {target_id}: {} at {}",
                peer.addr(),
                peer.http_endpoint,
            );
        } else {
            tracing::warn!("Peer {target_id} not found in config");
        }
    }

    if std::env::var("ACP_SHARED_SECRET").is_ok() {
        tracing::info!("ACP_SHARED_SECRET: set");
    } else {
        tracing::warn!("ACP_SHARED_SECRET: not set");
    }

    tracing::info!("Doctor check complete");
    Ok(())
}
