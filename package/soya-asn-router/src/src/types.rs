use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub enabled: bool,
    pub db_path: String,
    pub lan_interface: String,
    pub default_target_interface: String,
    pub periodic_sync: PeriodicSyncStatus,
    pub proxy: ProxyStatus,
    pub sync: SyncStatus,
    pub policy: PolicyStatus,
    pub interface_groups: Vec<InterfaceGroupStatus>,
    pub asns: Vec<AsnStatus>,
}

#[derive(Debug, Serialize)]
pub struct PeriodicSyncStatus {
    pub enabled: bool,
    pub mode: String,
    pub interval_minutes: u64,
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
    pub current_asn: Option<String>,
    pub total: i64,
    pub completed: i64,
    pub failed: i64,
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
pub struct InterfaceGroupStatus {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub check_enabled: bool,
    pub check_url: String,
    pub check_interval_seconds: u64,
    pub check_timeout_seconds: u64,
    pub failure_threshold: u32,
    pub recovery_threshold: u32,
    pub prefer_primary: bool,
    pub primary_interface: Option<String>,
    pub active_interface: Option<String>,
    pub state: String,
    pub switched_at: Option<String>,
    pub last_checked_at: Option<String>,
    pub last_error: Option<String>,
    pub interfaces: Vec<InterfaceHealthStatus>,
}

#[derive(Debug, Serialize)]
pub struct InterfaceHealthStatus {
    pub name: String,
    pub state: String,
    pub consecutive_successes: i64,
    pub consecutive_failures: i64,
    pub last_checked_at: Option<String>,
    pub last_ok_at: Option<String>,
    pub last_error: Option<String>,
    pub latency_ms: Option<i64>,
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
