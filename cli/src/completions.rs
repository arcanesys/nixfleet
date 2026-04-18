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

/// Build a blocking reqwest client with the same auth as the main CLI:
/// Bearer token + mTLS identity + custom CA cert.
fn build_completion_client(
    api_key: &str,
    client_cert: &str,
    client_key: &str,
    ca_cert: &str,
) -> Option<reqwest::blocking::Client> {
    let mut builder =
        reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(2));

    // mTLS identity
    if !client_cert.is_empty() && !client_key.is_empty() {
        let cert_pem = std::fs::read(client_cert).ok()?;
        let key_pem = std::fs::read(client_key).ok()?;
        let mut combined = cert_pem;
        combined.extend_from_slice(&key_pem);
        let identity = reqwest::Identity::from_pem(&combined).ok()?;
        builder = builder.identity(identity);
    }

    // Custom CA
    if !ca_cert.is_empty() {
        let ca_pem = std::fs::read(ca_cert).ok()?;
        let cert = reqwest::Certificate::from_pem(&ca_pem).ok()?;
        builder = builder.add_root_certificate(cert);
    }

    // Bearer auth
    if !api_key.is_empty() {
        let mut headers = reqwest::header::HeaderMap::new();
        let val = reqwest::header::HeaderValue::from_str(&format!("Bearer {}", api_key)).ok()?;
        headers.insert(reqwest::header::AUTHORIZATION, val);
        builder = builder.default_headers(headers);
    }

    builder.build().ok()
}

fn fetch_ids_blocking(endpoint: &str, id_field: &str, prefix: &str) -> Vec<CompletionCandidate> {
    let resolved = resolve_cp_config();
    let url = format!("{}{}", resolved.cp_url, endpoint);

    let client = match build_completion_client(
        &resolved.api_key,
        &resolved.client_cert,
        &resolved.client_key,
        &resolved.ca_cert,
    ) {
        Some(c) => c,
        None => return vec![],
    };

    let resp = match client.get(&url).send() {
        Ok(r) if r.status().is_success() => r,
        _ => return vec![],
    };

    let items: Vec<serde_json::Value> = match resp.json() {
        Ok(v) => v,
        Err(_) => return vec![],
    };

    // Sort newest first by created_at if available, then by ID descending.
    // Use display_order so completions appear before --options.
    let mut entries: Vec<(String, String)> = items
        .iter()
        .filter_map(|item| {
            let id = item[id_field].as_str()?.to_string();
            let created = item["created_at"].as_str().unwrap_or("").to_string();
            Some((id, created))
        })
        .filter(|(id, _)| id.starts_with(prefix))
        .collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1)); // newest first by created_at
    entries
        .into_iter()
        .enumerate()
        .map(|(i, (id, _))| CompletionCandidate::new(id).display_order(Some(i)))
        .collect()
}

struct ResolvedCompletionConfig {
    cp_url: String,
    api_key: String,
    client_cert: String,
    client_key: String,
    ca_cert: String,
}

/// Resolve full connection config for completions: CP URL, API key, TLS certs.
fn resolve_cp_config() -> ResolvedCompletionConfig {
    let default_url = "http://localhost:8080".to_string();
    let cwd = std::env::current_dir().unwrap_or_default();

    // Load config file
    let (cfg, config_dir) = crate::config::find_config_file(&cwd)
        .and_then(|path| {
            let dir = path
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .to_path_buf();
            crate::config::load_config_file(&path)
                .ok()
                .map(|c| (c, dir))
        })
        .unwrap_or_else(|| (crate::config::ConfigFile::default(), cwd));

    // Resolve CP URL
    let cp_url = std::env::var("NIXFLEET_CP_URL").ok().unwrap_or_else(|| {
        cfg.control_plane
            .as_ref()
            .and_then(|cp| cp.url.clone())
            .unwrap_or(default_url)
    });

    // Resolve API key from credentials
    let api_key = crate::config::load_credentials()
        .ok()
        .and_then(|creds| creds.entries.get(&cp_url).and_then(|e| e.api_key.clone()))
        .unwrap_or_default();

    // Resolve TLS paths (relative to config dir, with variable expansion)
    let resolve_path = |p: &str| -> String {
        let expanded = crate::config::expand_env_vars(p);
        let path = std::path::Path::new(&expanded);
        if path.is_absolute() {
            expanded
        } else {
            config_dir.join(&expanded).to_string_lossy().to_string()
        }
    };

    let client_cert = cfg
        .tls
        .as_ref()
        .and_then(|t| t.client_cert.as_deref())
        .map(&resolve_path)
        .unwrap_or_default();

    let client_key = cfg
        .tls
        .as_ref()
        .and_then(|t| t.client_key.as_deref())
        .map(&resolve_path)
        .unwrap_or_default();

    let ca_cert = cfg
        .control_plane
        .as_ref()
        .and_then(|cp| cp.ca_cert.as_deref())
        .map(&resolve_path)
        .unwrap_or_default();

    ResolvedCompletionConfig {
        cp_url,
        api_key,
        client_cert,
        client_key,
        ca_cert,
    }
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
