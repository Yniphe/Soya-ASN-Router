use std::fs;
use std::net::IpAddr;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::config::{AsnConfig, InterfaceGroupConfig};
use crate::constants::{LOCK_STALE_SECONDS, ROUTES_NFT_PATH, ROUTES_SH_PATH};
use crate::types::{
    AsnStatus, InterfaceGroupStatus, InterfaceHealthStatus, PolicyStatus, SyncStatus,
};
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
            locked_at INTEGER NOT NULL,
            current_asn TEXT,
            total INTEGER NOT NULL DEFAULT 0,
            completed INTEGER NOT NULL DEFAULT 0,
            failed INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS policy_state (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            state TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            interface_count INTEGER NOT NULL DEFAULT 0,
            ipv4_prefix_count INTEGER NOT NULL DEFAULT 0,
            last_error TEXT
        );

        CREATE TABLE IF NOT EXISTS interface_group_state (
            group_id TEXT PRIMARY KEY,
            active_interface TEXT,
            state TEXT NOT NULL DEFAULT 'unknown',
            updated_at TEXT NOT NULL,
            switched_at TEXT,
            last_checked_at TEXT,
            last_error TEXT
        );

        CREATE TABLE IF NOT EXISTS interface_health (
            group_id TEXT NOT NULL,
            interface TEXT NOT NULL,
            state TEXT NOT NULL DEFAULT 'unknown',
            consecutive_successes INTEGER NOT NULL DEFAULT 0,
            consecutive_failures INTEGER NOT NULL DEFAULT 0,
            last_checked_at TEXT,
            last_ok_at TEXT,
            last_error TEXT,
            latency_ms INTEGER,
            PRIMARY KEY (group_id, interface)
        );
        ",
    )?;

    let _ = conn.execute("ALTER TABLE asn_status ADD COLUMN provider_name TEXT", []);
    let _ = conn.execute(
        "ALTER TABLE asn_status ADD COLUMN provider_updated_at TEXT",
        [],
    );
    let _ = conn.execute("ALTER TABLE sync_lock ADD COLUMN current_asn TEXT", []);
    let _ = conn.execute(
        "ALTER TABLE sync_lock ADD COLUMN total INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE sync_lock ADD COLUMN completed INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE sync_lock ADD COLUMN failed INTEGER NOT NULL DEFAULT 0",
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

#[derive(Debug, Clone)]
pub struct InterfaceHealthRow {
    pub interface: String,
    pub state: String,
    pub consecutive_successes: i64,
    pub consecutive_failures: i64,
    pub last_checked_at: Option<String>,
    pub last_ok_at: Option<String>,
    pub last_error: Option<String>,
    pub latency_ms: Option<i64>,
}

pub fn ensure_interface_group_state(conn: &Connection, group: &InterfaceGroupConfig) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO interface_group_state(
            group_id, active_interface, state, updated_at
         )
         VALUES(?1, ?2, 'unknown', ?3)",
        params![group.id, group.primary_interface(), now_rfc3339()],
    )?;
    Ok(())
}

pub fn read_group_active_interface(
    conn: &Connection,
    group: &InterfaceGroupConfig,
) -> Result<Option<String>> {
    ensure_interface_group_state(conn, group)?;

    let active = conn
        .query_row(
            "SELECT active_interface FROM interface_group_state WHERE group_id = ?1",
            params![group.id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten()
        .filter(|interface| group.interfaces.iter().any(|item| item == interface));

    Ok(active.or_else(|| group.primary_interface().map(ToOwned::to_owned)))
}

pub fn write_group_state(
    conn: &Connection,
    group_id: &str,
    active_interface: Option<&str>,
    state: &str,
    switched: bool,
    last_error: Option<&str>,
) -> Result<()> {
    let now = now_rfc3339();
    conn.execute(
        "INSERT INTO interface_group_state(
            group_id, active_interface, state, updated_at, switched_at, last_checked_at, last_error
         )
         VALUES(?1, ?2, ?3, ?4, CASE WHEN ?5 THEN ?4 ELSE NULL END, ?4, ?6)
         ON CONFLICT(group_id) DO UPDATE SET
            active_interface = excluded.active_interface,
            state = excluded.state,
            updated_at = excluded.updated_at,
            switched_at = CASE
                WHEN ?5 THEN excluded.updated_at
                ELSE interface_group_state.switched_at
            END,
            last_checked_at = excluded.last_checked_at,
            last_error = excluded.last_error",
        params![group_id, active_interface, state, now, switched, last_error],
    )?;
    Ok(())
}

pub fn write_interface_health(
    conn: &Connection,
    group_id: &str,
    interface: &str,
    ok: bool,
    latency_ms: Option<i64>,
    error: Option<&str>,
) -> Result<InterfaceHealthRow> {
    let previous = read_interface_health(conn, group_id, interface)?;
    let successes = if ok {
        previous
            .as_ref()
            .map(|row| row.consecutive_successes + 1)
            .unwrap_or(1)
    } else {
        0
    };
    let failures = if ok {
        0
    } else {
        previous
            .as_ref()
            .map(|row| row.consecutive_failures + 1)
            .unwrap_or(1)
    };
    let state = if ok { "healthy" } else { "unhealthy" };
    let now = now_rfc3339();

    conn.execute(
        "INSERT INTO interface_health(
            group_id, interface, state, consecutive_successes, consecutive_failures,
            last_checked_at, last_ok_at, last_error, latency_ms
         )
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, CASE WHEN ?7 THEN ?6 ELSE NULL END, ?8, ?9)
         ON CONFLICT(group_id, interface) DO UPDATE SET
            state = excluded.state,
            consecutive_successes = excluded.consecutive_successes,
            consecutive_failures = excluded.consecutive_failures,
            last_checked_at = excluded.last_checked_at,
            last_ok_at = CASE
                WHEN ?7 THEN excluded.last_ok_at
                ELSE interface_health.last_ok_at
            END,
            last_error = excluded.last_error,
            latency_ms = excluded.latency_ms",
        params![group_id, interface, state, successes, failures, now, ok, error, latency_ms],
    )?;

    Ok(InterfaceHealthRow {
        interface: interface.to_string(),
        state: state.to_string(),
        consecutive_successes: successes,
        consecutive_failures: failures,
        last_checked_at: Some(now.clone()),
        last_ok_at: if ok {
            Some(now)
        } else {
            previous.and_then(|row| row.last_ok_at)
        },
        last_error: error.map(ToOwned::to_owned),
        latency_ms,
    })
}

pub fn read_interface_health(
    conn: &Connection,
    group_id: &str,
    interface: &str,
) -> Result<Option<InterfaceHealthRow>> {
    conn.query_row(
        "SELECT interface, state, consecutive_successes, consecutive_failures,
                last_checked_at, last_ok_at, last_error, latency_ms
         FROM interface_health
         WHERE group_id = ?1 AND interface = ?2",
        params![group_id, interface],
        read_health_row,
    )
    .optional()
    .map_err(Into::into)
}

pub fn read_interface_group_statuses(
    conn: &Connection,
    groups: &[InterfaceGroupConfig],
) -> Result<Vec<InterfaceGroupStatus>> {
    let mut result = Vec::with_capacity(groups.len());

    for group in groups {
        ensure_interface_group_state(conn, group)?;
        let state = conn
            .query_row(
                "SELECT active_interface, state, switched_at, last_checked_at, last_error
                 FROM interface_group_state
                 WHERE group_id = ?1",
                params![group.id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .optional()?;

        let (active_interface, state, switched_at, last_checked_at, last_error) = state
            .unwrap_or_else(|| {
                (
                    group.primary_interface().map(ToOwned::to_owned),
                    "unknown".to_string(),
                    None,
                    None,
                    None,
                )
            });
        let mut interfaces = Vec::with_capacity(group.interfaces.len());

        for interface in &group.interfaces {
            let health = read_interface_health(conn, &group.id, interface)?;
            interfaces.push(match health {
                Some(row) => InterfaceHealthStatus {
                    name: row.interface,
                    state: row.state,
                    consecutive_successes: row.consecutive_successes,
                    consecutive_failures: row.consecutive_failures,
                    last_checked_at: row.last_checked_at,
                    last_ok_at: row.last_ok_at,
                    last_error: row.last_error,
                    latency_ms: row.latency_ms,
                },
                None => InterfaceHealthStatus {
                    name: interface.clone(),
                    state: "unknown".to_string(),
                    consecutive_successes: 0,
                    consecutive_failures: 0,
                    last_checked_at: None,
                    last_ok_at: None,
                    last_error: None,
                    latency_ms: None,
                },
            });
        }

        result.push(InterfaceGroupStatus {
            id: group.id.clone(),
            name: group.name.clone(),
            enabled: group.enabled,
            check_enabled: group.check_enabled,
            check_url: group.check_url.clone(),
            check_interval_seconds: group.check_interval_seconds,
            check_timeout_seconds: group.check_timeout_seconds,
            failure_threshold: group.failure_threshold,
            recovery_threshold: group.recovery_threshold,
            prefer_primary: group.prefer_primary,
            primary_interface: group.primary_interface().map(ToOwned::to_owned),
            active_interface,
            state,
            switched_at,
            last_checked_at,
            last_error,
            interfaces,
        });
    }

    Ok(result)
}

fn read_health_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<InterfaceHealthRow> {
    Ok(InterfaceHealthRow {
        interface: row.get(0)?,
        state: row.get(1)?,
        consecutive_successes: row.get(2)?,
        consecutive_failures: row.get(3)?,
        last_checked_at: row.get(4)?,
        last_ok_at: row.get(5)?,
        last_error: row.get(6)?,
        latency_ms: row.get(7)?,
    })
}

pub fn acquire_sync_lock(conn: &Connection, mode: &str) -> Result<bool> {
    cleanup_stale_lock(conn)?;

    let acquired = conn
        .execute(
            "INSERT INTO sync_lock(id, mode, locked_at, current_asn, total, completed, failed)
             VALUES(1, ?1, ?2, NULL, 0, 0, 0)",
            params![mode, now_unix()],
        )
        .is_ok();

    Ok(acquired)
}

pub fn update_sync_progress(
    conn: &Connection,
    total: i64,
    completed: i64,
    failed: i64,
    current_asn: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE sync_lock
         SET total = ?1, completed = ?2, failed = ?3, current_asn = ?4
         WHERE id = 1",
        params![total, completed, failed, current_asn],
    )?;
    Ok(())
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
            "SELECT mode, locked_at, current_asn, total, completed, failed
             FROM sync_lock
             WHERE id = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .optional()?;

    Ok(match lock {
        Some((mode, locked_at, current_asn, total, completed, failed)) => SyncStatus {
            running: true,
            mode: Some(mode),
            locked_at: Some(locked_at),
            current_asn,
            total,
            completed,
            failed,
        },
        None => SyncStatus {
            running: false,
            mode: None,
            locked_at: None,
            current_asn: None,
            total: 0,
            completed: 0,
            failed: 0,
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
