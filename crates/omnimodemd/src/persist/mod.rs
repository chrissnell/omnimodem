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
                 ptt_invert    INTEGER NOT NULL DEFAULT 0,
                 tx_device_id  TEXT NOT NULL DEFAULT '',
                 tx_sample_rate INTEGER NOT NULL DEFAULT 0,
                 rsid_tx       INTEGER NOT NULL DEFAULT 0,
                 rsid_rx       INTEGER NOT NULL DEFAULT 0
             );",
        )?;
        // Idempotent migration for a DB created by the Phase-1 build, whose
        // `channels` table predates the audio/PTT columns. `CREATE TABLE` above
        // provisions them on a fresh DB; these `ALTER`s add them to an older
        // table. On a fresh DB each column already exists, so SQLite returns a
        // "duplicate column name" error, which we treat as "already migrated".
        for ddl in [
            "ALTER TABLE channels ADD COLUMN sample_rate INTEGER NOT NULL DEFAULT 48000",
            "ALTER TABLE channels ADD COLUMN fanout INTEGER NOT NULL DEFAULT 1",
            "ALTER TABLE channels ADD COLUMN ptt_method TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE channels ADD COLUMN ptt_device_id TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE channels ADD COLUMN ptt_node TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE channels ADD COLUMN ptt_pin INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE channels ADD COLUMN ptt_invert INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE channels ADD COLUMN tx_device_id TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE channels ADD COLUMN tx_sample_rate INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE channels ADD COLUMN rsid_tx INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE channels ADD COLUMN rsid_rx INTEGER NOT NULL DEFAULT 0",
        ] {
            match conn.execute(ddl, []) {
                Ok(_) => {}
                Err(rusqlite::Error::SqliteFailure(_, Some(msg)))
                    if msg.contains("duplicate column name") => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(Store { conn })
    }

    /// Insert or update a channel config (idempotent on channel id).
    pub fn upsert_channel(&self, cfg: &ChannelConfig) -> Result<(), StoreError> {
        let (method, ptt_dev, node, pin, invert) = encode_ptt(&cfg.ptt);
        self.conn.execute(
            "INSERT INTO channels
                 (id, name, mode, device_id, sample_rate, fanout,
                  ptt_method, ptt_device_id, ptt_node, ptt_pin, ptt_invert,
                  tx_device_id, tx_sample_rate, rsid_tx, rsid_rx)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
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
                 ptt_invert = excluded.ptt_invert,
                 tx_device_id = excluded.tx_device_id,
                 tx_sample_rate = excluded.tx_sample_rate,
                 rsid_tx = excluded.rsid_tx,
                 rsid_rx = excluded.rsid_rx;",
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
                cfg.tx_device_id.to_canonical_string(),
                cfg.tx_sample_rate,
                cfg.rsid_tx as i64,
                cfg.rsid_rx as i64,
            ],
        )?;
        Ok(())
    }

    /// Load all persisted channels, ordered by id.
    pub fn load_channels(&self) -> Result<Vec<ChannelConfig>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, mode, device_id, sample_rate, fanout,
                    ptt_method, ptt_device_id, ptt_node, ptt_pin, ptt_invert,
                    tx_device_id, tx_sample_rate, rsid_tx, rsid_rx
             FROM channels ORDER BY id;",
        )?;
        let rows = stmt.query_map([], |row| {
            let method: String = row.get(6)?;
            let ptt_dev: String = row.get(7)?;
            let node: String = row.get(8)?;
            let pin: i64 = row.get(9)?;
            let invert: i64 = row.get(10)?;
            let capture_dev = DeviceId::parse(&row.get::<_, String>(3)?)
                .unwrap_or_else(DeviceId::placeholder);
            // Empty tx_device_id == legacy / single-rig row: TX follows capture.
            let tx_dev_str: String = row.get(11)?;
            let tx_device_id = if tx_dev_str.is_empty() {
                capture_dev.clone()
            } else {
                DeviceId::parse(&tx_dev_str).unwrap_or_else(DeviceId::placeholder)
            };
            Ok(ChannelConfig {
                id: ChannelId(row.get::<_, u32>(0)?),
                name: row.get(1)?,
                mode: row.get(2)?,
                device_id: capture_dev,
                sample_rate: row.get(4)?,
                fanout: row.get(5)?,
                tx_device_id,
                tx_sample_rate: row.get(12)?,
                ptt: decode_ptt(&method, &ptt_dev, &node, pin, invert),
                rsid_tx: row.get::<_, i64>(13)? != 0,
                rsid_rx: row.get::<_, i64>(14)? != 0,
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
            tx_device_id: DeviceId::placeholder(),
            tx_sample_rate: 0,
            ptt: None,
            rsid_tx: false,
            rsid_rx: false,
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
    fn rsid_flags_roundtrip() {
        let store = Store::open_in_memory().unwrap();
        let mut c = cfg(0, "vfo-a");
        c.rsid_tx = true;
        c.rsid_rx = true;
        store.upsert_channel(&c).unwrap();
        let loaded = store.load_channels().unwrap();
        assert!(loaded[0].rsid_tx && loaded[0].rsid_rx);
        // Default is off on a plain row.
        store.upsert_channel(&cfg(1, "vfo-b")).unwrap();
        let loaded = store.load_channels().unwrap();
        assert!(!loaded[1].rsid_tx && !loaded[1].rsid_rx);
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
    fn migrates_a_phase1_schema_and_backfills_defaults() {
        // A Phase-1 DB: the original four columns, with one row.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE channels (
                 id        INTEGER PRIMARY KEY,
                 name      TEXT NOT NULL,
                 mode      TEXT NOT NULL,
                 device_id TEXT NOT NULL
             );
             INSERT INTO channels (id, name, mode, device_id)
                 VALUES (7, 'legacy', 'none', 'virtual:virtual:0');",
        )
        .unwrap();

        // Opening through the store must add the Phase-2 columns and load.
        let store = Store::init(conn).unwrap();
        let loaded = store.load_channels().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, ChannelId(7));
        assert_eq!(loaded[0].sample_rate, 48_000);
        assert_eq!(loaded[0].fanout, 1);
        assert!(loaded[0].ptt.is_none());
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

    #[test]
    fn roundtrips_split_tx_device() {
        let store = Store::open_in_memory().unwrap();
        let mut c = cfg(5, "split");
        c.device_id = DeviceId::AlsaCard { card_name: "Capture".into() };
        c.tx_device_id = DeviceId::AlsaCard { card_name: "Playback".into() };
        c.tx_sample_rate = 44_100;
        store.upsert_channel(&c).unwrap();

        let loaded = store.load_channels().unwrap();
        assert_eq!(loaded[0].tx_device_id, DeviceId::AlsaCard { card_name: "Playback".into() });
        assert_eq!(loaded[0].tx_sample_rate, 44_100);
    }

    #[test]
    fn legacy_row_backfills_tx_device_from_capture() {
        // A pre-Phase-6 row with no tx_* columns: tx_device_id must follow the
        // capture device, tx_sample_rate must default to 0.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE channels (
                 id        INTEGER PRIMARY KEY,
                 name      TEXT NOT NULL,
                 mode      TEXT NOT NULL,
                 device_id TEXT NOT NULL
             );
             INSERT INTO channels (id, name, mode, device_id)
                 VALUES (8, 'legacy', 'none', 'virtual:virtual:0');",
        )
        .unwrap();
        let store = Store::init(conn).unwrap();
        let loaded = store.load_channels().unwrap();
        assert_eq!(loaded[0].tx_device_id, loaded[0].device_id);
        assert_eq!(loaded[0].tx_sample_rate, 0);
    }
}
