use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use serde::Serialize;

use crate::constants::{
    DEFAULT_DB_PATH, DEFAULT_GROUP_CHECK_INTERVAL_SECONDS, DEFAULT_GROUP_CHECK_TIMEOUT_SECONDS,
    DEFAULT_GROUP_CHECK_URL, DEFAULT_GROUP_FAILURE_THRESHOLD, DEFAULT_GROUP_RECOVERY_THRESHOLD,
};
use crate::util::run_status_command;

#[derive(Debug, Clone)]
pub struct Config {
    pub enabled: bool,
    pub asns: Vec<AsnConfig>,
    pub custom_routes: Vec<CustomRouteConfig>,
    pub interface_groups: Vec<InterfaceGroupConfig>,
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

#[derive(Debug, Clone, Serialize)]
pub struct CustomRouteConfig {
    pub name: Option<String>,
    pub destination: String,
    pub target_interface: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct InterfaceGroupConfig {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub interfaces: Vec<String>,
    pub check_enabled: bool,
    pub check_url: String,
    pub check_interval_seconds: u64,
    pub check_timeout_seconds: u64,
    pub failure_threshold: u32,
    pub recovery_threshold: u32,
    pub prefer_primary: bool,
}

impl InterfaceGroupConfig {
    pub fn primary_interface(&self) -> Option<&str> {
        self.interfaces.first().map(String::as_str)
    }
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

        let sections = uci_show("soya-asn-router")
            .map(|output| parse_uci_sections("soya-asn-router", &output))
            .unwrap_or_default();
        let mut asns = load_asn_sections(&sections, &default_target_interface);
        let custom_routes = load_custom_route_sections(&sections, &default_target_interface);
        let interface_groups = load_interface_group_sections(&sections);

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
            custom_routes,
            interface_groups,
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

    pub fn active_custom_routes(&self) -> Vec<&CustomRouteConfig> {
        self.custom_routes
            .iter()
            .filter(|route| route.enabled)
            .collect()
    }

    pub fn active_interface_groups(&self) -> Vec<&InterfaceGroupConfig> {
        self.interface_groups
            .iter()
            .filter(|group| group.enabled && !group.interfaces.is_empty())
            .collect()
    }

    pub fn find_interface_group(&self, id: &str) -> Option<&InterfaceGroupConfig> {
        self.interface_groups
            .iter()
            .find(|group| group.id == id && group.enabled)
    }
}

#[derive(Default, Clone)]
struct UciSection {
    name: String,
    section_type: Option<String>,
    options: BTreeMap<String, String>,
    lists: BTreeMap<String, Vec<String>>,
}

fn load_asn_sections(sections: &[UciSection], default_target_interface: &str) -> Vec<AsnConfig> {
    sections
        .iter()
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

fn load_interface_group_sections(sections: &[UciSection]) -> Vec<InterfaceGroupConfig> {
    sections
        .iter()
        .filter(|section| section.section_type.as_deref() == Some("interface_group"))
        .filter_map(|section| {
            let id = normalize_group_id(
                section
                    .options
                    .get("id")
                    .map(String::as_str)
                    .unwrap_or(&section.name),
            )?;
            let name = section
                .options
                .get("name")
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| id.clone());
            let mut interfaces = section
                .lists
                .get("interface")
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .flat_map(|value| {
                    value
                        .split(|ch: char| ch == ',' || ch.is_ascii_whitespace())
                        .map(|item| normalize_interface_value(Some(item), ""))
                        .collect::<Vec<_>>()
                })
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>();

            if interfaces.is_empty() {
                interfaces = section
                    .options
                    .get("interface")
                    .map(|value| {
                        value
                            .split(|ch: char| ch == ',' || ch.is_ascii_whitespace())
                            .map(|item| normalize_interface_value(Some(item), ""))
                            .filter(|item| !item.is_empty())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
            }

            let mut seen_interfaces = BTreeSet::new();
            interfaces.retain(|interface| seen_interfaces.insert(interface.clone()));

            Some(InterfaceGroupConfig {
                id,
                name,
                enabled: parse_bool(section.options.get("enabled").map(String::as_str))
                    .unwrap_or(true),
                interfaces,
                check_enabled: parse_bool(section.options.get("check_enabled").map(String::as_str))
                    .unwrap_or(true),
                check_url: section
                    .options
                    .get("check_url")
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| DEFAULT_GROUP_CHECK_URL.to_string()),
                check_interval_seconds: parse_u64_option(
                    section.options.get("check_interval_seconds"),
                    DEFAULT_GROUP_CHECK_INTERVAL_SECONDS,
                ),
                check_timeout_seconds: parse_u64_option(
                    section.options.get("check_timeout_seconds"),
                    DEFAULT_GROUP_CHECK_TIMEOUT_SECONDS,
                ),
                failure_threshold: parse_u32_option(
                    section.options.get("failure_threshold"),
                    DEFAULT_GROUP_FAILURE_THRESHOLD,
                ),
                recovery_threshold: parse_u32_option(
                    section.options.get("recovery_threshold"),
                    DEFAULT_GROUP_RECOVERY_THRESHOLD,
                ),
                prefer_primary: parse_bool(
                    section.options.get("prefer_primary").map(String::as_str),
                )
                .unwrap_or(true),
            })
        })
        .collect()
}

fn load_custom_route_sections(
    sections: &[UciSection],
    default_target_interface: &str,
) -> Vec<CustomRouteConfig> {
    sections
        .iter()
        .filter(|section| section.section_type.as_deref() == Some("custom_route"))
        .filter_map(|section| {
            Some(CustomRouteConfig {
                name: section
                    .options
                    .get("name")
                    .map(|name| name.trim().to_string())
                    .filter(|name| !name.is_empty()),
                destination: normalize_ipv4_prefix(section.options.get("destination")?)?,
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

pub fn normalize_group_id(value: &str) -> Option<String> {
    let value = value.trim();

    if value.is_empty()
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return None;
    }

    Some(value.to_string())
}

pub fn group_id_from_target(target: &str) -> Option<&str> {
    target
        .trim()
        .strip_prefix("group:")
        .map(str::trim)
        .filter(|group_id| !group_id.is_empty())
}

fn decode_uci_values(value: &str) -> Vec<String> {
    let value = value.trim();
    let mut values = Vec::new();
    let mut current = String::new();
    let mut chars = value.chars().peekable();
    let mut quote = None::<char>;
    let mut started = false;

    while let Some(ch) = chars.next() {
        match quote {
            Some('\'') => {
                if ch == '\'' {
                    quote = None;
                } else {
                    current.push(ch);
                }
            }
            Some('"') => {
                if ch == '"' {
                    quote = None;
                } else if ch == '\\' {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                } else {
                    current.push(ch);
                }
            }
            _ if ch.is_ascii_whitespace() => {
                if started {
                    values.push(current.clone());
                    current.clear();
                    started = false;
                }
            }
            _ if ch == '\'' || ch == '"' => {
                quote = Some(ch);
                started = true;
            }
            _ if ch == '\\' => {
                started = true;
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            _ => {
                started = true;
                current.push(ch);
            }
        }
    }

    if started {
        values.push(current);
    }

    if !values.is_empty() {
        return values;
    }

    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        return vec![value[1..value.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")];
    }

    vec![value.to_string()]
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

pub fn normalize_ipv4_prefix(value: &str) -> Option<String> {
    let value = value.trim();
    let (address, mask) = match value.split_once('/') {
        Some((address, mask)) => (address.trim(), mask.trim().parse::<u8>().ok()?),
        None => (value, 32),
    };

    if mask > 32 {
        return None;
    }

    let address = address.parse::<Ipv4Addr>().ok()?;
    let address = u32::from(address);
    let mask_bits = if mask == 0 {
        0
    } else {
        u32::MAX << (32 - mask)
    };
    let network = Ipv4Addr::from(address & mask_bits);

    Some(format!("{network}/{mask}"))
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
            name: String::new(),
            section_type: Some("asn".to_string()),
            options,
            lists: BTreeMap::new(),
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

        let values = decode_uci_values(value);
        let value = values.first().cloned().unwrap_or_default();
        let (section_name, option_name) = match rest.split_once('.') {
            Some((section, option)) => (section, Some(option)),
            None => (rest, None),
        };
        let index = if let Some(index) = indexes.get(section_name) {
            *index
        } else {
            let index = sections.len();
            indexes.insert(section_name.to_string(), index);
            sections.push(UciSection {
                name: section_name.to_string(),
                ..UciSection::default()
            });
            index
        };

        if let Some(option_name) = option_name {
            sections[index]
                .lists
                .entry(option_name.to_string())
                .or_default()
                .extend(values.iter().cloned());
            sections[index]
                .options
                .insert(option_name.to_string(), values.join(" "));
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

fn parse_u64_option(value: Option<&String>, default: u64) -> u64 {
    value
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn parse_u32_option(value: Option<&String>, default: u32) -> u32 {
    value
        .and_then(|value| value.trim().parse::<u32>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
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

#[cfg(test)]
mod tests {
    use super::{decode_uci_values, load_interface_group_sections, normalize_ipv4_prefix};

    #[test]
    fn normalizes_ipv4_prefixes() {
        assert_eq!(
            normalize_ipv4_prefix("8.8.8.8"),
            Some("8.8.8.8/32".to_string())
        );
        assert_eq!(
            normalize_ipv4_prefix("8.8.8.9/24"),
            Some("8.8.8.0/24".to_string())
        );
        assert_eq!(
            normalize_ipv4_prefix("192.0.2.10/0"),
            Some("0.0.0.0/0".to_string())
        );
    }

    #[test]
    fn rejects_invalid_ipv4_prefixes() {
        assert_eq!(normalize_ipv4_prefix("example.com"), None);
        assert_eq!(normalize_ipv4_prefix("8.8.8.8/33"), None);
        assert_eq!(normalize_ipv4_prefix("300.8.8.8"), None);
    }

    #[test]
    fn decodes_uci_list_values() {
        assert_eq!(decode_uci_values("'wg0' 'tun0'"), vec!["wg0", "tun0"]);
        assert_eq!(
            decode_uci_values("'wg0'\\''backup' 'tun0'"),
            vec!["wg0'backup", "tun0"]
        );
        assert_eq!(decode_uci_values("'VPN Main'"), vec!["VPN Main"]);
    }

    #[test]
    fn parses_interface_group_list_values() {
        let sections = super::parse_uci_sections(
            "soya-asn-router",
            "\
soya-asn-router.@interface_group[0]=interface_group
soya-asn-router.@interface_group[0].id='vpn_main'
soya-asn-router.@interface_group[0].interface='wg0' 'tun0'
",
        );
        let groups = load_interface_group_sections(&sections);

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].interfaces, vec!["wg0", "tun0"]);
    }
}
