use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use serde::Serialize;

use crate::constants::DEFAULT_DB_PATH;
use crate::util::run_status_command;

#[derive(Debug, Clone)]
pub struct Config {
    pub enabled: bool,
    pub asns: Vec<AsnConfig>,
    pub lan_interface: String,
    pub default_target_interface: String,
    pub auto_apply_routes: bool,
    pub periodic_sync_enabled: bool,
    pub periodic_sync_mode: String,
    pub periodic_sync_interval_minutes: u64,
    pub route_policy_enabled: bool,
    pub proxy_enabled: bool,
    pub proxy_type: String,
    pub proxy_url: Option<String>,
    pub db_path: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct AsnConfig {
    pub asn: String,
    pub target_interface: String,
    pub enabled: bool,
}

#[derive(Debug, Serialize)]
pub struct AsnDedupeResult {
    pub kept: usize,
    pub removed_duplicates: usize,
    pub removed_invalid: usize,
}

#[derive(Debug, Serialize)]
pub struct AsnImportResult {
    pub imported: usize,
    pub skipped_existing: usize,
    pub skipped_duplicate: usize,
    pub invalid: usize,
    pub deduplicated: usize,
    pub interface: String,
    pub asns: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct AsnBulkDeleteResult {
    pub requested: usize,
    pub deleted: usize,
    pub missing: usize,
}

#[derive(Debug, Serialize)]
pub struct AsnBulkSetInterfaceResult {
    pub requested: usize,
    pub updated: usize,
    pub missing: usize,
    pub interface: String,
}

impl Config {
    pub fn load() -> Self {
        let enabled =
            parse_bool(uci_get("soya-asn-router.main.enabled").as_deref()).unwrap_or(false);
        let default_target_interface =
            uci_get("soya-asn-router.main.default_interface").unwrap_or_else(|| "wan".to_string());
        let lan_interface =
            uci_get("soya-asn-router.main.lan_interface").unwrap_or_else(|| "lan".to_string());
        let auto_apply_routes =
            parse_bool(uci_get("soya-asn-router.main.auto_apply_routes").as_deref())
                .unwrap_or(true);
        let periodic_sync_enabled =
            parse_bool(uci_get("soya-asn-router.main.periodic_sync_enabled").as_deref())
                .unwrap_or(false);
        let periodic_sync_mode =
            normalize_sync_mode(uci_get("soya-asn-router.main.periodic_sync_mode").as_deref());
        let periodic_sync_interval_minutes = parse_interval_minutes(uci_get(
            "soya-asn-router.main.periodic_sync_interval_minutes",
        ));
        let route_policy_enabled = env::var("SOYA_ASN_ROUTER_ROUTE_POLICY_ENABLED")
            .ok()
            .and_then(|value| parse_bool(Some(&value)))
            .or_else(|| parse_bool(uci_get("soya-asn-router.main.route_policy_enabled").as_deref()))
            .unwrap_or(true);

        let mut asns = load_asn_sections(&default_target_interface);

        if asns.is_empty() {
            asns = uci_get("soya-asn-router.main.asn")
                .unwrap_or_default()
                .split_whitespace()
                .filter_map(|asn| {
                    Some(AsnConfig {
                        asn: normalize_asn(asn)?,
                        target_interface: default_target_interface.clone(),
                        enabled: true,
                    })
                })
                .collect::<Vec<_>>();
        }

        if let Ok(value) = env::var("SOYA_ASN_ROUTER_ASNS") {
            asns = value
                .split(|ch: char| ch == ',' || ch.is_ascii_whitespace())
                .filter_map(|asn| {
                    Some(AsnConfig {
                        asn: normalize_asn(asn)?,
                        target_interface: default_target_interface.clone(),
                        enabled: true,
                    })
                })
                .collect();
        }

        dedup_asn_configs(&mut asns);

        let proxy_enabled =
            parse_bool(uci_get("soya-asn-router.main.proxy_enabled").as_deref()).unwrap_or(false);
        let proxy_type =
            uci_get("soya-asn-router.main.proxy_type").unwrap_or_else(|| "http".to_string());
        let proxy_url =
            uci_get("soya-asn-router.main.proxy_url").filter(|value| !value.trim().is_empty());

        let db_path = env::var("SOYA_ASN_ROUTER_DB_PATH")
            .ok()
            .or_else(|| uci_get("soya-asn-router.main.db_path"))
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_DB_PATH.to_string());

        Self {
            enabled,
            asns,
            lan_interface,
            default_target_interface,
            auto_apply_routes,
            periodic_sync_enabled,
            periodic_sync_mode,
            periodic_sync_interval_minutes,
            route_policy_enabled,
            proxy_enabled,
            proxy_type,
            proxy_url,
            db_path: PathBuf::from(db_path),
        }
    }

    pub fn active_asns(&self) -> Vec<&AsnConfig> {
        self.asns.iter().filter(|asn| asn.enabled).collect()
    }
}

#[derive(Default, Clone)]
struct UciSection {
    section_type: Option<String>,
    options: BTreeMap<String, String>,
}

fn load_asn_sections(default_target_interface: &str) -> Vec<AsnConfig> {
    let sections = uci_show("soya-asn-router")
        .map(|output| parse_uci_sections("soya-asn-router", &output))
        .unwrap_or_default();

    sections
        .into_iter()
        .filter(|section| section.section_type.as_deref() == Some("asn"))
        .filter_map(|section| {
            Some(AsnConfig {
                asn: normalize_asn(section.options.get("asn")?)?,
                target_interface: normalize_interface_value(
                    section.options.get("interface").map(String::as_str),
                    default_target_interface,
                ),
                enabled: parse_bool(section.options.get("enabled").map(String::as_str))
                    .unwrap_or(true),
            })
        })
        .collect()
}

fn dedup_asn_configs(asns: &mut Vec<AsnConfig>) {
    let mut unique = BTreeMap::<String, AsnConfig>::new();

    for config in asns.drain(..) {
        let Some(asn) = normalize_asn(&config.asn) else {
            continue;
        };
        let target_interface = normalize_interface_value(Some(&config.target_interface), "wan");

        unique.entry(asn.clone()).or_insert(AsnConfig {
            asn,
            target_interface,
            enabled: config.enabled,
        });
    }

    *asns = unique.into_values().collect();
}

fn normalize_interface_value(value: Option<&str>, fallback: &str) -> String {
    let value = value.unwrap_or(fallback).trim();
    if value.is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

fn decode_uci_value(value: &str) -> String {
    let value = value.trim();

    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        return value[1..value.len() - 1].replace("'\\''", "'");
    }

    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        return value[1..value.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\");
    }

    value.to_string()
}

pub fn normalize_asn(value: &str) -> Option<String> {
    let value = value.trim().to_ascii_uppercase();
    let number = value.strip_prefix("AS").unwrap_or(&value);

    if number.is_empty() || !number.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }

    let number = number.parse::<u32>().ok()?;
    if number == 0 {
        return None;
    }

    Some(format!("AS{number}"))
}

pub fn dedupe_asn_config() -> Result<AsnDedupeResult> {
    let sections = read_committed_sections()?;
    let mut seen = BTreeSet::<String>::new();
    let mut kept = Vec::new();
    let mut removed_duplicates = 0;
    let mut removed_invalid = 0;

    for mut section in asn_sections(sections) {
        let Some(asn) = section
            .options
            .get("asn")
            .and_then(|value| normalize_asn(value))
        else {
            removed_invalid += 1;
            continue;
        };

        if !seen.insert(asn.clone()) {
            removed_duplicates += 1;
            continue;
        }

        let target_interface =
            normalize_interface_value(section.options.get("interface").map(String::as_str), "wan");
        section.options.insert("asn".to_string(), asn);
        section
            .options
            .insert("interface".to_string(), target_interface);
        section
            .options
            .entry("enabled".to_string())
            .or_insert_with(|| "1".to_string());
        kept.push(section);
    }

    if removed_duplicates > 0 || removed_invalid > 0 {
        rewrite_asn_sections(&kept)?;
    }

    Ok(AsnDedupeResult {
        kept: kept.len(),
        removed_duplicates,
        removed_invalid,
    })
}

pub fn import_asns_from_text(text: &str, target_interface: &str) -> Result<AsnImportResult> {
    let target_interface = normalize_interface_value(Some(target_interface), "");
    if target_interface.is_empty() {
        return Err(anyhow!("target interface is required"));
    }

    let mut seen_input = BTreeSet::<String>::new();
    let mut parsed_asns = Vec::new();
    let mut skipped_duplicate = 0;
    let mut invalid = 0;

    for token in text.split(|ch: char| ch == ',' || ch == ';' || ch.is_ascii_whitespace()) {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }

        match normalize_asn(token) {
            Some(asn) if seen_input.insert(asn.clone()) => parsed_asns.push(asn),
            Some(_) => skipped_duplicate += 1,
            None => invalid += 1,
        }
    }

    if parsed_asns.is_empty() {
        return Err(anyhow!("import data does not contain valid ASN entries"));
    }

    let dedupe = dedupe_asn_config()?;
    let mut sections = asn_sections(read_committed_sections()?);
    let mut existing = BTreeSet::<String>::new();

    for section in &sections {
        if let Some(asn) = section
            .options
            .get("asn")
            .and_then(|value| normalize_asn(value))
        {
            existing.insert(asn);
        }
    }

    let mut imported_asns = Vec::new();
    let mut skipped_existing = 0;

    for asn in parsed_asns {
        if !existing.insert(asn.clone()) {
            skipped_existing += 1;
            continue;
        }

        let mut options = BTreeMap::new();
        options.insert("asn".to_string(), asn.clone());
        options.insert("interface".to_string(), target_interface.clone());
        options.insert("enabled".to_string(), "1".to_string());

        sections.push(UciSection {
            section_type: Some("asn".to_string()),
            options,
        });
        imported_asns.push(asn);
    }

    if !imported_asns.is_empty() {
        rewrite_asn_sections(&sections)?;
    }

    Ok(AsnImportResult {
        imported: imported_asns.len(),
        skipped_existing,
        skipped_duplicate,
        invalid,
        deduplicated: dedupe.removed_duplicates + dedupe.removed_invalid,
        interface: target_interface,
        asns: imported_asns,
    })
}

pub fn bulk_delete_asns(text: &str) -> Result<AsnBulkDeleteResult> {
    let requested = parse_asn_set(text);
    if requested.is_empty() {
        return Err(anyhow!("no valid ASN entries were provided"));
    }

    let mut sections = asn_sections(read_committed_sections()?);
    let requested_count = requested.len();
    let before = sections.len();

    sections.retain(|section| {
        section
            .options
            .get("asn")
            .and_then(|value| normalize_asn(value))
            .map(|asn| !requested.contains(&asn))
            .unwrap_or(true)
    });

    let deleted = before.saturating_sub(sections.len());
    if deleted > 0 {
        rewrite_asn_sections(&sections)?;
    }

    Ok(AsnBulkDeleteResult {
        requested: requested_count,
        deleted,
        missing: requested_count.saturating_sub(deleted),
    })
}

pub fn bulk_set_asn_interface(
    text: &str,
    target_interface: &str,
) -> Result<AsnBulkSetInterfaceResult> {
    let requested = parse_asn_set(text);
    if requested.is_empty() {
        return Err(anyhow!("no valid ASN entries were provided"));
    }

    let target_interface = normalize_interface_value(Some(target_interface), "");
    if target_interface.is_empty() {
        return Err(anyhow!("target interface is required"));
    }

    let mut sections = asn_sections(read_committed_sections()?);
    let mut updated = 0;

    for section in &mut sections {
        let Some(asn) = section
            .options
            .get("asn")
            .and_then(|value| normalize_asn(value))
        else {
            continue;
        };

        if requested.contains(&asn) {
            section
                .options
                .insert("interface".to_string(), target_interface.clone());
            updated += 1;
        }
    }

    if updated > 0 {
        rewrite_asn_sections(&sections)?;
    }

    Ok(AsnBulkSetInterfaceResult {
        requested: requested.len(),
        updated,
        missing: requested.len().saturating_sub(updated),
        interface: target_interface,
    })
}

fn parse_asn_set(text: &str) -> BTreeSet<String> {
    text.split(|ch: char| ch == ',' || ch == ';' || ch.is_ascii_whitespace())
        .filter_map(normalize_asn)
        .collect()
}

fn asn_sections(sections: Vec<UciSection>) -> Vec<UciSection> {
    sections
        .into_iter()
        .filter(|section| section.section_type.as_deref() == Some("asn"))
        .collect()
}

fn read_committed_sections() -> Result<Vec<UciSection>> {
    let output = run_uci_output(
        &["-q", "show", "soya-asn-router"],
        "failed to read soya-asn-router UCI config",
    )?;

    Ok(parse_uci_sections("soya-asn-router", &output))
}

fn parse_uci_sections(package: &str, output: &str) -> Vec<UciSection> {
    let prefix = format!("{package}.");
    let mut sections = Vec::<UciSection>::new();
    let mut indexes = BTreeMap::<String, usize>::new();

    for line in output.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let Some(rest) = key.strip_prefix(&prefix) else {
            continue;
        };

        let value = decode_uci_value(value);
        let (section_name, option_name) = match rest.split_once('.') {
            Some((section, option)) => (section, Some(option)),
            None => (rest, None),
        };
        let index = if let Some(index) = indexes.get(section_name) {
            *index
        } else {
            let index = sections.len();
            indexes.insert(section_name.to_string(), index);
            sections.push(UciSection::default());
            index
        };

        if let Some(option_name) = option_name {
            sections[index]
                .options
                .insert(option_name.to_string(), value);
        } else {
            sections[index].section_type = Some(value);
        }
    }

    sections
}

fn rewrite_asn_sections(sections: &[UciSection]) -> Result<()> {
    delete_all_asn_sections()?;

    for section in sections {
        let new_section = add_asn_section()?;

        for (option, value) in &section.options {
            set_uci_option(&new_section, option, value)?;
        }
    }

    run_status_command(
        Command::new("uci").args(["commit", "soya-asn-router"]),
        "failed to commit soya-asn-router UCI config",
    )
}

fn delete_all_asn_sections() -> Result<()> {
    loop {
        let status = Command::new("uci")
            .args(["-q", "delete", "soya-asn-router.@asn[0]"])
            .status()
            .context("failed to delete ASN UCI section")?;

        if !status.success() {
            return Ok(());
        }
    }
}

fn add_asn_section() -> Result<String> {
    let section = run_uci_output(
        &["add", "soya-asn-router", "asn"],
        "failed to add ASN UCI section",
    )?
    .trim()
    .to_string();

    if section.is_empty() {
        return Err(anyhow!("uci add did not return a section name"));
    }

    Ok(section)
}

fn set_uci_option(section: &str, option: &str, value: &str) -> Result<()> {
    let assignment = format!("soya-asn-router.{section}.{option}={value}");

    run_status_command(
        Command::new("uci").args(["set", &assignment]),
        "failed to set ASN UCI option",
    )
}

fn run_uci_output(args: &[&str], context: &str) -> Result<String> {
    let output = Command::new("uci")
        .args(args)
        .output()
        .with_context(|| context.to_string())?;

    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(anyhow!(
        "{}{}",
        context,
        if stderr.is_empty() {
            String::new()
        } else {
            format!(": {stderr}")
        }
    ))
}

fn parse_bool(value: Option<&str>) -> Option<bool> {
    match value?.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" | "enabled" => Some(true),
        "0" | "false" | "no" | "off" | "disabled" => Some(false),
        _ => None,
    }
}

fn normalize_sync_mode(value: Option<&str>) -> String {
    match value.map(str::trim) {
        Some("missing") => "missing".to_string(),
        _ => "all".to_string(),
    }
}

fn parse_interval_minutes(value: Option<String>) -> u64 {
    value
        .as_deref()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(1440)
}

fn uci_get(path: &str) -> Option<String> {
    let output = Command::new("uci")
        .args(["-q", "get", path])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn uci_show(package: &str) -> Option<String> {
    let output = Command::new("uci")
        .args(["-q", "show", package])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8_lossy(&output.stdout).to_string();
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

pub fn set_route_policy_enabled(enabled: bool) -> Result<()> {
    let value = if enabled { "1" } else { "0" };

    run_status_command(
        Command::new("uci").args([
            "set",
            &format!("soya-asn-router.main.route_policy_enabled={value}"),
        ]),
        "failed to update route policy UCI setting",
    )?;
    run_status_command(
        Command::new("uci").args(["commit", "soya-asn-router"]),
        "failed to commit route policy UCI setting",
    )
}
