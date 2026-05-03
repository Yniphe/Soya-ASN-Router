mod config;
mod constants;
mod db;
mod policy;
mod ripe;
mod types;
mod util;

use std::env;
use std::io::Read;
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use reqwest::blocking::Client;
use reqwest::Url;
use rusqlite::{params, Connection};

use crate::config::{dedupe_asn_config, import_asns_from_text, set_route_policy_enabled, Config};
use crate::constants::{ROUTE_START_RETRIES, ROUTE_START_RETRY_DELAY_SECONDS};
use crate::db::{
    acquire_sync_lock, cleanup_stale_lock, ensure_asn_row, has_successful_sync, mark_asn_error,
    open_database, read_configured_asn_status, read_policy_status, read_sync_status,
    release_sync_lock, save_prefixes, write_policy_state,
};
use crate::policy::{
    apply_routes, disable_routes, discover_interfaces, generate_route_files,
    reconcile_route_policy, PolicyStats,
};
use crate::ripe::{build_http_client, fetch_asn_provider, fetch_prefixes};
use crate::types::{InterfaceChoice, InterfaceListResponse, ProxyStatus, StatusResponse};
use crate::util::{now_rfc3339, truncate_error};

const MAX_IMPORT_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, Copy)]
enum SyncMode {
    Missing,
    All,
}

impl SyncMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::All => "all",
        }
    }
}

fn main() -> ExitCode {
    match real_main() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("soya-asn-router: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn real_main() -> Result<()> {
    let mut args = env::args().skip(1);

    match args.next().as_deref() {
        None | Some("daemon") => run_daemon(),
        Some("status") => print_status(),
        Some("sync-missing") => run_sync(SyncMode::Missing),
        Some("sync-all") => run_sync(SyncMode::All),
        Some("apply-routes") => run_apply_routes(true),
        Some("generate-routes") => run_apply_routes(false),
        Some("pause-routes") => run_pause_routes(),
        Some("resume-routes") => run_resume_routes(),
        Some("interfaces") => print_interfaces(),
        Some("dedupe-config") => run_dedupe_config(),
        Some("import-url") => {
            let url = args
                .next()
                .ok_or_else(|| anyhow!("import-url requires an HTTP or HTTPS URL"))?;
            let target_interface = args
                .next()
                .ok_or_else(|| anyhow!("import-url requires a target interface"))?;
            run_import_url(&url, &target_interface)
        }
        Some("--help") | Some("-h") => {
            print_usage();
            Ok(())
        }
        Some(command) => Err(anyhow!("unknown command: {command}")),
    }
}

fn print_usage() {
    println!(
        "Usage: soya-asn-router [daemon|status|sync-missing|sync-all|apply-routes|generate-routes|pause-routes|resume-routes|interfaces|dedupe-config|import-url URL INTERFACE]"
    );
}

fn run_daemon() -> Result<()> {
    let config = Config::load();
    let conn = open_database(&config.db_path)?;

    for attempt in 1..=ROUTE_START_RETRIES {
        match reconcile_route_policy(&conn, &config) {
            Ok(()) => break,
            Err(error) => {
                eprintln!(
                    "soya-asn-router: route policy startup attempt {attempt}/{ROUTE_START_RETRIES} failed: {error:#}"
                );

                if attempt == ROUTE_START_RETRIES {
                    break;
                }

                thread::sleep(Duration::from_secs(ROUTE_START_RETRY_DELAY_SECONDS));
            }
        }
    }

    println!(
        "soya-asn-router: backend ready; database={}",
        config.db_path.display()
    );

    loop {
        thread::sleep(Duration::from_secs(3600));
    }
}

fn print_status() -> Result<()> {
    let config = Config::load();
    let conn = open_database(&config.db_path)?;
    cleanup_stale_lock(&conn)?;

    let response = StatusResponse {
        enabled: config.enabled,
        db_path: config.db_path.display().to_string(),
        lan_interface: config.lan_interface.clone(),
        default_target_interface: config.default_target_interface.clone(),
        proxy: ProxyStatus {
            enabled: config.proxy_enabled,
            proxy_type: config.proxy_type.clone(),
            url: config.proxy_url.clone(),
        },
        sync: read_sync_status(&conn)?,
        policy: read_policy_status(&conn, config.route_policy_enabled)?,
        asns: read_configured_asn_status(&conn, &config.asns)?,
    };

    serde_json::to_writer(std::io::stdout(), &response)?;
    println!();
    Ok(())
}

fn run_dedupe_config() -> Result<()> {
    let result = dedupe_asn_config()?;

    serde_json::to_writer(std::io::stdout(), &result)?;
    println!();
    Ok(())
}

fn run_import_url(url: &str, target_interface: &str) -> Result<()> {
    let url = Url::parse(url).context("invalid import URL")?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(anyhow!("import URL must use HTTP or HTTPS"));
    }

    let config = Config::load();
    let client = build_http_client(&config)?;
    let response = client
        .get(url)
        .send()
        .context("failed to fetch ASN import URL")?
        .error_for_status()
        .context("ASN import URL returned HTTP error")?;

    let mut body = String::new();
    response
        .take(MAX_IMPORT_BYTES + 1)
        .read_to_string(&mut body)
        .context("failed to read ASN import response")?;

    if body.len() as u64 > MAX_IMPORT_BYTES {
        return Err(anyhow!(
            "ASN import response is too large; maximum is {} bytes",
            MAX_IMPORT_BYTES
        ));
    }

    let result = import_asns_from_text(&body, target_interface)?;
    serde_json::to_writer(std::io::stdout(), &result)?;
    println!();
    Ok(())
}

fn print_interfaces() -> Result<()> {
    let response = InterfaceListResponse {
        interfaces: discover_interfaces()?
            .into_iter()
            .map(|iface| InterfaceChoice {
                device: iface.device_name(),
                name: iface.interface,
                up: iface.up,
            })
            .collect(),
    };

    serde_json::to_writer(std::io::stdout(), &response)?;
    println!();
    Ok(())
}

fn run_apply_routes(apply: bool) -> Result<()> {
    let config = Config::load();
    let mut conn = open_database(&config.db_path)?;

    let result = if apply {
        set_route_policy_enabled(true)?;
        apply_routes(&mut conn, &config)
    } else {
        generate_route_files(&conn, &config)
    };

    match result {
        Ok(stats) => {
            if !apply {
                write_policy_state(
                    &conn,
                    "generated",
                    stats.interface_count,
                    stats.ipv4_prefix_count,
                    stats.warning.as_deref(),
                )?;
            }
            print_status()
        }
        Err(error) => {
            if !apply {
                let message = truncate_error(&format!("{error:#}"));
                write_policy_state(&conn, "error", 0, 0, Some(&message))?;
            }
            Err(error)
        }
    }
}

fn run_pause_routes() -> Result<()> {
    let config = Config::load();
    let conn = open_database(&config.db_path)?;

    set_route_policy_enabled(false)?;
    disable_routes(&conn)?;
    print_status()
}

fn run_resume_routes() -> Result<()> {
    let config = Config::load();
    let conn = open_database(&config.db_path)?;

    set_route_policy_enabled(true)?;
    apply_routes(&conn, &config)?;
    print_status()
}

fn run_sync(mode: SyncMode) -> Result<()> {
    let config = Config::load();
    let mut conn = open_database(&config.db_path)?;

    if !acquire_sync_lock(&conn, mode.as_str())? {
        println!("soya-asn-router: sync already running");
        return Ok(());
    }

    let sync_result = run_sync_locked(&mut conn, &config, mode);
    let apply_result = if config.auto_apply_routes && config.route_policy_enabled {
        apply_routes(&mut conn, &config)
    } else {
        Ok(PolicyStats {
            interface_count: 0,
            ipv4_prefix_count: 0,
            warning: None,
        })
    };
    release_sync_lock(&conn);

    sync_result?;
    apply_result.map(|_| ())
}

fn run_sync_locked(conn: &mut Connection, config: &Config, mode: SyncMode) -> Result<()> {
    let active_asns = config.active_asns();

    for asn in &active_asns {
        ensure_asn_row(conn, &asn.asn)?;
    }

    let asns = match mode {
        SyncMode::Missing => active_asns
            .iter()
            .filter_map(|asn| match has_successful_sync(conn, &asn.asn) {
                Ok(false) => Some(Ok(asn.asn.clone())),
                Ok(true) => None,
                Err(error) => Some(Err(error)),
            })
            .collect::<Result<Vec<_>>>()?,
        SyncMode::All => active_asns.iter().map(|asn| asn.asn.clone()).collect(),
    };

    let now = now_rfc3339();
    for asn in &asns {
        conn.execute(
            "UPDATE asn_status
             SET state = 'queued', last_error = NULL, updated_at = ?2
             WHERE asn = ?1",
            params![asn, now],
        )?;
    }

    if asns.is_empty() {
        return Ok(());
    }

    let client = build_http_client(config)?;

    for asn in asns {
        if let Err(error) = sync_one_asn(conn, &client, &asn) {
            let message = truncate_error(&format!("{error:#}"));
            mark_asn_error(conn, &asn, &message)?;
        }
    }

    Ok(())
}

fn sync_one_asn(conn: &mut Connection, client: &Client, asn: &str) -> Result<()> {
    let started_at = now_rfc3339();
    conn.execute(
        "UPDATE asn_status
         SET state = 'syncing', sync_started_at = ?2, last_error = NULL, updated_at = ?2
         WHERE asn = ?1",
        params![asn, started_at],
    )?;

    let prefixes = fetch_prefixes(client, asn)?;
    let provider_name = match fetch_asn_provider(client, asn) {
        Ok(provider_name) => provider_name,
        Err(error) => {
            eprintln!("soya-asn-router: failed to fetch provider name for {asn}: {error:#}");
            None
        }
    };

    save_prefixes(conn, asn, prefixes, provider_name)
}
