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

    // Sort newest first by created_at. Show date as help text in the
    // completion menu so the user can identify entries.
    let mut entries: Vec<(String, String)> = items
        .iter()
        .filter_map(|item| {
            let id = item[id_field].as_str()?.to_string();
            let created = item["created_at"].as_str().unwrap_or("").to_string();
            Some((id, created))
        })
        .filter(|(id, _)| id.starts_with(prefix))
        .collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1));
    entries
        .into_iter()
        .map(|(id, created)| {
            // Truncate datetime to date+time (drop seconds/tz)
            let short_date = created.get(..16).unwrap_or(&created);
            CompletionCandidate::new(id).help(Some(short_date.to_string().into()))
        })
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

/// Print a shell completion script. For zsh, outputs a patched version
/// that uses `_describe -V` (unsorted) so completions preserve our
/// newest-first ordering instead of being sorted alphabetically.
pub fn print_completion_script(shell: &str) {
    let bin = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "nixfleet".to_string());

    match shell {
        "zsh" => {
            // Patched zsh script: uses _describe -V for unsorted groups
            print!(
                r#"#compdef nixfleet
function _clap_dynamic_completer_nixfleet() {{
    local _CLAP_COMPLETE_INDEX=$(expr $CURRENT - 1)
    local _CLAP_IFS=$'\n'

    local completions=("${{(@f)$( \
        _CLAP_IFS="$_CLAP_IFS" \
        _CLAP_COMPLETE_INDEX="$_CLAP_COMPLETE_INDEX" \
        COMPLETE="zsh" \
        {bin} -- "${{words[@]}}" 2>/dev/null \
    )}}")

    if [[ -n $completions ]]; then
        local -a dirs=()
        local -a vals=()
        local -a opts=()
        local completion
        for completion in $completions; do
            local value="${{completion%%:*}}"
            if [[ "$value" == */ ]]; then
                local dir_no_slash="${{value%/}}"
                if [[ "$completion" == *:* ]]; then
                    local desc="${{completion#*:}}"
                    dirs+=("$dir_no_slash:$desc")
                else
                    dirs+=("$dir_no_slash")
                fi
            elif [[ "$value" == -* ]]; then
                opts+=("$completion")
            else
                vals+=("$completion")
            fi
        done
        if [[ -n $vals ]]; then
            local -a val_ids=()
            local -a val_labels=()
            local v
            local idx=0
            for v in $vals; do
                local id="${{v%%:*}}"
                val_ids+=("$id")
                if [[ "$v" == *:* ]]; then
                    val_labels+=("$(printf '%03d' $idx) ${{v#*:}}  $id")
                else
                    val_labels+=("$(printf '%03d' $idx) $id")
                fi
                (( idx++ ))
            done
            compadd -V 1-values -l -d val_labels -a val_ids
        fi
        [[ -n $dirs ]] && compadd -V 2-dirs -a dirs
        if [[ -n $opts ]]; then
            local -a opt_ids=()
            local -a opt_labels=()
            local o
            for o in $opts; do
                opt_ids+=("${{o%%:*}}")
                if [[ "$o" == *:* ]]; then
                    opt_labels+=("${{o%%:*}}  -- ${{o#*:}}")
                else
                    opt_labels+=("${{o%%:*}}")
                fi
            done
            compadd -V 3-options -l -d opt_labels -a opt_ids
        fi
    fi
}}

compdef _clap_dynamic_completer_nixfleet nixfleet
"#,
                bin = bin
            );
        }
        _ => {
            // For bash/fish, use clap_complete's built-in generator
            eprintln!(
                "For {shell}, use: eval \"$(COMPLETE={shell} {bin})\"",
                shell = shell,
                bin = bin
            );
        }
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
