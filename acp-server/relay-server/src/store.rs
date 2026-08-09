//! SQLite-backed message store

use crate::models::{Envelope, Peer, PendingMessage};
use rusqlite::{params, Connection, Result as SqlResult};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

const PEER_TTL_SECONDS: f64 = 300.0;

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

impl Store {
    pub fn new(db_path: &Path) -> SqlResult<Self> {
        let conn = Connection::open(db_path)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }

    pub fn put(&self, msg_id: &str, envelope: &Envelope, payload: &Option<serde_json::Value>) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        let env_json = serde_json::to_string(envelope).unwrap_or_default();
        let payload_json = payload.as_ref().map(|p| serde_json::to_string(p).unwrap_or_default());
        let sender = &envelope.sender;
        let recipient = &envelope.recipient;
        let origin = envelope.origin.as_ref();
        let corr_id = envelope.corr_id.as_deref().unwrap_or(msg_id);
        let intent = &envelope.intent;
        let content_type = envelope.content_type.as_deref().unwrap_or("application/json");
        let priority = envelope.priority.as_deref().unwrap_or("normal");
        let deadline = Self::parse_deadline(envelope.deadline.as_deref());

        conn.execute(
            r#"INSERT INTO messages
               (msg_id, corr_id, origin_agent, origin_machine, origin_human,
                sender_agent, sender_machine, recipient_agent, recipient_machine,
                intent, content_type, priority, payload, envelope_json,
                created_at, deadline, error, status, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(msg_id) DO UPDATE SET
                   envelope_json = excluded.envelope_json,
                   payload = excluded.payload,
                   updated_at = excluded.updated_at"#,
            params![
                msg_id, corr_id,
                origin.as_ref().map(|o| o.agent_id.as_str()).unwrap_or(""),
                origin.as_ref().and_then(|o| o.machine_id.as_deref()).unwrap_or(""),
                "",
                sender.agent_id.as_str(),
                sender.machine_id.as_deref().unwrap_or(""),
                recipient.agent_id.as_str(),
                recipient.machine_id.as_deref().unwrap_or(""),
                intent, content_type, priority,
                payload_json.as_deref().unwrap_or(""),
                env_json, now(), deadline, "", "pending", now(),
            ],
        )?;
        Ok(())
    }

    pub fn get_all_pending(&self, recipient_agent_id: &str) -> SqlResult<Vec<PendingMessage>> {
        let conn = self.conn.lock().unwrap();
        let cutoff = now() - 86400.0;
        let mut stmt = conn.prepare(
            "SELECT envelope_json, payload FROM messages
             WHERE recipient_agent = ?
               AND status IN ('pending', 'delivered')
               AND (deadline IS NULL OR deadline > ?)
             ORDER BY created_at ASC"
        )?;
        let rows = stmt.query_map(params![recipient_agent_id, cutoff], |row| {
            let env_json: String = row.get(0)?;
            let payload_str: Option<String> = row.get(1)?;
            Ok(PendingMessage {
                envelope: serde_json::from_str(&env_json).unwrap_or_else(|_| {
                    serde_json::from_str(r#"{"msg_id":"?"}"#).unwrap()
                }),
                payload: payload_str.and_then(|s| {
                    if s.is_empty() { None } else { serde_json::from_str(&s).ok() }
                }),
            })
        })?;
        let rows = rows.collect::<SqlResult<Vec<_>>>()?;
        let mut result = Vec::new();
        for msg in rows {
            result.push(msg);
        }
        Ok(result)
    }

    pub fn update_status(&self, msg_id: &str, status: &str) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE messages SET status = ?, updated_at = ? WHERE msg_id = ?",
            params![status, now(), msg_id],
        )?;
        Ok(())
    }

    pub fn get_all_messages(&self) -> SqlResult<Vec<serde_json::Value>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT msg_id, corr_id, recipient_agent, sender_agent, intent, status, error, payload
              FROM messages LIMIT 50"
        )?;
        let rows = stmt.query_map([], |row| {
            let payload: Option<String> = row.get(7)?;
            Ok(serde_json::json!({
                "msg_id": row.get::<_, String>(0)?,
                "corr_id": row.get::<_, String>(1)?,
                "recipient_agent": row.get::<_, String>(2)?,
                "sender_agent": row.get::<_, String>(3)?,
                "intent": row.get::<_, String>(4)?,
                "status": row.get::<_, String>(5)?,
                "error": row.get::<_, Option<String>>(6)?,
                "payload": payload.and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok()),
            }))
        })?;
        let mut result = Vec::new();
        for r in rows { if let Ok(v) = r { result.push(v); } }
        Ok(result)
    }

    pub fn register_peer(&self, peer: &Peer) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        let caps = peer.capabilities.as_ref()
            .map(|c| serde_json::to_string(c).unwrap_or_default())
            .unwrap_or_default();
        conn.execute(
            r#"INSERT INTO peer_registry
               (agent_id, machine_id, http_endpoint, ws_endpoint,
                capabilities_json, last_seen_at, registered_at)
               VALUES (?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(agent_id) DO UPDATE SET
                   machine_id = excluded.machine_id,
                   http_endpoint = excluded.http_endpoint,
                   ws_endpoint = excluded.ws_endpoint,
                   capabilities_json = excluded.capabilities_json,
                   last_seen_at = excluded.last_seen_at"#,
            params![
                peer.agent_id, peer.machine_id, peer.http_endpoint,
                peer.ws_endpoint.as_deref().unwrap_or(""),
                caps, now(), now(),
            ],
        )?;
        Ok(())
    }

    pub fn get_peers(&self, include_stale: bool) -> SqlResult<Vec<Peer>> {
        let conn = self.conn.lock().unwrap();
        let peers = if include_stale {
            let mut stmt = conn.prepare("SELECT * FROM peer_registry")?;
            let rows = stmt.query_map([], peer_row_map)?;
            rows.collect::<SqlResult<Vec<Peer>>>()?
        } else {
            let cutoff = now() - PEER_TTL_SECONDS;
            let mut stmt = conn.prepare("SELECT * FROM peer_registry WHERE last_seen_at > ?")?;
            let rows = stmt.query_map(params![cutoff], peer_row_map)?;
            rows.collect::<SqlResult<Vec<Peer>>>()?
        };
        Ok(peers)
    }

    pub fn remove_peer(&self, agent_id: &str) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM peer_registry WHERE agent_id = ?", params![agent_id])?;
        Ok(())
    }

    pub fn touch_peer(&self, agent_id: &str) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE peer_registry SET last_seen_at = ? WHERE agent_id = ?",
            params![now(), agent_id],
        )?;
        Ok(())
    }

    fn parse_deadline(deadline: Option<&str>) -> Option<f64> {
        let deadline = deadline?;
        let deadline = deadline.trim_end_matches('Z');
        chrono::DateTime::parse_from_rfc3339(deadline)
            .ok()
            .map(|dt| dt.timestamp() as f64)
    }
}

fn peer_row_map(row: &rusqlite::Row) -> rusqlite::Result<Peer> {
    let ws_endpoint: Option<String> = row.get("ws_endpoint")?;
    let capabilities: Option<Vec<String>> = row
        .get::<_, String>("capabilities_json")
        .ok()
        .and_then(|s| if s.is_empty() { None } else { serde_json::from_str(&s).ok() });
    Ok(Peer {
        agent_id: row.get("agent_id")?,
        machine_id: row.get("machine_id")?,
        http_endpoint: row.get("http_endpoint")?,
        ws_endpoint: ws_endpoint.filter(|s| !s.is_empty()),
        capabilities,
        last_seen_at: row.get("last_seen_at").ok(),
    })
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS messages (
    msg_id        TEXT PRIMARY KEY,
    corr_id       TEXT NOT NULL,
    origin_agent  TEXT,
    origin_machine TEXT,
    origin_human  TEXT,
    sender_agent  TEXT NOT NULL,
    sender_machine TEXT NOT NULL,
    recipient_agent TEXT NOT NULL,
    recipient_machine TEXT NOT NULL,
    intent        TEXT NOT NULL,
    content_type  TEXT DEFAULT 'application/json',
    priority      TEXT DEFAULT 'normal',
    payload       TEXT,
    envelope_json TEXT NOT NULL,
    created_at    REAL NOT NULL,
    deadline      REAL,
    error         TEXT,
    status        TEXT DEFAULT 'pending',
    updated_at    REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS peer_registry (
    agent_id      TEXT PRIMARY KEY,
    machine_id   TEXT NOT NULL,
    http_endpoint TEXT NOT NULL,
    ws_endpoint  TEXT,
    capabilities_json TEXT DEFAULT '[]',
    last_seen_at REAL NOT NULL,
    registered_at REAL NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_messages_status ON messages(status);
CREATE INDEX IF NOT EXISTS idx_messages_recipient ON messages(recipient_agent, recipient_machine);
CREATE INDEX IF NOT EXISTS idx_messages_created ON messages(created_at);
"#;
