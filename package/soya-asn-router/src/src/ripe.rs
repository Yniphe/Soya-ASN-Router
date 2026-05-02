use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use reqwest::blocking::Client;
use reqwest::Proxy;
use serde::Deserialize;

use crate::config::Config;

#[derive(Debug, Deserialize)]
struct RipeResponse {
    status: Option<String>,
    data: Option<RipeData>,
    messages: Option<Vec<Vec<String>>>,
}

#[derive(Debug, Deserialize)]
struct RipeData {
    prefixes: Vec<RipePrefix>,
}

#[derive(Debug, Deserialize)]
struct RipePrefix {
    prefix: String,
}

#[derive(Debug, Deserialize)]
struct RipeOverviewResponse {
    status: Option<String>,
    data: Option<RipeOverviewData>,
    messages: Option<Vec<Vec<String>>>,
}

#[derive(Debug, Deserialize)]
struct RipeOverviewData {
    holder: Option<String>,
}

pub fn build_http_client(config: &Config) -> Result<Client> {
    let mut builder = Client::builder()
        .timeout(Duration::from_secs(60))
        .user_agent("soya-asn-router-openwrt/0.1");

    if config.proxy_enabled {
        if let Some(proxy_url) = config.proxy_url.as_deref() {
            let proxy_url = normalize_proxy_url(&config.proxy_type, proxy_url);
            builder = builder.proxy(Proxy::all(&proxy_url).with_context(|| {
                format!("invalid {} proxy URL: {proxy_url}", config.proxy_type)
            })?);
        }
    }

    builder.build().context("failed to build HTTP client")
}

pub fn fetch_prefixes(client: &Client, asn: &str) -> Result<Vec<String>> {
    let url = format!("https://stat.ripe.net/data/announced-prefixes/data.json?resource={asn}");
    let response = client
        .get(url)
        .send()
        .with_context(|| format!("RIPE request failed for {asn}"))?
        .error_for_status()
        .with_context(|| format!("RIPE returned HTTP error for {asn}"))?;

    let body = response
        .json::<RipeResponse>()
        .with_context(|| format!("RIPE returned invalid JSON for {asn}"))?;

    if body.status.as_deref() != Some("ok") {
        let message = body
            .messages
            .as_ref()
            .and_then(|messages| messages.first())
            .and_then(|message| message.get(1))
            .cloned()
            .unwrap_or_else(|| "RIPE response status is not ok".to_string());

        return Err(anyhow!(message));
    }

    Ok(body
        .data
        .map(|data| {
            data.prefixes
                .into_iter()
                .map(|prefix| prefix.prefix)
                .collect()
        })
        .unwrap_or_default())
}

pub fn fetch_asn_provider(client: &Client, asn: &str) -> Result<Option<String>> {
    let url = format!("https://stat.ripe.net/data/as-overview/data.json?resource={asn}");
    let response = client
        .get(url)
        .send()
        .with_context(|| format!("RIPE AS overview request failed for {asn}"))?
        .error_for_status()
        .with_context(|| format!("RIPE returned HTTP error for AS overview {asn}"))?;

    let body = response
        .json::<RipeOverviewResponse>()
        .with_context(|| format!("RIPE returned invalid AS overview JSON for {asn}"))?;

    if body.status.as_deref() != Some("ok") {
        let message = body
            .messages
            .as_ref()
            .and_then(|messages| messages.first())
            .and_then(|message| message.get(1))
            .cloned()
            .unwrap_or_else(|| "RIPE AS overview status is not ok".to_string());

        return Err(anyhow!(message));
    }

    Ok(body
        .data
        .and_then(|data| data.holder)
        .map(|holder| holder.trim().to_string())
        .filter(|holder| !holder.is_empty()))
}

fn normalize_proxy_url(proxy_type: &str, value: &str) -> String {
    let value = value.trim();

    if value.contains("://") {
        return value.to_string();
    }

    match proxy_type {
        "socks5" => format!("socks5h://{value}"),
        _ => format!("http://{value}"),
    }
}
