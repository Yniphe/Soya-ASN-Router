use std::collections::BTreeMap;
use std::process::Command;
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use rusqlite::Connection;

use crate::config::{Config, InterfaceGroupConfig};
use crate::db::{
    read_group_active_interface, write_group_state, write_interface_health, InterfaceHealthRow,
};
use crate::policy::{discover_interfaces, NetworkInterface};
use crate::util::truncate_error;

#[derive(Default)]
pub struct HealthCheckScheduler {
    last_checked: BTreeMap<String, Instant>,
}

#[derive(Debug)]
pub struct HealthCheckResult {
    pub changed: bool,
    pub checked_groups: usize,
}

pub fn check_interface_groups(
    conn: &Connection,
    config: &Config,
    scheduler: Option<&mut HealthCheckScheduler>,
    force: bool,
) -> Result<HealthCheckResult> {
    let groups = config.active_interface_groups();

    if groups.is_empty() {
        return Ok(HealthCheckResult {
            changed: false,
            checked_groups: 0,
        });
    }

    let interfaces = discover_interfaces()?;
    let mut changed = false;
    let mut checked_groups = 0;

    match scheduler {
        Some(scheduler) => {
            for group in groups {
                if should_check_group(scheduler, group, force) {
                    checked_groups += 1;
                    changed |= check_one_group(conn, group, &interfaces)?;
                    scheduler
                        .last_checked
                        .insert(group.id.clone(), Instant::now());
                }
            }
        }
        None => {
            for group in groups {
                checked_groups += 1;
                changed |= check_one_group(conn, group, &interfaces)?;
            }
        }
    }

    Ok(HealthCheckResult {
        changed,
        checked_groups,
    })
}

fn should_check_group(
    scheduler: &HealthCheckScheduler,
    group: &InterfaceGroupConfig,
    force: bool,
) -> bool {
    if force {
        return true;
    }

    group.check_enabled
        && scheduler
            .last_checked
            .get(&group.id)
            .map(|last_checked| {
                last_checked.elapsed().as_secs() >= group.check_interval_seconds.max(1)
            })
            .unwrap_or(true)
}

fn check_one_group(
    conn: &Connection,
    group: &InterfaceGroupConfig,
    interfaces: &[NetworkInterface],
) -> Result<bool> {
    if !group.check_enabled || group.interfaces.is_empty() {
        return Ok(false);
    }

    let mut health_rows = BTreeMap::<String, InterfaceHealthRow>::new();
    let mut last_error = None::<String>;

    for interface in &group.interfaces {
        let check = run_interface_check(group, interface, interfaces);
        if let Err(error) = &check {
            last_error = Some(truncate_error(&format!("{error:#}")));
        }

        let row = match check {
            Ok(latency_ms) => {
                write_interface_health(conn, &group.id, interface, true, Some(latency_ms), None)?
            }
            Err(error) => {
                let message = truncate_error(&format!("{error:#}"));
                write_interface_health(conn, &group.id, interface, false, None, Some(&message))?
            }
        };

        health_rows.insert(interface.clone(), row);
    }

    let previous_active = read_group_active_interface(conn, group)?;
    let active = choose_active_interface(group, previous_active.as_deref(), &health_rows);
    let changed = active != previous_active;
    let state = group_state(group, active.as_deref(), &health_rows);
    let group_error = if state == "error" {
        Some("all group interfaces are unhealthy".to_string())
    } else if state == "degraded" {
        last_error
    } else {
        None
    };

    write_group_state(
        conn,
        &group.id,
        active.as_deref(),
        &state,
        changed,
        group_error.as_deref(),
    )?;

    Ok(changed)
}

fn run_interface_check(
    group: &InterfaceGroupConfig,
    interface: &str,
    interfaces: &[NetworkInterface],
) -> Result<i64> {
    let network_interface = find_interface(interfaces, interface)
        .with_context(|| format!("interface '{interface}' was not found"))?;
    let device = network_interface
        .device_name()
        .unwrap_or_else(|| interface.to_string());
    let started = Instant::now();
    let timeout = group.check_timeout_seconds.max(1).to_string();
    let output = Command::new("curl")
        .args([
            "--interface",
            &device,
            "--connect-timeout",
            &timeout,
            "--max-time",
            &timeout,
            "--ipv4",
            "--silent",
            "--show-error",
            "--fail",
            "--output",
            "/dev/null",
            &group.check_url,
        ])
        .output()
        .with_context(|| format!("failed to run curl for {interface}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(anyhow!(
            "health check failed for {interface}{}",
            if stderr.is_empty() {
                String::new()
            } else {
                format!(": {stderr}")
            }
        ));
    }

    Ok(started.elapsed().as_millis().min(i64::MAX as u128) as i64)
}

fn choose_active_interface(
    group: &InterfaceGroupConfig,
    previous_active: Option<&str>,
    health_rows: &BTreeMap<String, InterfaceHealthRow>,
) -> Option<String> {
    let primary = group.primary_interface()?;
    let previous = previous_active
        .filter(|interface| group.interfaces.iter().any(|item| item == interface))
        .unwrap_or(primary);

    if group.prefer_primary && is_recovered(group, primary, health_rows) {
        return Some(primary.to_string());
    }

    if !is_unhealthy(group, previous, health_rows) {
        return Some(previous.to_string());
    }

    group
        .interfaces
        .iter()
        .find(|interface| is_candidate_healthy(group, interface, health_rows))
        .cloned()
        .or_else(|| Some(previous.to_string()))
}

fn group_state(
    group: &InterfaceGroupConfig,
    active: Option<&str>,
    health_rows: &BTreeMap<String, InterfaceHealthRow>,
) -> String {
    let Some(active) = active else {
        return "error".to_string();
    };

    if is_unhealthy(group, active, health_rows) {
        return "error".to_string();
    }

    if has_failures(active, health_rows) || group.primary_interface() != Some(active) {
        "degraded".to_string()
    } else {
        "healthy".to_string()
    }
}

fn is_candidate_healthy(
    group: &InterfaceGroupConfig,
    interface: &str,
    health_rows: &BTreeMap<String, InterfaceHealthRow>,
) -> bool {
    let Some(row) = health_rows.get(interface) else {
        return false;
    };

    row.consecutive_successes > 0
        && row.consecutive_failures < i64::from(group.failure_threshold.max(1))
}

fn has_failures(interface: &str, health_rows: &BTreeMap<String, InterfaceHealthRow>) -> bool {
    health_rows
        .get(interface)
        .map(|row| row.consecutive_failures > 0)
        .unwrap_or(false)
}

fn is_recovered(
    group: &InterfaceGroupConfig,
    interface: &str,
    health_rows: &BTreeMap<String, InterfaceHealthRow>,
) -> bool {
    health_rows
        .get(interface)
        .map(|row| {
            row.consecutive_successes >= i64::from(group.recovery_threshold.max(1))
                && row.consecutive_failures == 0
        })
        .unwrap_or(false)
}

fn is_unhealthy(
    group: &InterfaceGroupConfig,
    interface: &str,
    health_rows: &BTreeMap<String, InterfaceHealthRow>,
) -> bool {
    health_rows
        .get(interface)
        .map(|row| row.consecutive_failures >= i64::from(group.failure_threshold.max(1)))
        .unwrap_or(false)
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
