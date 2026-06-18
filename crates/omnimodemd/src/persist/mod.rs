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
use crate::ptt::registry::{PttConfig, PttMethod};
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
                 id            INTEGER PRIMARY KEY,
                 name          TEXT NOT NULL,
                 mode          TEXT NOT NULL,
                 device_id     TEXT NOT NULL,
                 sample_rate   INTEGER NOT NULL DEFAULT 48000,
                 fanout        INTEGER NOT NULL DEFAULT 1,
                 ptt_method    TEXT NOT NULL DEFAULT '',
                 ptt_device_id TEXT NOT NULL DEFAULT '',
                 ptt_node      TEXT NOT NULL DEFAULT '',
                 ptt_pin       INTEGER NOT NULL DEFAULT 0,
                 ptt_invert    INTEGER NOT NULL DEFAULT 0
             );",
        )?;
        Ok(Store { conn })
    }

    /// Insert or update a channel config (idempotent on channel id).
    pub fn upsert_channel(&self, cfg: &ChannelConfig) -> Result<(), StoreError> {
        let (method, ptt_dev, node, pin, invert) = encode_ptt(&cfg.ptt);
        self.conn.execute(
            "INSERT INTO channels
                 (id, name, mode, device_id, sample_rate, fanout,
                  ptt_method, ptt_device_id, ptt_node, ptt_pin, ptt_invert)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(id) DO UPDATE SET
                 name = excluded.name,
                 mode = excluded.mode,
                 device_id = excluded.device_id,
                 sample_rate = excluded.sample_rate,
                 fanout = excluded.fanout,
                 ptt_method = excluded.ptt_method,
                 ptt_device_id = excluded.ptt_device_id,
                 ptt_node = excluded.ptt_node,
                 ptt_pin = excluded.ptt_pin,
                 ptt_invert = excluded.ptt_invert;",
            rusqlite::params![
                cfg.id.0,
                cfg.name,
                cfg.mode,
                cfg.device_id.to_canonical_string(),
                cfg.sample_rate,
                cfg.fanout,
                method,
                ptt_dev,
                node,
                pin,
                invert,
            ],
        )?;
        Ok(())
    }

    /// Load all persisted channels, ordered by id.
    pub fn load_channels(&self) -> Result<Vec<ChannelConfig>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, mode, device_id, sample_rate, fanout,
                    ptt_method, ptt_device_id, ptt_node, ptt_pin, ptt_invert
             FROM channels ORDER BY id;",
        )?;
        let rows = stmt.query_map([], |row| {
            let method: String = row.get(6)?;
            let ptt_dev: String = row.get(7)?;
            let node: String = row.get(8)?;
            let pin: i64 = row.get(9)?;
            let invert: i64 = row.get(10)?;
            Ok(ChannelConfig {
                id: ChannelId(row.get::<_, u32>(0)?),
                name: row.get(1)?,
                mode: row.get(2)?,
                device_id: DeviceId::parse(&row.get::<_, String>(3)?)
                    .unwrap_or_else(DeviceId::placeholder),
                sample_rate: row.get(4)?,
                fanout: row.get(5)?,
                ptt: decode_ptt(&method, &ptt_dev, &node, pin, invert),
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}

/// Flatten an optional `PttConfig` into the persisted columns. An empty method
/// string means "no PTT binding".
fn encode_ptt(ptt: &Option<PttConfig>) -> (String, String, String, i64, i64) {
    let Some(p) = ptt else {
        return (String::new(), String::new(), String::new(), 0, 0);
    };
    let dev = p.device_id.to_canonical_string();
    let (method, node, pin) = match &p.method {
        PttMethod::None => ("none".to_string(), String::new(), 0),
        PttMethod::Vox => ("vox".to_string(), String::new(), 0),
        PttMethod::SerialRts { node } => ("serial_rts".to_string(), node.clone(), 0),
        PttMethod::SerialDtr { node } => ("serial_dtr".to_string(), node.clone(), 0),
        PttMethod::Cm108 { node, pin } => ("cm108".to_string(), node.clone(), *pin as i64),
        PttMethod::Gpio { chip, line } => ("gpio".to_string(), chip.clone(), *line as i64),
    };
    (method, dev, node, pin, p.invert as i64)
}

/// Rebuild a `PttConfig` from the persisted columns; `None` when no binding.
fn decode_ptt(method: &str, dev: &str, node: &str, pin: i64, invert: i64) -> Option<PttConfig> {
    if method.is_empty() {
        return None;
    }
    let m = match method {
        "none" => PttMethod::None,
        "vox" => PttMethod::Vox,
        "serial_rts" => PttMethod::SerialRts { node: node.to_string() },
        "serial_dtr" => PttMethod::SerialDtr { node: node.to_string() },
        "cm108" => PttMethod::Cm108 { node: node.to_string(), pin: pin as u8 },
        "gpio" => PttMethod::Gpio { chip: node.to_string(), line: pin as u32 },
        _ => return None,
    };
    Some(PttConfig {
        device_id: DeviceId::parse(dev).unwrap_or_else(DeviceId::placeholder),
        method: m,
        invert: invert != 0,
    })
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
            sample_rate: 48_000,
            fanout: 1,
            ptt: None,
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

    #[test]
    fn audio_and_ptt_bindings_roundtrip() {
        let store = Store::open_in_memory().unwrap();
        let mut c = cfg(2, "with-ptt");
        c.device_id = DeviceId::AlsaCard { card_name: "Device".into() };
        c.sample_rate = 44_100;
        c.fanout = 2;
        c.ptt = Some(PttConfig {
            device_id: DeviceId::Serial { by_id: "usb-FTDI_x".into() },
            method: PttMethod::Cm108 { node: "/dev/hidraw0".into(), pin: 3 },
            invert: true,
        });
        store.upsert_channel(&c).unwrap();

        let loaded = store.load_channels().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0], c);
    }
}
