//! SQLite-backed configuration store.
//!
//! Config outlives any single gRPC client (frontends are external), so it lives
//! in a modem-owned SQLite file. Channels are keyed on the stable `DeviceId`,
//! not a volatile device path, so a device that moves nodes still binds.
//!
//! Phase 1 note: writes happen on the core thread. There is no audio pump yet,
//! so this cannot stall the sample path. When DSP lands (Phase 3), persistence
//! moves to the control edge or a dedicated writer thread per the design.

use crate::ids::{ChannelId, DeviceId};
use crate::supervisor::channel::ChannelConfig;
use rusqlite::Connection;

/// Errors from the persistence layer.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// A SQLite-backed config store.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (creating if needed) a store at `path` and apply the schema.
    pub fn open(path: &std::path::Path) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    /// Open an in-memory store (tests).
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self, StoreError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS channels (
                 id        INTEGER PRIMARY KEY,
                 name      TEXT NOT NULL,
                 mode      TEXT NOT NULL,
                 device_id TEXT NOT NULL
             );",
        )?;
        Ok(Store { conn })
    }

    /// Insert or update a channel config (idempotent on channel id).
    pub fn upsert_channel(&self, cfg: &ChannelConfig) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO channels (id, name, mode, device_id)
                 VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
                 name = excluded.name,
                 mode = excluded.mode,
                 device_id = excluded.device_id;",
            rusqlite::params![cfg.id.0, cfg.name, cfg.mode, cfg.device_id.0],
        )?;
        Ok(())
    }

    /// Load all persisted channels, ordered by id.
    pub fn load_channels(&self) -> Result<Vec<ChannelConfig>, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, mode, device_id FROM channels ORDER BY id;")?;
        let rows = stmt.query_map([], |row| {
            Ok(ChannelConfig {
                id: ChannelId(row.get::<_, u32>(0)?),
                name: row.get(1)?,
                mode: row.get(2)?,
                device_id: DeviceId(row.get::<_, String>(3)?),
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(id: u32, name: &str) -> ChannelConfig {
        ChannelConfig {
            id: ChannelId(id),
            name: name.to_string(),
            mode: "none".to_string(),
            device_id: DeviceId::placeholder(),
        }
    }

    #[test]
    fn upsert_then_load_roundtrips() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_channel(&cfg(0, "vfo-a")).unwrap();
        store.upsert_channel(&cfg(1, "vfo-b")).unwrap();

        let loaded = store.load_channels().unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, ChannelId(0));
        assert_eq!(loaded[0].name, "vfo-a");
        assert_eq!(loaded[1].name, "vfo-b");
        assert_eq!(loaded[0].device_id, DeviceId::placeholder());
    }

    #[test]
    fn upsert_is_idempotent_on_id() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_channel(&cfg(0, "first")).unwrap();
        store.upsert_channel(&cfg(0, "second")).unwrap();

        let loaded = store.load_channels().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "second");
    }
}
