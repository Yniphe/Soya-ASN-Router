use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub enabled: bool,
    pub db_path: String,
    pub lan_interface: String,
    pub default_target_interface: String,
    pub proxy: ProxyStatus,
    pub sync: SyncStatus,
    pub policy: PolicyStatus,
    pub asns: Vec<AsnStatus>,
}

#[derive(Debug, Serialize)]
pub struct ProxyStatus {
    pub enabled: bool,
    pub proxy_type: String,
    pub url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SyncStatus {
    pub running: bool,
    pub mode: Option<String>,
    pub locked_at: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct PolicyStatus {
    pub enabled: bool,
    pub state: String,
    pub updated_at: Option<String>,
    pub interface_count: i64,
    pub ipv4_prefix_count: i64,
    pub last_error: Option<String>,
    pub nft_path: String,
    pub script_path: String,
}

#[derive(Debug, Serialize)]
pub struct AsnStatus {
    pub asn: String,
    pub provider_name: Option<String>,
    pub enabled: bool,
    pub target_interface: String,
    pub state: String,
    pub ipv4_count: i64,
    pub ipv6_count: i64,
    pub last_synced_at: Option<String>,
    pub sync_started_at: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct InterfaceListResponse {
    pub interfaces: Vec<InterfaceChoice>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InterfaceChoice {
    pub name: String,
    pub device: Option<String>,
    pub up: bool,
}
