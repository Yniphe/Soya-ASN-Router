use std::collections::BTreeMap;
use std::env;
use std::path::PathBuf;
use std::process::Command;

use anyhow::Result;
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

#[derive(Default)]
struct UciAsnSection {
    section_type: Option<String>,
    asn: Option<String>,
    target_interface: Option<String>,
    enabled: Option<String>,
}

fn load_asn_sections(default_target_interface: &str) -> Vec<AsnConfig> {
    let Some(output) = uci_show("soya-asn-router") else {
        return Vec::new();
    };

    let mut sections = BTreeMap::<String, UciAsnSection>::new();

    for line in output.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let Some(rest) = key.strip_prefix("soya-asn-router.") else {
            continue;
        };
        let value = decode_uci_value(value);

        if let Some((section, option)) = rest.split_once('.') {
            let entry = sections.entry(section.to_string()).or_default();
            match option {
                "asn" => entry.asn = Some(value),
                "interface" => entry.target_interface = Some(value),
                "enabled" => entry.enabled = Some(value),
                _ => {}
            }
        } else {
            sections.entry(rest.to_string()).or_default().section_type = Some(value);
        }
    }

    sections
        .into_values()
        .filter(|section| section.section_type.as_deref() == Some("asn"))
        .filter_map(|section| {
            Some(AsnConfig {
                asn: normalize_asn(section.asn.as_deref()?)?,
                target_interface: normalize_interface_value(
                    section.target_interface.as_deref(),
                    default_target_interface,
                ),
                enabled: parse_bool(section.enabled.as_deref()).unwrap_or(true),
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

fn normalize_asn(value: &str) -> Option<String> {
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

fn parse_bool(value: Option<&str>) -> Option<bool> {
    match value?.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" | "enabled" => Some(true),
        "0" | "false" | "no" | "off" | "disabled" => Some(false),
        _ => None,
    }
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
