use std::fs;
use std::net::IpAddr;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::config::AsnConfig;
use crate::constants::{LOCK_STALE_SECONDS, ROUTES_NFT_PATH, ROUTES_SH_PATH};
use crate::types::{AsnStatus, PolicyStatus, SyncStatus};
use crate::util::{now_rfc3339, now_unix};

pub fn open_database(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let conn =
        Connection::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    conn.busy_timeout(Duration::from_secs(15))?;
    migrate_database(&conn)?;
    Ok(conn)
}

fn migrate_database(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS asn_status (
            asn TEXT PRIMARY KEY,
            state TEXT NOT NULL DEFAULT 'not_synced',
            ipv4_count INTEGER NOT NULL DEFAULT 0,
            ipv6_count INTEGER NOT NULL DEFAULT 0,
            last_synced_at TEXT,
            sync_started_at TEXT,
            last_error TEXT,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS prefixes (
            asn TEXT NOT NULL,
            prefix TEXT NOT NULL,
            family INTEGER NOT NULL,
            synced_at TEXT NOT NULL,
            PRIMARY KEY (asn, prefix)
        );

        CREATE INDEX IF NOT EXISTS prefixes_family_idx
            ON prefixes(family);

        CREATE TABLE IF NOT EXISTS sync_lock (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            mode TEXT NOT NULL,
            locked_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS policy_state (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            state TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            interface_count INTEGER NOT NULL DEFAULT 0,
            ipv4_prefix_count INTEGER NOT NULL DEFAULT 0,
            last_error TEXT
        );
        ",
    )?;

    let _ = conn.execute("ALTER TABLE asn_status ADD COLUMN provider_name TEXT", []);
    let _ = conn.execute(
        "ALTER TABLE asn_status ADD COLUMN provider_updated_at TEXT",
        [],
    );

    Ok(())
}

pub fn ensure_asn_row(conn: &Connection, asn: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO asn_status(asn, state, ipv4_count, ipv6_count, updated_at)
         VALUES(?1, 'not_synced', 0, 0, ?2)",
        params![asn, now_rfc3339()],
    )?;
    Ok(())
}

pub fn has_successful_sync(conn: &Connection, asn: &str) -> Result<bool> {
    let last_synced_at = conn
        .query_row(
            "SELECT last_synced_at FROM asn_status WHERE asn = ?1",
            params![asn],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();

    Ok(last_synced_at.is_some())
}

pub fn save_prefixes(
    conn: &mut Connection,
    asn: &str,
    prefixes: Vec<String>,
    provider_name: Option<String>,
) -> Result<()> {
    let synced_at = now_rfc3339();
    let tx = conn.transaction()?;

    tx.execute("DELETE FROM prefixes WHERE asn = ?1", params![asn])?;

    let mut ipv4_count = 0i64;
    let mut ipv6_count = 0i64;

    {
        let mut stmt = tx.prepare(
            "INSERT OR REPLACE INTO prefixes(asn, prefix, family, synced_at)
             VALUES(?1, ?2, ?3, ?4)",
        )?;

        for prefix in prefixes {
            let Some(family) = prefix_family(&prefix) else {
                continue;
            };

            if family == 4 {
                ipv4_count += 1;
            } else {
                ipv6_count += 1;
            }

            stmt.execute(params![asn, prefix, family, synced_at])?;
        }
    }

    tx.execute(
        "UPDATE asn_status
         SET state = 'synced',
             ipv4_count = ?2,
             ipv6_count = ?3,
             last_synced_at = ?4,
             sync_started_at = NULL,
             provider_name = COALESCE(?5, provider_name),
             provider_updated_at = CASE WHEN ?5 IS NULL THEN provider_updated_at ELSE ?4 END,
             last_error = NULL,
             updated_at = ?4
         WHERE asn = ?1",
        params![asn, ipv4_count, ipv6_count, synced_at, provider_name],
    )?;

    tx.commit()?;
    Ok(())
}

pub fn mark_asn_error(conn: &Connection, asn: &str, error: &str) -> Result<()> {
    let now = now_rfc3339();
    conn.execute(
        "UPDATE asn_status
         SET state = 'error',
             sync_started_at = NULL,
             last_error = ?2,
             updated_at = ?3
         WHERE asn = ?1",
        params![asn, error, now],
    )?;
    Ok(())
}

pub fn read_configured_asn_status(conn: &Connection, asns: &[AsnConfig]) -> Result<Vec<AsnStatus>> {
    let mut result = Vec::with_capacity(asns.len());

    for config in asns {
        let status = conn
            .query_row(
                "SELECT state, ipv4_count, ipv6_count, last_synced_at, sync_started_at, last_error, provider_name
                 FROM asn_status
                 WHERE asn = ?1",
                params![config.asn],
                |row| {
                    Ok(AsnStatus {
                        asn: config.asn.clone(),
                        provider_name: row.get(6)?,
                        enabled: config.enabled,
                        target_interface: config.target_interface.clone(),
                        state: row.get(0)?,
                        ipv4_count: row.get(1)?,
                        ipv6_count: row.get(2)?,
                        last_synced_at: row.get(3)?,
                        sync_started_at: row.get(4)?,
                        last_error: row.get(5)?,
                    })
                },
            )
            .optional()?
            .unwrap_or_else(|| AsnStatus {
                asn: config.asn.clone(),
                provider_name: None,
                enabled: config.enabled,
                target_interface: config.target_interface.clone(),
                state: "not_synced".to_string(),
                ipv4_count: 0,
                ipv6_count: 0,
                last_synced_at: None,
                sync_started_at: None,
                last_error: None,
            });

        result.push(status);
    }

    Ok(result)
}

pub fn acquire_sync_lock(conn: &Connection, mode: &str) -> Result<bool> {
    cleanup_stale_lock(conn)?;

    let acquired = conn
        .execute(
            "INSERT INTO sync_lock(id, mode, locked_at) VALUES(1, ?1, ?2)",
            params![mode, now_unix()],
        )
        .is_ok();

    Ok(acquired)
}

pub fn release_sync_lock(conn: &Connection) {
    let _ = conn.execute("DELETE FROM sync_lock WHERE id = 1", []);
}

pub fn cleanup_stale_lock(conn: &Connection) -> Result<()> {
    conn.execute(
        "DELETE FROM sync_lock WHERE locked_at < ?1",
        params![now_unix() - LOCK_STALE_SECONDS],
    )?;
    Ok(())
}

pub fn read_sync_status(conn: &Connection) -> Result<SyncStatus> {
    let lock = conn
        .query_row(
            "SELECT mode, locked_at FROM sync_lock WHERE id = 1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;

    Ok(match lock {
        Some((mode, locked_at)) => SyncStatus {
            running: true,
            mode: Some(mode),
            locked_at: Some(locked_at),
        },
        None => SyncStatus {
            running: false,
            mode: None,
            locked_at: None,
        },
    })
}

pub fn read_policy_status(conn: &Connection, enabled: bool) -> Result<PolicyStatus> {
    let state = conn
        .query_row(
            "SELECT state, updated_at, interface_count, ipv4_prefix_count, last_error
             FROM policy_state
             WHERE id = 1",
            [],
            |row| {
                Ok(PolicyStatus {
                    enabled,
                    state: row.get(0)?,
                    updated_at: row.get(1)?,
                    interface_count: row.get(2)?,
                    ipv4_prefix_count: row.get(3)?,
                    last_error: row.get(4)?,
                    nft_path: ROUTES_NFT_PATH.to_string(),
                    script_path: ROUTES_SH_PATH.to_string(),
                })
            },
        )
        .optional()?;

    Ok(state.unwrap_or_else(|| PolicyStatus {
        enabled,
        state: if enabled { "not_applied" } else { "paused" }.to_string(),
        updated_at: None,
        interface_count: 0,
        ipv4_prefix_count: 0,
        last_error: None,
        nft_path: ROUTES_NFT_PATH.to_string(),
        script_path: ROUTES_SH_PATH.to_string(),
    }))
}

pub fn write_policy_state(
    conn: &Connection,
    state: &str,
    interface_count: i64,
    ipv4_prefix_count: i64,
    last_error: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO policy_state(
            id, state, updated_at, interface_count, ipv4_prefix_count, last_error
         )
         VALUES(1, ?1, ?2, ?3, ?4, ?5)",
        params![
            state,
            now_rfc3339(),
            interface_count,
            ipv4_prefix_count,
            last_error
        ],
    )?;

    Ok(())
}

pub fn read_ipv4_prefixes(conn: &Connection, asn: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT prefix
         FROM prefixes
         WHERE asn = ?1 AND family = 4
         ORDER BY prefix",
    )?;

    let rows = stmt.query_map(params![asn], |row| row.get::<_, String>(0))?;
    let mut prefixes = Vec::new();

    for row in rows {
        let prefix = row?;
        if is_valid_ipv4_prefix(&prefix) {
            prefixes.push(prefix);
        }
    }

    Ok(prefixes)
}

fn prefix_family(prefix: &str) -> Option<i64> {
    let address = prefix.split('/').next()?.parse::<IpAddr>().ok()?;

    Some(match address {
        IpAddr::V4(_) => 4,
        IpAddr::V6(_) => 6,
    })
}

fn is_valid_ipv4_prefix(prefix: &str) -> bool {
    let Some((address, mask)) = prefix.split_once('/') else {
        return false;
    };

    address.parse::<std::net::Ipv4Addr>().is_ok()
        && mask.parse::<u8>().map(|mask| mask <= 32).unwrap_or(false)
}
