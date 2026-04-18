use clap_complete::engine::{ArgValueCompleter, CompletionCandidate};

fn complete_rollout_ids(current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    let prefix = current.to_string_lossy().to_string();
    fetch_ids_blocking("/api/v1/rollouts", "id", &prefix)
}

fn complete_release_ids(current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    let prefix = current.to_string_lossy().to_string();
    fetch_ids_blocking("/api/v1/releases", "id", &prefix)
}

fn complete_machine_ids(current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    let prefix = current.to_string_lossy().to_string();
    fetch_ids_blocking("/api/v1/machines", "machine_id", &prefix)
}

fn fetch_ids_blocking(endpoint: &str, id_field: &str, prefix: &str) -> Vec<CompletionCandidate> {
    let (cp_url, api_key) = resolve_cp_config();
    let url = format!("{}{}", cp_url, endpoint);

    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .danger_accept_invalid_certs(true)
        .build()
    {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    let mut req = client.get(&url);
    if !api_key.is_empty() {
        req = req.header("x-api-key", &api_key);
    }

    let resp = match req.send() {
        Ok(r) if r.status().is_success() => r,
        _ => return vec![],
    };

    let items: Vec<serde_json::Value> = match resp.json() {
        Ok(v) => v,
        Err(_) => return vec![],
    };

    items
        .iter()
        .filter_map(|item| item[id_field].as_str().map(|s| s.to_string()))
        .filter(|id| id.starts_with(prefix))
        .map(|id| CompletionCandidate::new(id))
        .collect()
}

/// Resolve CP URL and API key from config + credentials for completions.
fn resolve_cp_config() -> (String, String) {
    let default_url = "http://localhost:8080".to_string();

    let cp_url = std::env::var("NIXFLEET_CP_URL").ok().unwrap_or_else(|| {
        let cwd = std::env::current_dir().unwrap_or_default();
        crate::config::find_config_file(&cwd)
            .and_then(|path| crate::config::load_config_file(&path).ok())
            .and_then(|cfg| cfg.control_plane.as_ref().and_then(|cp| cp.url.clone()))
            .unwrap_or(default_url)
    });

    // Load API key from credentials file
    let api_key = crate::config::load_credentials()
        .ok()
        .and_then(|creds| creds.entries.get(&cp_url).and_then(|e| e.api_key.clone()))
        .unwrap_or_default();

    (cp_url, api_key)
}

pub fn rollout_id_completer() -> ArgValueCompleter {
    ArgValueCompleter::new(complete_rollout_ids)
}

pub fn release_id_completer() -> ArgValueCompleter {
    ArgValueCompleter::new(complete_release_ids)
}

pub fn machine_id_completer() -> ArgValueCompleter {
    ArgValueCompleter::new(complete_machine_ids)
}
