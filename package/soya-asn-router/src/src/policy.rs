use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::config::{group_id_from_target, Config};
use crate::constants::{
    MARK_BASE, MAX_POLICY_GROUPS, ROUTES_NFT_PATH, ROUTES_SH_PATH, ROUTE_TABLE_NAME,
    RULE_PREF_BASE, TABLE_ID_BASE,
};
use crate::db::{read_group_active_interface, read_ipv4_prefixes, write_policy_state};
use crate::util::truncate_error;

#[derive(Debug)]
pub struct PolicyStats {
    pub interface_count: i64,
    pub ipv4_prefix_count: i64,
    pub warning: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RoutePolicyPreview {
    pub interface_count: i64,
    pub ipv4_prefix_count: i64,
    pub groups: Vec<RoutePolicyPreviewGroup>,
}

#[derive(Debug, Serialize)]
pub struct RoutePolicyPreviewGroup {
    pub interface: String,
    pub device: String,
    pub table_id: u32,
    pub mark: u32,
    pub pref: u32,
    pub asn_count: i64,
    pub custom_route_count: i64,
    pub ipv4_prefix_count: i64,
    pub asn_ipv4_prefix_count: i64,
    pub custom_ipv4_prefix_count: i64,
    pub default_route_nexthop: Option<String>,
    pub sample_prefixes: Vec<String>,
}

#[derive(Debug)]
struct PolicyPlan {
    lan_device: String,
    groups: Vec<PolicyGroup>,
}

#[derive(Debug)]
struct PolicyGroup {
    interface: String,
    device: String,
    table_id: u32,
    mark: u32,
    pref: u32,
    asn_set_name: String,
    custom_set_name: String,
    asn_count: i64,
    custom_route_count: i64,
    asn_prefixes: Vec<String>,
    custom_prefixes: Vec<String>,
    default_route: Option<NetworkRoute>,
}

impl PolicyGroup {
    fn ipv4_prefix_count(&self) -> i64 {
        self.asn_prefixes.len() as i64 + self.custom_prefixes.len() as i64
    }

    fn sample_prefixes(&self, limit: usize) -> Vec<String> {
        self.custom_prefixes
            .iter()
            .chain(self.asn_prefixes.iter())
            .take(limit)
            .cloned()
            .collect()
    }
}

#[derive(Debug, Default)]
struct PrefixGroupBuild {
    asn_count: i64,
    custom_route_count: i64,
    asn_prefixes: BTreeSet<String>,
    custom_prefixes: BTreeSet<String>,
}

#[derive(Debug, Deserialize)]
struct NetworkDump {
    #[serde(default)]
    interface: Vec<NetworkInterface>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NetworkInterface {
    pub(crate) interface: String,
    #[serde(default)]
    pub(crate) up: bool,
    #[serde(default)]
    device: Option<String>,
    #[serde(default)]
    l3_device: Option<String>,
    #[serde(default)]
    route: Vec<NetworkRoute>,
}

#[derive(Debug, Clone, Deserialize)]
struct NetworkRoute {
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    mask: Option<u8>,
    #[serde(default)]
    nexthop: Option<String>,
}

impl NetworkInterface {
    pub(crate) fn device_name(&self) -> Option<String> {
        self.l3_device
            .as_deref()
            .or(self.device.as_deref())
            .map(str::trim)
            .filter(|device| !device.is_empty())
            .map(ToOwned::to_owned)
    }

    fn default_ipv4_route(&self) -> Option<&NetworkRoute> {
        self.route.iter().find(|route| {
            route.mask == Some(0)
                && route
                    .target
                    .as_deref()
                    .map(|target| target == "0.0.0.0")
                    .unwrap_or(false)
        })
    }
}

pub fn discover_interfaces() -> Result<Vec<NetworkInterface>> {
    let output = Command::new("ubus")
        .args(["-S", "call", "network.interface", "dump"])
        .output()
        .context("failed to call ubus network.interface dump")?;

    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(anyhow!(
            "ubus network.interface dump failed{}",
            if message.is_empty() {
                String::new()
            } else {
                format!(": {message}")
            }
        ));
    }

    let mut dump = serde_json::from_slice::<NetworkDump>(&output.stdout)
        .context("failed to parse ubus network interface dump")?;
    dump.interface
        .sort_by(|left, right| left.interface.cmp(&right.interface));

    Ok(dump.interface)
}

pub fn preview_routes(conn: &Connection, config: &Config) -> Result<RoutePolicyPreview> {
    let plan = build_policy_plan(conn, config)?;
    let ipv4_prefix_count = count_prefixes(&plan.groups);

    Ok(RoutePolicyPreview {
        interface_count: plan.groups.len() as i64,
        ipv4_prefix_count,
        groups: plan
            .groups
            .into_iter()
            .map(|group| {
                let ipv4_prefix_count = group.ipv4_prefix_count();
                let asn_ipv4_prefix_count = group.asn_prefixes.len() as i64;
                let custom_ipv4_prefix_count = group.custom_prefixes.len() as i64;
                let sample_prefixes = group.sample_prefixes(10);
                let default_route_nexthop = group
                    .default_route
                    .as_ref()
                    .and_then(|route| route.nexthop.as_deref())
                    .map(str::trim)
                    .filter(|nexthop| !nexthop.is_empty())
                    .map(ToOwned::to_owned);

                RoutePolicyPreviewGroup {
                    interface: group.interface,
                    device: group.device,
                    table_id: group.table_id,
                    mark: group.mark,
                    pref: group.pref,
                    asn_count: group.asn_count,
                    custom_route_count: group.custom_route_count,
                    ipv4_prefix_count,
                    asn_ipv4_prefix_count,
                    custom_ipv4_prefix_count,
                    default_route_nexthop,
                    sample_prefixes,
                }
            })
            .collect(),
    })
}

pub fn generate_route_files(conn: &Connection, config: &Config) -> Result<PolicyStats> {
    let plan = build_policy_plan(conn, config)?;
    let ipv4_prefix_count = count_prefixes(&plan.groups);

    let routes_dir = Path::new(ROUTES_NFT_PATH)
        .parent()
        .ok_or_else(|| anyhow!("invalid route policy path: {ROUTES_NFT_PATH}"))?;
    fs::create_dir_all(routes_dir)
        .with_context(|| format!("failed to create {}", routes_dir.display()))?;

    fs::write(
        ROUTES_NFT_PATH,
        render_nft_policy(&plan.lan_device, &plan.groups),
    )
    .with_context(|| format!("failed to write {ROUTES_NFT_PATH}"))?;
    fs::write(ROUTES_SH_PATH, render_route_script(&plan.groups))
        .with_context(|| format!("failed to write {ROUTES_SH_PATH}"))?;
    fs::set_permissions(ROUTES_SH_PATH, fs::Permissions::from_mode(0o755))
        .with_context(|| format!("failed to chmod {ROUTES_SH_PATH}"))?;

    Ok(PolicyStats {
        interface_count: plan.groups.len() as i64,
        ipv4_prefix_count,
        warning: None,
    })
}

fn build_policy_plan(conn: &Connection, config: &Config) -> Result<PolicyPlan> {
    let interfaces = discover_interfaces()?;
    let lan = find_interface(&interfaces, &config.lan_interface)
        .with_context(|| format!("LAN interface '{}' was not found", config.lan_interface))?;
    let lan_device = lan.device_name().with_context(|| {
        format!(
            "LAN interface '{}' does not have an L3 device",
            config.lan_interface
        )
    })?;

    let mut prefix_groups: BTreeMap<String, PrefixGroupBuild> = BTreeMap::new();

    for asn in config.active_asns() {
        let target_interface = resolve_target_interface(conn, config, &asn.target_interface)?;
        let group = prefix_groups.entry(target_interface).or_default();

        group.asn_count += 1;

        for prefix in read_ipv4_prefixes(conn, &asn.asn)? {
            group.asn_prefixes.insert(prefix);
        }
    }

    for route in config.active_custom_routes() {
        let target_interface = resolve_target_interface(conn, config, &route.target_interface)?;
        let group = prefix_groups.entry(target_interface).or_default();

        group.custom_route_count += 1;
        group.custom_prefixes.insert(route.destination.clone());
    }

    prefix_groups
        .retain(|_, group| !group.asn_prefixes.is_empty() || !group.custom_prefixes.is_empty());

    if prefix_groups.len() as u32 > MAX_POLICY_GROUPS {
        return Err(anyhow!(
            "too many target interfaces: {} configured, maximum is {}",
            prefix_groups.len(),
            MAX_POLICY_GROUPS
        ));
    }

    let mut groups = Vec::with_capacity(prefix_groups.len());

    for (index, (interface, prefix_group)) in prefix_groups.into_iter().enumerate() {
        let target = find_interface(&interfaces, &interface)
            .with_context(|| format!("target interface '{interface}' was not found"))?;
        let device = target
            .device_name()
            .with_context(|| format!("target interface '{interface}' does not have a device"))?;
        let offset = index as u32;

        groups.push(PolicyGroup {
            asn_set_name: format!("to_{}_{}_v4", offset, sanitize_nft_ident(&interface)),
            custom_set_name: format!("custom_to_{}_{}_v4", offset, sanitize_nft_ident(&interface)),
            interface,
            device,
            table_id: TABLE_ID_BASE + offset,
            mark: MARK_BASE + offset,
            pref: RULE_PREF_BASE + offset,
            asn_count: prefix_group.asn_count,
            custom_route_count: prefix_group.custom_route_count,
            asn_prefixes: prefix_group.asn_prefixes.into_iter().collect(),
            custom_prefixes: prefix_group.custom_prefixes.into_iter().collect(),
            default_route: target.default_ipv4_route().cloned(),
        });
    }

    Ok(PolicyPlan { lan_device, groups })
}

fn resolve_target_interface(conn: &Connection, config: &Config, target: &str) -> Result<String> {
    let Some(group_id) = group_id_from_target(target) else {
        return Ok(target.to_string());
    };
    let group = config
        .find_interface_group(group_id)
        .with_context(|| format!("interface group '{group_id}' was not found"))?;

    read_group_active_interface(conn, group)?
        .or_else(|| group.primary_interface().map(ToOwned::to_owned))
        .with_context(|| format!("interface group '{group_id}' has no active interface"))
}

fn count_prefixes(groups: &[PolicyGroup]) -> i64 {
    groups
        .iter()
        .map(PolicyGroup::ipv4_prefix_count)
        .sum::<i64>()
}

pub fn apply_routes(conn: &Connection, config: &Config) -> Result<PolicyStats> {
    let stats = match generate_route_files(conn, config) {
        Ok(stats) => stats,
        Err(error) => {
            let message = truncate_error(&format!("{error:#}"));
            write_policy_state(conn, "error", 0, 0, Some(&message))?;
            return Err(error);
        }
    };

    let output = match Command::new("sh").arg(ROUTES_SH_PATH).output() {
        Ok(output) => output,
        Err(error) => {
            let message = truncate_error(&format!("failed to run {ROUTES_SH_PATH}: {error}"));
            write_policy_state(conn, "error", 0, 0, Some(&message))?;
            return Err(anyhow!(message));
        }
    };

    if !output.status.success() {
        let message = truncate_error(&route_script_error(&output));
        write_policy_state(conn, "error", 0, 0, Some(&message))?;
        return Err(anyhow!(message));
    }

    write_policy_state(
        conn,
        "applied",
        stats.interface_count,
        stats.ipv4_prefix_count,
        stats.warning.as_deref(),
    )?;

    Ok(stats)
}

pub fn disable_routes(conn: &Connection) -> Result<()> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(render_route_cleanup_script())
        .output()
        .context("failed to run route policy cleanup")?;

    if !output.status.success() {
        let message = truncate_error(&route_script_error(&output));
        write_policy_state(conn, "error", 0, 0, Some(&message))?;
        return Err(anyhow!(message));
    }

    write_policy_state(conn, "paused", 0, 0, None)?;
    Ok(())
}

pub fn reconcile_route_policy(conn: &Connection, config: &Config) -> Result<()> {
    if config.route_policy_enabled {
        apply_routes(conn, config)?;
    } else {
        disable_routes(conn)?;
    }

    Ok(())
}

fn find_interface<'a>(
    interfaces: &'a [NetworkInterface],
    name: &str,
) -> Option<&'a NetworkInterface> {
    interfaces
        .iter()
        .find(|iface| iface.interface == name)
        .or_else(|| {
            interfaces
                .iter()
                .find(|iface| iface.device_name().as_deref() == Some(name))
        })
}

fn render_nft_policy(lan_device: &str, groups: &[PolicyGroup]) -> String {
    let mut nft = String::new();

    nft.push_str(&format!("table inet {ROUTE_TABLE_NAME} {{\n"));

    for group in groups {
        render_prefix_set(&mut nft, &group.asn_set_name, &group.asn_prefixes);
        render_prefix_set(&mut nft, &group.custom_set_name, &group.custom_prefixes);
    }

    nft.push_str("  chain prerouting {\n");
    nft.push_str("    type filter hook prerouting priority mangle; policy accept;\n");

    for group in groups {
        if !group.asn_prefixes.is_empty() {
            render_mark_rule(&mut nft, lan_device, &group.asn_set_name, group.mark);
        }
    }

    for group in groups {
        if !group.custom_prefixes.is_empty() {
            render_mark_rule(&mut nft, lan_device, &group.custom_set_name, group.mark);
        }
    }

    nft.push_str("  }\n");
    nft.push_str("}\n");

    nft
}

fn render_prefix_set(nft: &mut String, set_name: &str, prefixes: &[String]) {
    if prefixes.is_empty() {
        return;
    }

    nft.push_str(&format!("  set {set_name} {{\n"));
    nft.push_str("    type ipv4_addr\n");
    nft.push_str("    flags interval\n");
    nft.push_str("    auto-merge\n");
    nft.push_str("    elements = {\n");

    for (index, prefix) in prefixes.iter().enumerate() {
        let comma = if index + 1 == prefixes.len() { "" } else { "," };
        nft.push_str(&format!("      {prefix}{comma}\n"));
    }

    nft.push_str("    }\n");
    nft.push_str("  }\n\n");
}

fn render_mark_rule(nft: &mut String, lan_device: &str, set_name: &str, mark: u32) {
    nft.push_str(&format!(
        "    iifname {} ip daddr @{set_name} meta mark set 0x{mark:x}\n",
        nft_quote(lan_device)
    ));
}

fn render_route_script(groups: &[PolicyGroup]) -> String {
    let mut script = render_route_cleanup_script();

    script.push_str(&format!("nft -f {}\n\n", shell_quote(ROUTES_NFT_PATH)));

    for group in groups {
        let device = shell_quote(&group.device);
        let comment_interface = group.interface.replace(['\n', '\r'], " ");
        script.push_str(&format!("# target interface: {comment_interface}\n"));
        match group
            .default_route
            .as_ref()
            .and_then(|route| route.nexthop.as_deref())
            .map(str::trim)
            .filter(|nexthop| !nexthop.is_empty())
        {
            Some(nexthop) => script.push_str(&format!(
                "ip -4 route replace default via {} dev {} table {}\n",
                shell_quote(nexthop),
                device,
                group.table_id
            )),
            None => script.push_str(&format!(
                "ip -4 route replace default dev {} table {}\n",
                device, group.table_id
            )),
        }

        script.push_str(&format!(
            "ip -4 rule add pref {} fwmark 0x{:x} lookup {}\n",
            group.pref, group.mark, group.table_id
        ));
    }

    script.push_str("\nip -4 route flush cache 2>/dev/null || true\n");
    script
}

fn render_route_cleanup_script() -> String {
    let mut script = String::new();

    script.push_str("#!/bin/sh\n");
    script.push_str("set -eu\n\n");
    script.push_str(&format!(
        "nft delete table inet {ROUTE_TABLE_NAME} 2>/dev/null || true\n"
    ));
    script.push_str("i=0\n");
    script.push_str(&format!("while [ \"$i\" -lt {MAX_POLICY_GROUPS} ]; do\n"));
    script.push_str(&format!(
        "  ip -4 rule del pref $(({RULE_PREF_BASE} + i)) 2>/dev/null || true\n"
    ));
    script.push_str(&format!(
        "  ip -4 route flush table $(({TABLE_ID_BASE} + i)) 2>/dev/null || true\n"
    ));
    script.push_str("  i=$((i + 1))\n");
    script.push_str("done\n\n");
    script.push_str("ip -4 route flush cache 2>/dev/null || true\n\n");

    script
}

fn route_script_error(output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => format!("route policy script failed with status {}", output.status),
        (false, true) => format!("route policy script failed: {stdout}"),
        (true, false) => format!("route policy script failed: {stderr}"),
        (false, false) => format!("route policy script failed: {stderr}; stdout: {stdout}"),
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn nft_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn sanitize_nft_ident(value: &str) -> String {
    let mut ident = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();

    if ident.is_empty() {
        ident.push_str("iface");
    }

    ident
}

#[cfg(test)]
mod tests {
    use super::{render_nft_policy, PolicyGroup};

    fn policy_group(
        interface: &str,
        mark: u32,
        asn_prefixes: &[&str],
        custom_prefixes: &[&str],
    ) -> PolicyGroup {
        PolicyGroup {
            interface: interface.to_string(),
            device: interface.to_string(),
            table_id: 1000,
            mark,
            pref: 30000,
            asn_set_name: format!("to_{interface}_v4"),
            custom_set_name: format!("custom_to_{interface}_v4"),
            asn_count: asn_prefixes.len() as i64,
            custom_route_count: custom_prefixes.len() as i64,
            asn_prefixes: asn_prefixes
                .iter()
                .map(|prefix| (*prefix).to_string())
                .collect(),
            custom_prefixes: custom_prefixes
                .iter()
                .map(|prefix| (*prefix).to_string())
                .collect(),
            default_route: None,
        }
    }

    #[test]
    fn custom_prefix_rules_are_rendered_after_asn_rules() {
        let groups = [
            policy_group("wan", 0x1200, &["8.8.8.0/24"], &[]),
            policy_group("wg0", 0x1201, &[], &["8.8.8.8/32"]),
        ];
        let nft = render_nft_policy("br-lan", &groups);

        assert!(nft.contains("set to_wan_v4"));
        assert!(nft.contains("set custom_to_wg0_v4"));
        assert!(!nft.contains("@custom_to_wan_v4"));
        assert!(!nft.contains("@to_wg0_v4"));

        let asn_rule = nft.find("@to_wan_v4").expect("missing ASN rule");
        let custom_rule = nft.find("@custom_to_wg0_v4").expect("missing custom rule");
        assert!(asn_rule < custom_rule);
    }
}
