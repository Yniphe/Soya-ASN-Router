use std::process::Command;

use anyhow::{anyhow, Context, Result};
use chrono::Utc;

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

pub fn now_unix() -> i64 {
    Utc::now().timestamp()
}

pub fn truncate_error(message: &str) -> String {
    const LIMIT: usize = 512;

    if message.len() <= LIMIT {
        return message.to_string();
    }

    format!("{}...", &message[..LIMIT])
}

pub fn run_status_command(command: &mut Command, context: &str) -> Result<()> {
    let output = command.output().context(context.to_string())?;

    if output.status.success() {
        return Ok(());
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
