//! SQLite-backed message store and peer registry.
//!
//! The connection is behind a [`parking_lot::Mutex`]: `SQLite` serializes writers
//! anyway, and `parking_lot`'s guard cannot be poisoned, so no read path has to
//! deal with a lock that a panicking writer left behind.

use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use rusqlite::{params, Connection, Result as SqlResult};

use crate::models::{Envelope, Peer, PendingMessage};

/// How long after its last sighting a peer stops being offered for routing.
const PEER_TTL_SECONDS: f64 = 300.0;

/// How far back `get_all_pending` looks when filtering on a deadline.
const PENDING_WINDOW_SECONDS: f64 = 86_400.0;

/// Cap on the debug message listing.
const DEBUG_MESSAGE_LIMIT: u32 = 50;

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Handle on the relay's `SQLite` database. Cloning shares one connection.
#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

impl Store {
    /// Open (or create) the database at `db_path` and apply the schema.
    ///
    /// # Errors
    /// Returns the `rusqlite` error when the file cannot be opened or the
    /// schema cannot be applied.
    pub fn new(db_path: &Path) -> SqlResult<Self> {
        let conn = Connection::open(db_path)?;
        conn.execute_batch(SCHEMA)?;
        // Added after the first release; existing databases predate the column.
        conn.execute(
            "ALTER TABLE peer_registry ADD COLUMN reachable INTEGER NOT NULL DEFAULT 1",
            [],
        )
        .ok();
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Insert a message without replacing an existing message with the same ID.
    ///
    /// # Errors
    /// Returns the `rusqlite` error when the insert fails.
    pub fn put(
        &self,
        msg_id: &str,
        envelope: &Envelope,
        payload: &Option<serde_json::Value>,
    ) -> SqlResult<()> {
        let conn = self.conn.lock();
        let env_json = serde_json::to_string(envelope).unwrap_or_default();
        let payload_json = payload
            .as_ref()
            .map(|p| serde_json::to_string(p).unwrap_or_default());
        let origin = envelope.origin.as_ref();

        conn.execute(
            r"INSERT INTO messages
               (msg_id, corr_id, origin_agent, origin_machine, origin_human,
                sender_agent, sender_machine, recipient_agent, recipient_machine,
                intent, content_type, priority, payload, envelope_json,
                created_at, deadline, error, status, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(msg_id) DO NOTHING",
            params![
                msg_id,
                envelope.corr_id.as_deref().unwrap_or(msg_id),
                origin.map_or("", |o| o.agent_id.as_str()),
                origin.and_then(|o| o.machine_id.as_deref()).unwrap_or(""),
                "",
                envelope.sender.agent_id.as_str(),
                envelope.sender.machine_id.as_deref().unwrap_or(""),
                envelope.recipient.agent_id.as_str(),
                envelope.recipient.machine_id.as_deref().unwrap_or(""),
                &envelope.intent,
                envelope
                    .content_type
                    .as_deref()
                    .unwrap_or("application/json"),
                envelope.priority.as_deref().unwrap_or("normal"),
                payload_json.as_deref().unwrap_or(""),
                env_json,
                now(),
                Self::parse_deadline(envelope.deadline.as_deref()),
                "",
                "pending",
                now(),
            ],
        )?;
        Ok(())
    }

    /// Every message awaiting collection by `recipient_agent_id`, oldest first.
    ///
    /// Rows whose stored envelope no longer parses are skipped rather than
    /// failing the whole poll — one bad row must not strand an agent's inbox.
    ///
    /// # Errors
    /// Returns the `rusqlite` error when the query fails.
    pub fn get_all_pending(&self, recipient_agent_id: &str) -> SqlResult<Vec<PendingMessage>> {
        let conn = self.conn.lock();
        let cutoff = now() - PENDING_WINDOW_SECONDS;
        let mut stmt = conn.prepare(
            "SELECT envelope_json, payload FROM messages
             WHERE recipient_agent = ?
               AND status IN ('pending', 'delivered')
               AND (deadline IS NULL OR deadline > ?)
             ORDER BY created_at ASC",
        )?;

        let rows = stmt.query_map(params![recipient_agent_id, cutoff], |row| {
            let env_json: String = row.get(0)?;
            let payload_str: Option<String> = row.get(1)?;
            Ok(serde_json::from_str::<Envelope>(&env_json)
                .ok()
                .map(|envelope| PendingMessage {
                    envelope,
                    payload: payload_str
                        .filter(|s| !s.is_empty())
                        .and_then(|s| serde_json::from_str(&s).ok()),
                }))
        })?;

        rows.collect::<SqlResult<Vec<_>>>()
            .map(|msgs| msgs.into_iter().flatten().collect())
    }

    /// Move a message to `status`.
    ///
    /// # Errors
    /// Returns the `rusqlite` error when the update fails.
    pub fn update_status(&self, msg_id: &str, status: &str) -> SqlResult<()> {
        self.conn.lock().execute(
            "UPDATE messages SET status = ?, updated_at = ? WHERE msg_id = ?",
            params![status, now(), msg_id],
        )?;
        Ok(())
    }

    /// Delivery status of one message: `(status, updated_at)`.
    ///
    /// # Errors
    /// Returns the `rusqlite` error when the query fails.
    pub fn get_status(&self, msg_id: &str) -> SqlResult<Option<(String, f64)>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT status, updated_at FROM messages WHERE msg_id = ?")?;
        let mut rows = stmt.query(params![msg_id])?;
        match rows.next()? {
            Some(row) => Ok(Some((row.get(0)?, row.get(1)?))),
            None => Ok(None),
        }
    }

    /// Return the sender and recipient recorded for one message.
    ///
    /// # Errors
    /// Returns the `SQLite` error when the query fails.
    pub fn get_message_addresses(&self, msg_id: &str) -> SqlResult<Option<(String, String)>> {
        let conn = self.conn.lock();
        let mut stmt =
            conn.prepare("SELECT sender_agent, recipient_agent FROM messages WHERE msg_id = ?")?;
        let mut rows = stmt.query(params![msg_id])?;
        match rows.next()? {
            Some(row) => Ok(Some((row.get(0)?, row.get(1)?))),
            None => Ok(None),
        }
    }

    /// The most recent messages, as JSON for the dashboard.
    ///
    /// Capped at the 50 most recent; rows that fail to read are skipped.
    ///
    /// # Errors
    /// Returns the `rusqlite` error when the query fails.
    pub fn get_all_messages(&self) -> SqlResult<Vec<serde_json::Value>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT msg_id, corr_id, recipient_agent, sender_agent, intent, status, error, payload,
                    envelope_json, created_at, updated_at
              FROM messages
              ORDER BY updated_at DESC
              LIMIT ?",
        )?;
        let rows = stmt.query_map(params![DEBUG_MESSAGE_LIMIT], |row| {
            let payload: Option<String> = row.get(7)?;
            let envelope: serde_json::Value = row
                .get::<_, String>(8)
                .ok()
                .and_then(|value| serde_json::from_str(&value).ok())
                .unwrap_or(serde_json::Value::Null);
            Ok(serde_json::json!({
                "msg_id": row.get::<_, String>(0)?,
                "corr_id": row.get::<_, String>(1)?,
                "session_id": envelope.get("session_id"),
                "run_id": envelope.get("run_id"),
                "recipient_agent": row.get::<_, String>(2)?,
                "sender_agent": row.get::<_, String>(3)?,
                "intent": row.get::<_, String>(4)?,
                "status": row.get::<_, String>(5)?,
                "error": row.get::<_, Option<String>>(6)?,
                "payload": payload.and_then(|v| serde_json::from_str::<serde_json::Value>(&v).ok()),
                "created_at": row.get::<_, f64>(9)?,
                "updated_at": row.get::<_, f64>(10)?,
            }))
        })?;
        Ok(rows.flatten().collect())
    }

    /// Record a peer's endpoint, clearing any earlier unreachable mark.
    ///
    /// # Errors
    /// Returns the `rusqlite` error when the upsert fails.
    pub fn register_peer(&self, peer: &Peer) -> SqlResult<()> {
        let conn = self.conn.lock();
        let caps = peer
            .capabilities
            .as_ref()
            .map(|c| serde_json::to_string(c).unwrap_or_default())
            .unwrap_or_default();
        conn.execute(
            r"INSERT INTO peer_registry
               (agent_id, machine_id, http_endpoint, ws_endpoint,
                capabilities_json, last_seen_at, registered_at, reachable)
               VALUES (?, ?, ?, ?, ?, ?, ?, 1)
               ON CONFLICT(agent_id) DO UPDATE SET
                   machine_id = excluded.machine_id,
                   http_endpoint = excluded.http_endpoint,
                   ws_endpoint = excluded.ws_endpoint,
                   capabilities_json = excluded.capabilities_json,
                   last_seen_at = excluded.last_seen_at,
                   reachable = 1",
            params![
                peer.agent_id,
                peer.machine_id,
                peer.http_endpoint,
                peer.ws_endpoint.as_deref().unwrap_or(""),
                caps,
                now(),
                now(),
            ],
        )?;
        Ok(())
    }

    /// Registered peers; `include_stale` also returns those past the 5-minute TTL.
    ///
    /// # Errors
    /// Returns the `rusqlite` error when the query fails.
    pub fn get_peers(&self, include_stale: bool) -> SqlResult<Vec<Peer>> {
        let conn = self.conn.lock();
        if include_stale {
            let mut stmt = conn.prepare("SELECT * FROM peer_registry")?;
            let rows = stmt.query_map([], peer_row_map)?;
            rows.collect()
        } else {
            let cutoff = now() - PEER_TTL_SECONDS;
            let mut stmt = conn.prepare("SELECT * FROM peer_registry WHERE last_seen_at > ?")?;
            let rows = stmt.query_map(params![cutoff], peer_row_map)?;
            rows.collect()
        }
    }

    /// Drop a peer from the registry.
    ///
    /// # Errors
    /// Returns the `rusqlite` error when the delete fails.
    pub fn remove_peer(&self, agent_id: &str) -> SqlResult<()> {
        self.conn.lock().execute(
            "DELETE FROM peer_registry WHERE agent_id = ?",
            params![agent_id],
        )?;
        Ok(())
    }

    /// Record that a push to this peer's `http_endpoint` failed.
    ///
    /// The peer stays registered — it may well be a poll-only agent that never
    /// accepts pushes — so the relay just stops paying the forward timeout and
    /// brokers instead. Registering again clears the flag.
    ///
    /// # Errors
    /// Returns the `rusqlite` error when the update fails.
    pub fn mark_peer_unreachable(&self, agent_id: &str) -> SqlResult<()> {
        self.conn.lock().execute(
            "UPDATE peer_registry SET reachable = 0 WHERE agent_id = ?",
            params![agent_id],
        )?;
        Ok(())
    }

    /// Refresh a peer's `last_seen_at` without changing its endpoint.
    ///
    /// # Errors
    /// Returns the `rusqlite` error when the update fails.
    pub fn touch_peer(&self, agent_id: &str) -> SqlResult<()> {
        self.conn.lock().execute(
            "UPDATE peer_registry SET last_seen_at = ? WHERE agent_id = ?",
            params![now(), agent_id],
        )?;
        Ok(())
    }

    /// Read an RFC 3339 deadline as Unix seconds, tolerating a trailing `Z`.
    ///
    /// The column is REAL, so the timestamp has to become an `f64`. Unix seconds
    /// stay exact in a 53-bit mantissa until the year 285 million.
    #[expect(
        clippy::cast_precision_loss,
        reason = "Unix seconds fit exactly in f64 for any date this system will see"
    )]
    fn parse_deadline(deadline: Option<&str>) -> Option<f64> {
        let deadline = deadline?.trim_end_matches('Z');
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
        .filter(|s| !s.is_empty())
        .and_then(|s| serde_json::from_str(&s).ok());
    Ok(Peer {
        agent_id: row.get("agent_id")?,
        machine_id: row.get("machine_id")?,
        http_endpoint: row.get("http_endpoint")?,
        ws_endpoint: ws_endpoint.filter(|s| !s.is_empty()),
        capabilities,
        last_seen_at: row.get("last_seen_at").ok(),
        reachable: row
            .get::<_, i64>("reachable")
            .map(|v| v != 0)
            .unwrap_or(true),
    })
}

const SCHEMA: &str = r"
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
";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AgentAddr;

    fn store() -> Store {
        Store::new(Path::new(":memory:")).unwrap()
    }

    fn envelope(msg_id: &str, recipient: &str) -> Envelope {
        Envelope {
            msg_id: msg_id.to_string(),
            corr_id: None,
            session_id: None,
            run_id: None,
            origin: None,
            sender: AgentAddr {
                agent_id: "agent-alpha".to_string(),
                machine_id: Some("laptop-1".to_string()),
            },
            recipient: AgentAddr {
                agent_id: recipient.to_string(),
                machine_id: Some("server-1".to_string()),
            },
            reply_to: None,
            intent: "delegate".to_string(),
            content_type: None,
            priority: None,
            deadline: None,
            error: None,
            hops: None,
        }
    }

    fn peer(agent_id: &str) -> Peer {
        Peer {
            agent_id: agent_id.to_string(),
            machine_id: "server-1".to_string(),
            http_endpoint: "http://localhost:8444".to_string(),
            ws_endpoint: None,
            capabilities: Some(vec!["agent".to_string()]),
            last_seen_at: None,
            reachable: true,
        }
    }

    #[test]
    fn a_stored_message_is_pending_for_its_recipient() {
        let store = store();
        store
            .put("msg_1", &envelope("msg_1", "agent-beta"), &None)
            .unwrap();

        let pending = store.get_all_pending("agent-beta").unwrap();

        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn a_stored_message_is_not_pending_for_anyone_else() {
        let store = store();
        store
            .put("msg_1", &envelope("msg_1", "agent-beta"), &None)
            .unwrap();

        assert!(store.get_all_pending("agent-gamma").unwrap().is_empty());
    }

    #[test]
    fn an_acknowledged_message_stops_being_pending() {
        let store = store();
        store
            .put("msg_1", &envelope("msg_1", "agent-beta"), &None)
            .unwrap();

        store.update_status("msg_1", "acknowledged").unwrap();

        assert!(store.get_all_pending("agent-beta").unwrap().is_empty());
    }

    #[test]
    fn a_delivered_message_stays_pending_until_acknowledged() {
        let store = store();
        store
            .put("msg_1", &envelope("msg_1", "agent-beta"), &None)
            .unwrap();

        store.update_status("msg_1", "delivered").unwrap();

        assert_eq!(store.get_all_pending("agent-beta").unwrap().len(), 1);
    }

    #[test]
    fn a_stored_payload_survives_the_roundtrip() {
        let store = store();
        let payload = Some(serde_json::json!({"task": "review"}));
        store
            .put("msg_1", &envelope("msg_1", "agent-beta"), &payload)
            .unwrap();

        let pending = store.get_all_pending("agent-beta").unwrap();

        assert_eq!(pending[0].payload, payload);
    }

    #[test]
    fn putting_the_same_message_twice_does_not_duplicate_it() {
        let store = store();
        let env = envelope("msg_1", "agent-beta");
        store.put("msg_1", &env, &None).unwrap();

        store.put("msg_1", &env, &None).unwrap();

        assert_eq!(store.get_all_pending("agent-beta").unwrap().len(), 1);
    }

    #[test]
    fn an_unknown_message_has_no_status() {
        assert!(store().get_status("msg_missing").unwrap().is_none());
    }

    #[test]
    fn a_registered_peer_is_listed() {
        let store = store();
        store.register_peer(&peer("agent-beta")).unwrap();

        let peers = store.get_peers(false).unwrap();

        assert_eq!(peers.len(), 1);
    }

    #[test]
    fn a_registered_peer_starts_reachable() {
        let store = store();
        store.register_peer(&peer("agent-beta")).unwrap();

        assert!(store.get_peers(false).unwrap()[0].reachable);
    }

    #[test]
    fn marking_a_peer_unreachable_keeps_it_registered() {
        let store = store();
        store.register_peer(&peer("agent-beta")).unwrap();

        store.mark_peer_unreachable("agent-beta").unwrap();

        assert!(!store.get_peers(false).unwrap()[0].reachable);
    }

    #[test]
    fn re_registering_clears_the_unreachable_mark() {
        let store = store();
        store.register_peer(&peer("agent-beta")).unwrap();
        store.mark_peer_unreachable("agent-beta").unwrap();

        store.register_peer(&peer("agent-beta")).unwrap();

        assert!(store.get_peers(false).unwrap()[0].reachable);
    }

    #[test]
    fn a_removed_peer_is_gone() {
        let store = store();
        store.register_peer(&peer("agent-beta")).unwrap();

        store.remove_peer("agent-beta").unwrap();

        assert!(store.get_peers(true).unwrap().is_empty());
    }

    #[test]
    fn a_deadline_with_a_trailing_z_parses() {
        assert!(Store::parse_deadline(Some("2030-01-01T00:00:00+00:00Z")).is_some());
    }

    #[test]
    fn an_unparseable_deadline_is_ignored() {
        assert!(Store::parse_deadline(Some("not-a-date")).is_none());
    }
}
