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
    let cp_url = resolve_cp_url();
    let url = format!("{}{}", cp_url, endpoint);

    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
    {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    let resp = match client.get(&url).send() {
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

fn resolve_cp_url() -> String {
    if let Ok(url) = std::env::var("NIXFLEET_CP_URL") {
        return url;
    }

    let cwd = std::env::current_dir().unwrap_or_default();
    if let Some(config_path) = crate::config::find_config_file(&cwd) {
        if let Ok(cfg) = crate::config::load_config_file(&config_path) {
            if let Some(url) = cfg.control_plane.as_ref().and_then(|cp| cp.url.clone()) {
                return url;
            }
        }
    }

    "http://localhost:8080".to_string()
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
