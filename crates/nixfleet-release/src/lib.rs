#![allow(clippy::doc_lazy_continuation)]
//! Producer for `releases/fleet.resolved.json` and signed sidecars. Pipeline:
//! enumerate hosts -> build -> push -> eval -> inject closureHashes -> stamp meta
//! -> canonicalize -> sign -> smoke-verify -> atomic write -> optional git
//! commit/push. Hook contract lives at the binary surface.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use nixfleet_proto::{FleetResolved, RevocationEntry, Revocations};
use nixfleet_reconciler::project_manifest;
use sha2::{Digest, Sha256};

mod git;
mod sign;

pub use git::render_commit_message;

use git::{git_commit_release, git_head_sha, git_push_release};
use sign::{sign, smoke_verify, write_release};

/// Hosts to release. Resolved against the consumer's flake at runtime.
#[derive(Debug, Clone)]
pub enum HostsSpec {
    /// Union of `nixosConfigurations.*` and `darwinConfigurations.*`.
    Auto,
    /// `Auto` minus the listed names.
    AutoExclude(Vec<String>),
    /// Explicit list, order preserved. Names appearing in both
    /// `nixosConfigurations` and `darwinConfigurations` error at classify
    /// time; operator must disambiguate.
    Explicit(Vec<String>),
}

/// Which `*Configurations` attrset a host lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKind {
    Nixos,
    Darwin,
}

impl HostKind {
    pub fn attr_prefix(self) -> &'static str {
        match self {
            HostKind::Nixos => "nixosConfigurations",
            HostKind::Darwin => "darwinConfigurations",
        }
    }
}

/// CLI-assembled config consumed by `run`.
#[derive(Debug, Clone)]
pub struct ReleaseConfig {
    pub flake_dir: PathBuf,
    /// Default `.#fleet.resolved`.
    pub fleet_resolved_attr: String,
    pub hosts: HostsSpec,
    /// Env: `NIXFLEET_HOST`, `NIXFLEET_PATH`, `NIXFLEET_CLOSURE_HASH`.
    pub push_cmd: Option<String>,
    /// Env: `NIXFLEET_INPUT` (canonical bytes), `NIXFLEET_OUTPUT`
    /// (where hook writes raw signature). Required.
    pub sign_cmd: String,
    /// `ed25519` | `ecdsa-p256`.
    pub signature_algorithm: String,
    pub release_dir: PathBuf,
    pub artifact_name: String,
    pub git_commit: bool,
    pub git_push: Option<GitPushTarget>,
    pub commit_template: String,
    pub git_user_name: Option<String>,
    pub git_user_email: Option<String>,
    /// Structural smoke verify: canonicalize round-trip + schema parse +
    /// non-zero sig length. Default on.
    pub smoke_verify: bool,
    /// Reuse `signedAt` when closureHashes match - produces byte-stable
    /// releases on no-op runs.
    pub reuse_unchanged_signature: bool,
    /// Flake attr yielding the revocations list. When set, the pipeline signs
    /// `revocations.json` alongside `fleet.resolved.json` via the same
    /// `sign_cmd`. `None` skips the revocations artifact.
    pub revocations_attr: Option<String>,
    /// Flake attr yielding the bootstrap-nonces list. When set, the pipeline
    /// signs `bootstrap-nonces.json` alongside `fleet.resolved.json` via the
    /// same `sign_cmd`. `None` skips the artifact entirely (which means CP
    /// enrolment is unusable in strict mode - only used in dev/test).
    pub bootstrap_nonces_attr: Option<String>,
    /// Source URL for building pinned hosts at non-current commits. Optional
    /// at the type level but required at runtime iff any non-expired host pin
    /// specifies a commit different from the current release commit
    /// (validation in `validate_pin_source_url`).
    pub pin_source_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GitPushTarget {
    pub remote: String,
    pub branch: String,
}

#[derive(Debug)]
pub enum RunOutcome {
    /// `commit_sha` is `Some` when `--git-commit` was set and a commit landed.
    Released {
        commit_sha: Option<String>,
        hosts: Vec<String>,
    },
    /// Closure hashes unchanged; only reachable with `reuse_unchanged_signature`.
    NoChange,
}

pub fn run(config: &ReleaseConfig) -> Result<RunOutcome> {
    validate_config(config)?;

    tracing::info!(
        target: "nixfleet_release",
        flake = %config.flake_dir.display(),
        "release pipeline start",
    );

    let hosts = enumerate_hosts(config)?;
    if hosts.is_empty() {
        bail!("no hosts to release - empty enumeration");
    }
    let host_names: Vec<&str> = hosts.iter().map(|(n, _)| n.as_str()).collect();
    tracing::info!(count = hosts.len(), hosts = ?host_names, "enumerated");

    // Eval BEFORE build: pin metadata branches the build path per host. Safe
    // to reorder - eval is its own nix invocation with no build dependency.
    let mut resolved = eval_fleet_resolved(config)?;
    let current_commit = git_head_sha(&config.flake_dir).ok();
    let now = Utc::now();
    filter_expired_pins(&mut resolved, now);
    validate_pin_source_url(config, &resolved, current_commit.as_deref())?;

    let built = build_hosts(config, &hosts, &resolved, current_commit.as_deref())?;
    tracing::info!(built = built.len(), total = hosts.len(), "build done");

    if let Some(cmd) = &config.push_cmd {
        for (host, path) in built.iter() {
            let hash = closure_hash(path);
            push_one(cmd, host, path, &hash)?;
        }
    }

    let hashes: BTreeMap<String, String> = built
        .iter()
        .map(|(h, p)| (h.clone(), closure_hash(p)))
        .collect();
    inject_closure_hashes(&mut resolved, &hashes);

    let release_path = config.release_dir.join(&config.artifact_name);
    let signature_path = config
        .release_dir
        .join(format!("{}.sig", config.artifact_name));
    let preserved_signed_at: Option<DateTime<Utc>> = if config.reuse_unchanged_signature {
        load_existing_signed_at_if_unchanged(&release_path, &resolved)?
    } else {
        None
    };

    let signed_at = preserved_signed_at.unwrap_or_else(Utc::now);
    let ci_commit = current_commit.clone();
    stamp_meta(
        &mut resolved,
        signed_at,
        ci_commit.clone(),
        &config.signature_algorithm,
    );

    let canonical = canonicalize_resolved(&resolved)?;

    let sig_bytes = if preserved_signed_at.is_some() && signature_path.exists() {
        std::fs::read(&signature_path).context("read existing signature")?
    } else {
        sign(&config.sign_cmd, canonical.as_bytes())?
    };

    if config.smoke_verify {
        smoke_verify(canonical.as_bytes(), &sig_bytes)?;
    }

    write_release(
        &config.release_dir,
        &config.artifact_name,
        canonical.as_bytes(),
        &sig_bytes,
    )?;

    // LOADBEARING: empty list still emits the file. CP-rebuild recovery
    // primes `cert_revocations` from this; a missing file would unlock every
    // revoked cert on rebuild.
    let mut revocations_paths: Vec<PathBuf> = Vec::new();
    if let Some(attr) = &config.revocations_attr {
        let entries = eval_revocations(config, attr)?;
        let revs = Revocations {
            schema_version: 1,
            revocations: entries,
            meta: nixfleet_proto::Meta {
                schema_version: 1,
                signed_at: Some(signed_at),
                ci_commit: ci_commit.clone(),
                signature_algorithm: Some(config.signature_algorithm.clone()),
            },
        };
        let revs_json = serde_json::to_string(&revs).context("serialise revocations.json")?;
        let revs_canonical = nixfleet_canonicalize::canonicalize(&revs_json)
            .context("canonicalize revocations.json")?;
        let revs_sig_path = config.release_dir.join("revocations.json.sig");
        let revs_path = config.release_dir.join("revocations.json");
        // Reuse on-disk signature when canonical bytes match (idempotent).
        let revs_sig_bytes = if revs_path.exists()
            && revs_sig_path.exists()
            && std::fs::read(&revs_path).ok().as_deref() == Some(revs_canonical.as_bytes())
        {
            std::fs::read(&revs_sig_path).context("read existing revocations signature")?
        } else {
            sign(&config.sign_cmd, revs_canonical.as_bytes())?
        };
        write_release(
            &config.release_dir,
            "revocations.json",
            revs_canonical.as_bytes(),
            &revs_sig_bytes,
        )?;
        revocations_paths.push(revs_path);
        revocations_paths.push(revs_sig_path);
        tracing::info!(
            target: "nixfleet_release",
            entries = revs.revocations.len(),
            "revocations.json signed + written",
        );
    }

    // LOADBEARING: empty list still emits the file. CP-rebuild recovery
    // primes the in-memory allowlist from this; a missing file would
    // re-open the replay-after-wipe window for unprocessed nonces.
    let mut bootstrap_nonces_paths: Vec<PathBuf> = Vec::new();
    if let Some(attr) = &config.bootstrap_nonces_attr {
        let raw_entries = eval_bootstrap_nonces(config, attr)?;
        let pruned = prune_expired_bootstrap_nonces(raw_entries, signed_at);
        let bn = nixfleet_proto::BootstrapNonces {
            schema_version: 1,
            bootstrap_nonces: pruned,
            meta: nixfleet_proto::Meta {
                schema_version: 1,
                signed_at: Some(signed_at),
                ci_commit: ci_commit.clone(),
                signature_algorithm: Some(config.signature_algorithm.clone()),
            },
        };
        let bn_json = serde_json::to_string(&bn).context("serialise bootstrap-nonces.json")?;
        let bn_canonical = nixfleet_canonicalize::canonicalize(&bn_json)
            .context("canonicalize bootstrap-nonces.json")?;
        let bn_path = config.release_dir.join("bootstrap-nonces.json");
        let bn_sig_path = config.release_dir.join("bootstrap-nonces.json.sig");
        let bn_sig_bytes = if bn_path.exists()
            && bn_sig_path.exists()
            && std::fs::read(&bn_path).ok().as_deref() == Some(bn_canonical.as_bytes())
        {
            std::fs::read(&bn_sig_path).context("read existing bootstrap-nonces signature")?
        } else {
            sign(&config.sign_cmd, bn_canonical.as_bytes())?
        };
        write_release(
            &config.release_dir,
            "bootstrap-nonces.json",
            bn_canonical.as_bytes(),
            &bn_sig_bytes,
        )?;
        bootstrap_nonces_paths.push(bn_path);
        bootstrap_nonces_paths.push(bn_sig_path);
        tracing::info!(
            target: "nixfleet_release",
            entries = bn.bootstrap_nonces.len(),
            "bootstrap-nonces.json signed + written",
        );
    }

    // One signed manifest per channel; fleetResolvedHash binds each to this
    // snapshot, blocking mix-and-match across rotations.
    let mut manifest_paths: Vec<PathBuf> = Vec::new();
    let fleet_resolved_hash = sha256_hex(canonical.as_bytes());
    let rollouts_dir = config.release_dir.join("rollouts");
    for (channel_name, _channel) in resolved.channels.iter() {
        let manifest = match project_manifest(
            &resolved,
            channel_name,
            &fleet_resolved_hash,
            signed_at,
            ci_commit.as_deref(),
            &config.signature_algorithm,
        )? {
            Some(m) => m,
            None => continue,
        };

        let manifest_json = serde_json::to_string(&manifest)
            .with_context(|| format!("serialise manifest for channel {channel_name}"))?;
        let manifest_canonical = nixfleet_canonicalize::canonicalize(&manifest_json)
            .with_context(|| format!("canonicalize manifest for channel {channel_name}"))?;
        let rollout_id = nixfleet_reconciler::compute_rollout_id(&manifest)
            .with_context(|| format!("compute rolloutId for channel {channel_name}"))?;

        let artifact_name = format!("{rollout_id}.json");
        let manifest_path = rollouts_dir.join(&artifact_name);
        let sig_path = rollouts_dir.join(format!("{artifact_name}.sig"));

        // rolloutId IS the content hash, so identical bytes ⇒ identical path.
        // Reuse on-disk signature when bytes match.
        let sig_bytes = if manifest_path.exists()
            && sig_path.exists()
            && std::fs::read(&manifest_path).ok().as_deref() == Some(manifest_canonical.as_bytes())
        {
            std::fs::read(&sig_path).context("read existing manifest signature")?
        } else {
            sign(&config.sign_cmd, manifest_canonical.as_bytes())?
        };

        write_release(
            &rollouts_dir,
            &artifact_name,
            manifest_canonical.as_bytes(),
            &sig_bytes,
        )?;
        manifest_paths.push(manifest_path);
        manifest_paths.push(sig_path);

        tracing::info!(
            target: "nixfleet_release",
            rollout_id = %rollout_id,
            channel = %channel_name,
            host_count = manifest.host_set.len(),
            "rollout manifest signed + written",
        );
    }

    let mut commit_sha = None;
    if config.git_commit {
        let mut release_files = vec![release_path.clone(), signature_path.clone()];
        release_files.extend(revocations_paths.iter().cloned());
        release_files.extend(bootstrap_nonces_paths.iter().cloned());
        release_files.extend(manifest_paths.iter().cloned());
        let committed =
            git_commit_release(config, &release_files, ci_commit.as_deref(), signed_at)?;
        if let Some(c) = &config.git_push {
            if committed {
                git_push_release(&config.flake_dir, c)?;
            } else {
                tracing::info!("no release change - skip push");
            }
        }
        commit_sha = if committed {
            git_head_sha(&config.flake_dir).ok()
        } else {
            None
        };
        if !committed && preserved_signed_at.is_some() {
            return Ok(RunOutcome::NoChange);
        }
    }

    let host_names: Vec<String> = hashes.keys().cloned().collect();
    Ok(RunOutcome::Released {
        commit_sha,
        hosts: host_names,
    })
}

fn validate_config(c: &ReleaseConfig) -> Result<()> {
    match c.signature_algorithm.as_str() {
        "ed25519" | "ecdsa-p256" => {}
        other => bail!("--signature-algorithm must be 'ed25519' or 'ecdsa-p256', got '{other}'"),
    }
    if c.git_push.is_some() && !c.git_commit {
        bail!("--git-push requires --git-commit");
    }
    if c.sign_cmd.trim().is_empty() {
        bail!("--sign-cmd is required and cannot be empty");
    }
    Ok(())
}

/// `(host, kind)` pairs. NixOS sorted, then Darwin sorted; `Explicit`
/// preserves caller order. Missing attrsets are empty, not errors.
fn enumerate_hosts(config: &ReleaseConfig) -> Result<Vec<(String, HostKind)>> {
    let mut nixos = list_attr_optional(&config.flake_dir, "nixosConfigurations")?;
    nixos.sort();
    nixos.dedup();
    let mut darwin = list_attr_optional(&config.flake_dir, "darwinConfigurations")?;
    darwin.sort();
    darwin.dedup();

    let in_nixos = |n: &str| nixos.iter().any(|h| h == n);
    let in_darwin = |n: &str| darwin.iter().any(|h| h == n);

    Ok(match &config.hosts {
        HostsSpec::Auto => nixos
            .iter()
            .map(|n| (n.clone(), HostKind::Nixos))
            .chain(darwin.iter().map(|n| (n.clone(), HostKind::Darwin)))
            .collect(),
        HostsSpec::AutoExclude(exclude) => {
            let kept_nixos = nixos
                .iter()
                .filter(|h| !exclude.iter().any(|e| e == *h))
                .map(|n| (n.clone(), HostKind::Nixos));
            let kept_darwin = darwin
                .iter()
                .filter(|h| !exclude.iter().any(|e| e == *h))
                .map(|n| (n.clone(), HostKind::Darwin));
            kept_nixos.chain(kept_darwin).collect()
        }
        HostsSpec::Explicit(list) => list
            .iter()
            .map(|n| match (in_nixos(n), in_darwin(n)) {
                (true, false) => Ok((n.clone(), HostKind::Nixos)),
                (false, true) => Ok((n.clone(), HostKind::Darwin)),
                (true, true) => Err(anyhow::anyhow!(
                    "host '{n}' is declared in both nixosConfigurations and \
                     darwinConfigurations - disambiguate before releasing"
                )),
                (false, false) => Err(anyhow::anyhow!(
                    "host '{n}' is in neither nixosConfigurations nor \
                     darwinConfigurations of flake {}",
                    config.flake_dir.display()
                )),
            })
            .collect::<Result<Vec<_>>>()?,
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Strip entries with `expires_at < signed_at`. Run at sign time so the
/// signed artifact only contains the operational set; fleet.nix can keep
/// historical entries as an audit log.
pub(crate) fn prune_expired_bootstrap_nonces(
    entries: Vec<nixfleet_proto::BootstrapNonceEntry>,
    signed_at: DateTime<Utc>,
) -> Vec<nixfleet_proto::BootstrapNonceEntry> {
    entries
        .into_iter()
        .filter(|e| e.expires_at >= signed_at)
        .collect()
}

fn eval_revocations(config: &ReleaseConfig, attr: &str) -> Result<Vec<RevocationEntry>> {
    let output = Command::new("nix")
        .args(["eval", "--json", "--no-warn-dirty", &format!(".#{attr}")])
        .current_dir(&config.flake_dir)
        .output()
        .with_context(|| format!("invoke `nix eval .#{attr}`"))?;
    if !output.status.success() {
        bail!(
            "nix eval .#{attr}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    serde_json::from_slice(&output.stdout)
        .with_context(|| format!("parse revocations from `nix eval .#{attr}`"))
}

fn eval_bootstrap_nonces(
    config: &ReleaseConfig,
    attr: &str,
) -> Result<Vec<nixfleet_proto::BootstrapNonceEntry>> {
    let output = Command::new("nix")
        .args(["eval", "--json", "--no-warn-dirty", &format!(".#{attr}")])
        .current_dir(&config.flake_dir)
        .output()
        .with_context(|| format!("invoke `nix eval .#{attr}`"))?;
    if !output.status.success() {
        bail!(
            "nix eval .#{attr}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    serde_json::from_slice(&output.stdout)
        .with_context(|| format!("parse bootstrap nonces from `nix eval .#{attr}`"))
}

/// Enumerate attribute names; missing attrset -> empty. "Missing attribute"
/// matches a small set of stable nix-eval phrasings.
fn list_attr_optional(flake_dir: &Path, attr_path: &str) -> Result<Vec<String>> {
    let output = Command::new("nix")
        .args([
            "eval",
            "--json",
            "--no-warn-dirty",
            &format!(".#{attr_path}"),
            "--apply",
            "builtins.attrNames",
        ])
        .current_dir(flake_dir)
        .output()
        .with_context(|| format!("invoke `nix eval .#{attr_path}`"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let lowered = stderr.to_lowercase();
        let is_missing = [
            "does not provide attribute",
            "has no attribute",
            "attribute 'darwinconfigurations' missing",
            "attribute 'nixosconfigurations' missing",
        ]
        .iter()
        .any(|needle| lowered.contains(needle));
        if is_missing {
            tracing::debug!(
                attr_path,
                "flake does not declare {attr_path}; treating as empty"
            );
            return Ok(vec![]);
        }
        bail!("nix eval .#{attr_path}: {stderr}");
    }
    let names: Vec<String> = serde_json::from_slice(&output.stdout)
        .with_context(|| format!("parse JSON from `nix eval .#{attr_path}`"))?;
    Ok(names)
}

/// Sequential build. No pin or pin.commit == current_commit -> local build
/// path; non-current commit -> flake-ref build via `<pin_source_url>?rev=<commit>`.
/// Cross-platform builds rely on the operator's `nix.buildMachines`. Failures
/// abort before any push.
fn build_hosts(
    config: &ReleaseConfig,
    hosts: &[(String, HostKind)],
    resolved: &FleetResolved,
    current_commit: Option<&str>,
) -> Result<BTreeMap<String, PathBuf>> {
    let mut out = BTreeMap::new();
    for (host, kind) in hosts {
        let pinned_commit = pin_target_commit(resolved, host, current_commit);
        let path = match pinned_commit {
            None => {
                let attr = format!(
                    ".#{}.{host}.config.system.build.toplevel",
                    kind.attr_prefix()
                );
                build_local(&config.flake_dir, &attr)
                    .with_context(|| format!("build host {host}"))?
            }
            Some(commit) => {
                let url = config.pin_source_url.as_deref().ok_or_else(|| {
                    // Defensive bail; `validate_pin_source_url` catches this
                    // earlier in normal flow.
                    anyhow::anyhow!(
                        "host '{host}' is pinned to commit '{commit}' but \
                         --pin-source-url is unset"
                    )
                })?;
                build_pinned(url, commit, *kind, host)
                    .with_context(|| format!("build pinned host {host} @ {commit}"))?
            }
        };
        tracing::info!(host = %host, kind = ?kind, path, "built");
        out.insert(host.clone(), PathBuf::from(path));
    }
    Ok(out)
}

/// `Some(commit)` iff the host has a pin AND `pin.commit ≠ current_commit`.
/// Same-commit pins return `None` so the local build path handles them.
fn pin_target_commit<'a>(
    resolved: &'a FleetResolved,
    host: &str,
    current_commit: Option<&str>,
) -> Option<&'a str> {
    let pin = resolved.hosts.get(host)?.pin.as_ref()?;
    if Some(pin.commit.as_str()) == current_commit {
        return None;
    }
    Some(pin.commit.as_str())
}

fn build_local(flake_dir: &Path, attr: &str) -> Result<String> {
    let output = Command::new("nix")
        .args([
            "build",
            "--no-link",
            "--print-out-paths",
            "--no-warn-dirty",
            attr,
        ])
        .current_dir(flake_dir)
        .output()
        .with_context(|| format!("invoke `nix build {attr}`"))?;
    interpret_build_output(attr, output)
}

/// Build via flake-ref at a different commit. The `url?rev=<commit>` form lets
/// Nix handle checkout + caching; we don't manage a worktree ourselves.
fn build_pinned(pin_source_url: &str, commit: &str, kind: HostKind, host: &str) -> Result<String> {
    let attr = format!(
        "{pin_source_url}?rev={commit}#{}.{host}.config.system.build.toplevel",
        kind.attr_prefix()
    );
    tracing::info!(
        host = %host,
        commit = %commit,
        url = %pin_source_url,
        "building pinned host via flake-ref",
    );
    let output = Command::new("nix")
        .args([
            "build",
            "--no-link",
            "--print-out-paths",
            "--no-warn-dirty",
            &attr,
        ])
        .output()
        .with_context(|| format!("invoke `nix build {attr}`"))?;
    interpret_build_output(&attr, output)
}

fn interpret_build_output(attr: &str, output: std::process::Output) -> Result<String> {
    if !output.status.success() {
        bail!(
            "nix build {attr}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() {
        bail!("nix build {attr}: empty output");
    }
    Ok(path)
}

/// Drop expired pins (past `expires_at`); affected hosts fall back to the
/// current-commit build path. Pins with no `expires_at` always remain.
pub fn filter_expired_pins(resolved: &mut FleetResolved, now: DateTime<Utc>) {
    for (host_name, host) in resolved.hosts.iter_mut() {
        if let Some(pin) = host.pin.as_ref()
            && let Some(expires) = pin.expires_at
            && expires <= now
        {
            tracing::info!(
                host = %host_name,
                expired_at = %expires,
                commit = %pin.commit,
                "pin expired - falling back to current-commit build",
            );
            host.pin = None;
        }
    }
}

/// Errors when any host has a non-current-commit pin but `--pin-source-url`
/// is unset. Run AFTER `filter_expired_pins` so expired pins aren't counted.
fn validate_pin_source_url(
    config: &ReleaseConfig,
    resolved: &FleetResolved,
    current_commit: Option<&str>,
) -> Result<()> {
    if config.pin_source_url.is_some() {
        return Ok(());
    }
    let needs: Vec<&str> = resolved
        .hosts
        .iter()
        .filter_map(|(name, host)| {
            host.pin.as_ref().and_then(|p| {
                if Some(p.commit.as_str()) != current_commit {
                    Some(name.as_str())
                } else {
                    None
                }
            })
        })
        .collect();
    if !needs.is_empty() {
        bail!(
            "--pin-source-url is required: hosts with non-current-commit pins ({}) \
             can't be built without a source URL",
            needs.join(", ")
        );
    }
    Ok(())
}

fn closure_hash(path: &Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string()
}

fn push_one(cmd: &str, host: &str, path: &Path, closure_hash: &str) -> Result<()> {
    tracing::info!(host = %host, "push hook");
    let status = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .env("NIXFLEET_HOST", host)
        .env("NIXFLEET_PATH", path)
        .env("NIXFLEET_CLOSURE_HASH", closure_hash)
        .status()
        .with_context(|| format!("invoke push hook for {host}"))?;
    if !status.success() {
        bail!(
            "push hook for {host} exited {} ({:?})",
            status.code().unwrap_or(-1),
            cmd,
        );
    }
    Ok(())
}

pub(crate) fn eval_fleet_resolved(config: &ReleaseConfig) -> Result<FleetResolved> {
    let output = Command::new("nix")
        .args([
            "eval",
            "--json",
            "--no-warn-dirty",
            &config.fleet_resolved_attr,
        ])
        .current_dir(&config.flake_dir)
        .output()
        .with_context(|| format!("invoke `nix eval {}`", config.fleet_resolved_attr))?;
    if !output.status.success() {
        bail!(
            "nix eval {}: {}",
            config.fleet_resolved_attr,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let resolved: FleetResolved = serde_json::from_slice(&output.stdout)
        .with_context(|| format!("parse {} as FleetResolved", config.fleet_resolved_attr))?;
    Ok(resolved)
}

/// Sets `hosts[h].closureHash`. Unknown hosts in `hashes` are silently skipped.
pub fn inject_closure_hashes(resolved: &mut FleetResolved, hashes: &BTreeMap<String, String>) {
    for (host, hash) in hashes {
        if let Some(h) = resolved.hosts.get_mut(host) {
            h.closure_hash = Some(hash.clone());
        }
    }
}

pub fn stamp_meta(
    resolved: &mut FleetResolved,
    signed_at: DateTime<Utc>,
    ci_commit: Option<String>,
    signature_algorithm: &str,
) {
    resolved.meta.signed_at = Some(signed_at);
    resolved.meta.ci_commit = ci_commit;
    resolved.meta.signature_algorithm = Some(signature_algorithm.to_string());
}

pub fn canonicalize_resolved(resolved: &FleetResolved) -> Result<String> {
    let raw =
        serde_json::to_string(resolved).context("serialize FleetResolved before canonicalize")?;
    nixfleet_canonicalize::canonicalize(&raw).context("canonicalize fleet.resolved")
}

/// Returns existing `meta.signedAt` when on-disk closure hashes match.
fn load_existing_signed_at_if_unchanged(
    release_path: &Path,
    resolved: &FleetResolved,
) -> Result<Option<DateTime<Utc>>> {
    if !release_path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(release_path)
        .with_context(|| format!("read existing release {}", release_path.display()))?;
    let existing: FleetResolved =
        serde_json::from_str(&raw).context("parse existing release file")?;

    let cur_hashes: BTreeMap<&str, Option<&str>> = resolved
        .hosts
        .iter()
        .map(|(k, v)| (k.as_str(), v.closure_hash.as_deref()))
        .collect();
    let prev_hashes: BTreeMap<&str, Option<&str>> = existing
        .hosts
        .iter()
        .map(|(k, v)| (k.as_str(), v.closure_hash.as_deref()))
        .collect();

    if cur_hashes == prev_hashes {
        Ok(existing.meta.signed_at)
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod bootstrap_nonces_tests {
    use super::*;
    use nixfleet_proto::BootstrapNonceEntry;

    fn entry(nonce: &str, expires_at: &str) -> BootstrapNonceEntry {
        BootstrapNonceEntry {
            nonce: nonce.into(),
            hostname: "agent-01".into(),
            expires_at: expires_at.parse().unwrap(),
            minted_at: None,
            minted_by: None,
        }
    }

    #[test]
    fn prune_drops_entries_with_expires_at_before_signed_at() {
        let signed_at: DateTime<Utc> = "2026-05-13T10:00:00Z".parse().unwrap();
        let entries = vec![
            entry("expired", "2026-05-12T10:00:00Z"),
            entry("fresh", "2026-05-14T10:00:00Z"),
            entry("exactly-now", "2026-05-13T10:00:00Z"),
        ];
        let kept = prune_expired_bootstrap_nonces(entries, signed_at);
        let nonces: Vec<&str> = kept.iter().map(|e| e.nonce.as_str()).collect();
        // expiresAt < signedAt is dropped; expiresAt == signedAt is kept
        // (still has zero seconds of validity at signing instant; CP will
        // reject when it sees it at a later wall-clock moment).
        assert_eq!(nonces, vec!["fresh", "exactly-now"]);
    }

    #[test]
    fn prune_empty_list_is_empty() {
        let signed_at: DateTime<Utc> = "2026-05-13T10:00:00Z".parse().unwrap();
        let kept = prune_expired_bootstrap_nonces(vec![], signed_at);
        assert!(kept.is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nixfleet_proto::{Channel, Compliance, Host, Meta};

    fn dummy_resolved() -> FleetResolved {
        let mut hosts = std::collections::HashMap::new();
        hosts.insert(
            "test-host".to_string(),
            Host {
                system: "x86_64-linux".into(),
                tags: vec![],
                channel: "stable".into(),
                closure_hash: None,
                pubkey: None,
                pin: None,
            },
        );
        hosts.insert(
            "host-03".to_string(),
            Host {
                system: "aarch64-darwin".into(),
                tags: vec![],
                channel: "stable".into(),
                closure_hash: None,
                pubkey: None,
                pin: None,
            },
        );
        let mut channels = std::collections::HashMap::new();
        channels.insert(
            "stable".to_string(),
            Channel {
                rollout_policy: "default".into(),
                reconcile_interval_minutes: 5,
                freshness_window: 60,
                signing_interval_minutes: 30,
                compliance: Compliance {
                    frameworks: vec![],
                    mode: "disabled".to_string(),
                },
            },
        );
        FleetResolved {
            schema_version: 1,
            hosts,
            channels,
            rollout_policies: Default::default(),
            waves: Default::default(),
            edges: vec![],
            channel_edges: vec![],
            disruption_budgets: vec![],
            meta: Meta {
                schema_version: 1,
                signed_at: None,
                ci_commit: None,
                signature_algorithm: Some("ed25519".into()),
            },
        }
    }

    #[test]
    fn inject_sets_closure_hash_for_known_hosts_and_skips_unknown() {
        let mut r = dummy_resolved();
        let mut hashes = BTreeMap::new();
        hashes.insert(
            "test-host".to_string(),
            "abc123-nixos-system-test-host".to_string(),
        );
        hashes.insert("ghost".to_string(), "should-be-ignored".to_string());
        inject_closure_hashes(&mut r, &hashes);
        assert_eq!(
            r.hosts["test-host"].closure_hash.as_deref(),
            Some("abc123-nixos-system-test-host")
        );
        assert!(r.hosts["host-03"].closure_hash.is_none());
        assert!(!r.hosts.contains_key("ghost"));
    }

    #[test]
    fn stamp_meta_writes_three_fields() {
        let mut r = dummy_resolved();
        let ts = DateTime::parse_from_rfc3339("2026-04-27T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        stamp_meta(&mut r, ts, Some("deadbeef".into()), "ed25519");
        assert_eq!(r.meta.signed_at, Some(ts));
        assert_eq!(r.meta.ci_commit.as_deref(), Some("deadbeef"));
        assert_eq!(r.meta.signature_algorithm.as_deref(), Some("ed25519"));
    }

    #[test]
    fn canonicalize_round_trip_is_byte_stable() {
        let mut r = dummy_resolved();
        let ts = DateTime::parse_from_rfc3339("2026-04-27T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        stamp_meta(&mut r, ts, Some("deadbeef".into()), "ed25519");
        let c1 = canonicalize_resolved(&r).unwrap();
        let parsed: FleetResolved = serde_json::from_str(&c1).unwrap();
        let c2 = canonicalize_resolved(&parsed).unwrap();
        assert_eq!(c1, c2);
    }

    #[test]
    fn render_commit_message_substitutes() {
        let ts = DateTime::parse_from_rfc3339("2026-04-27T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let m = render_commit_message(
            "chore(ci): release {sha:0:8} [skip ci]",
            "deadbeefcafebabe",
            ts,
        );
        assert_eq!(m, "chore(ci): release deadbeef [skip ci]");

        let m2 = render_commit_message("ts={ts}, sha={sha}", "abc", ts);
        assert_eq!(m2, "ts=2026-04-27T12:00:00+00:00, sha=abc");
    }

    fn manifest_resolved() -> FleetResolved {
        use nixfleet_proto::{HealthGate, PolicyWave, RolloutPolicy, Selector, Wave};
        let mut hosts = std::collections::HashMap::new();
        hosts.insert(
            "agent-02".to_string(),
            Host {
                system: "x86_64-linux".into(),
                tags: vec![],
                channel: "stable".into(),
                closure_hash: Some("aaaa-host-b".into()),
                pubkey: None,
                pin: None,
            },
        );
        hosts.insert(
            "agent-01".to_string(),
            Host {
                system: "x86_64-linux".into(),
                tags: vec![],
                channel: "stable".into(),
                closure_hash: Some("aaaa-host-a".into()),
                pubkey: None,
                pin: None,
            },
        );
        hosts.insert(
            "agent-no-closure".to_string(),
            Host {
                system: "x86_64-linux".into(),
                tags: vec![],
                channel: "stable".into(),
                closure_hash: None,
                pubkey: None,
                pin: None,
            },
        );
        let mut channels = std::collections::HashMap::new();
        channels.insert(
            "stable".to_string(),
            Channel {
                rollout_policy: "default".into(),
                reconcile_interval_minutes: 5,
                freshness_window: 60,
                signing_interval_minutes: 30,
                compliance: Compliance {
                    frameworks: vec!["anssi-bp028".into()],
                    mode: "permissive".to_string(),
                },
            },
        );
        let mut rollout_policies = std::collections::HashMap::new();
        rollout_policies.insert(
            "default".to_string(),
            RolloutPolicy {
                strategy: "waves".into(),
                waves: vec![PolicyWave {
                    selector: Selector {
                        tags: vec![],
                        tags_any: vec![],
                        hosts: vec![],
                        channel: None,
                        all: true,
                    },
                    soak_minutes: 5,
                }],
                health_gate: HealthGate::default(),
                on_health_failure: nixfleet_proto::OnHealthFailure::Halt,
            },
        );
        let mut waves = std::collections::HashMap::new();
        waves.insert(
            "stable".to_string(),
            vec![
                Wave {
                    hosts: vec!["agent-01".into()],
                    soak_minutes: 5,
                },
                Wave {
                    hosts: vec!["agent-02".into()],
                    soak_minutes: 5,
                },
            ],
        );
        FleetResolved {
            schema_version: 1,
            hosts,
            channels,
            rollout_policies,
            waves,
            edges: vec![],
            channel_edges: vec![],
            disruption_budgets: vec![],
            meta: Meta {
                schema_version: 1,
                signed_at: None,
                ci_commit: None,
                signature_algorithm: Some("ed25519".into()),
            },
        }
    }

    #[test]
    fn project_manifest_emits_sorted_host_set_with_correct_wave_indices() {
        let r = manifest_resolved();
        let ts = DateTime::parse_from_rfc3339("2026-04-30T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let m = project_manifest(&r, "stable", "feedface", ts, Some("def45678"), "ed25519")
            .unwrap()
            .expect("non-empty manifest");
        assert_eq!(m.host_set[0].hostname, "agent-01");
        assert_eq!(m.host_set[1].hostname, "agent-02");
        assert_eq!(m.host_set.len(), 2);
        assert_eq!(m.host_set[0].wave_index, 0);
        assert_eq!(m.host_set[1].wave_index, 1);
        assert_eq!(m.host_set[0].target_closure, "aaaa-host-a");
        assert_eq!(m.host_set[1].target_closure, "aaaa-host-b");
        assert_eq!(m.fleet_resolved_hash, "feedface");
        assert_eq!(m.display_name, "stable@def45678");
        assert_eq!(m.channel_ref, "def45678");
        assert_eq!(m.meta.signed_at, Some(ts));
        assert_eq!(m.compliance_frameworks, vec!["anssi-bp028".to_string()]);
    }

    #[test]
    fn project_manifest_returns_none_when_no_host_has_closure_hash() {
        let r = dummy_resolved();
        let mut r = r;
        r.rollout_policies.insert(
            "default".to_string(),
            nixfleet_proto::RolloutPolicy {
                strategy: "waves".into(),
                waves: vec![],
                health_gate: nixfleet_proto::HealthGate::default(),
                on_health_failure: nixfleet_proto::OnHealthFailure::Halt,
            },
        );
        let ts = Utc::now();
        let result = project_manifest(&r, "stable", "deadbeef", ts, None, "ed25519").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn project_manifest_errors_on_missing_channel() {
        let r = manifest_resolved();
        let ts = Utc::now();
        let err = project_manifest(&r, "ghost", "feedface", ts, None, "ed25519").unwrap_err();
        assert!(err.to_string().contains("channel ghost"));
    }

    #[test]
    fn sha256_hex_is_64_char_lowercase() {
        let h = sha256_hex(b"hello world");
        assert_eq!(h.len(), 64);
        assert!(
            h.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert_eq!(
            h,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn validate_rejects_bad_algorithm() {
        let mut c = base_config();
        c.signature_algorithm = "rsa".into();
        let err = validate_config(&c).unwrap_err();
        assert!(err.to_string().contains("signature-algorithm"));
    }

    #[test]
    fn validate_rejects_push_without_commit() {
        let mut c = base_config();
        c.git_push = Some(GitPushTarget {
            remote: "origin".into(),
            branch: "main".into(),
        });
        c.git_commit = false;
        let err = validate_config(&c).unwrap_err();
        assert!(err.to_string().contains("--git-commit"));
    }

    fn base_config() -> ReleaseConfig {
        ReleaseConfig {
            flake_dir: PathBuf::from("."),
            fleet_resolved_attr: ".#fleet.resolved".into(),
            hosts: HostsSpec::Auto,
            push_cmd: None,
            sign_cmd: "true".into(),
            signature_algorithm: "ed25519".into(),
            release_dir: PathBuf::from("releases"),
            artifact_name: "fleet.resolved.json".into(),
            git_commit: false,
            git_push: None,
            commit_template: "release {sha:0:8}".into(),
            git_user_name: None,
            git_user_email: None,
            smoke_verify: true,
            reuse_unchanged_signature: false,
            revocations_attr: None,
            bootstrap_nonces_attr: None,
            pin_source_url: None,
        }
    }

    #[test]
    fn host_kind_attr_prefix_matches_flake_convention() {
        assert_eq!(HostKind::Nixos.attr_prefix(), "nixosConfigurations");
        assert_eq!(HostKind::Darwin.attr_prefix(), "darwinConfigurations");
    }

    fn pin_resolved(host_pin: Option<nixfleet_proto::Pin>) -> FleetResolved {
        let mut r = dummy_resolved();
        r.hosts.get_mut("test-host").unwrap().pin = host_pin;
        r
    }

    fn fresh_pin(commit: &str, expires_at: Option<DateTime<Utc>>) -> nixfleet_proto::Pin {
        nixfleet_proto::Pin {
            commit: commit.to_string(),
            reason: "test".to_string(),
            expires_at,
        }
    }

    #[test]
    fn filter_expired_drops_past_expiry_and_keeps_future() {
        let now = Utc::now();
        let past = now - chrono::Duration::days(1);
        let future = now + chrono::Duration::days(1);

        let mut r = pin_resolved(Some(fresh_pin("c1", Some(past))));
        filter_expired_pins(&mut r, now);
        assert!(
            r.hosts["test-host"].pin.is_none(),
            "expired pin must be dropped",
        );

        let mut r = pin_resolved(Some(fresh_pin("c2", Some(future))));
        filter_expired_pins(&mut r, now);
        assert!(r.hosts["test-host"].pin.is_some(), "fresh pin must survive",);

        let mut r = pin_resolved(Some(fresh_pin("c3", None)));
        filter_expired_pins(&mut r, now);
        assert!(
            r.hosts["test-host"].pin.is_some(),
            "pin with no expiry must always survive",
        );
    }

    #[test]
    fn filter_expired_treats_exact_now_as_expired() {
        // `<=` boundary: window-closes-at-instant means now == expiresAt drops.
        let now = Utc::now();
        let mut r = pin_resolved(Some(fresh_pin("c", Some(now))));
        filter_expired_pins(&mut r, now);
        assert!(r.hosts["test-host"].pin.is_none());
    }

    #[test]
    fn pin_target_commit_none_for_unpinned_host() {
        let r = pin_resolved(None);
        assert!(pin_target_commit(&r, "test-host", Some("abc")).is_none());
    }

    #[test]
    fn pin_target_commit_none_when_pin_matches_release_commit() {
        let r = pin_resolved(Some(fresh_pin("abc1234", None)));
        assert!(
            pin_target_commit(&r, "test-host", Some("abc1234")).is_none(),
            "same-commit pin must hit the local build path, not flake-ref",
        );
    }

    #[test]
    fn pin_target_commit_some_when_pin_diverges_from_release_commit() {
        let r = pin_resolved(Some(fresh_pin("frozen-abc", None)));
        assert_eq!(
            pin_target_commit(&r, "test-host", Some("current-def")),
            Some("frozen-abc"),
        );
    }

    #[test]
    fn validate_pin_source_url_errors_when_needed_and_unset() {
        let mut c = base_config();
        c.pin_source_url = None;
        let r = pin_resolved(Some(fresh_pin("frozen-abc", None)));
        let err = validate_pin_source_url(&c, &r, Some("current-def")).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("--pin-source-url is required"),
            "error must mention the missing flag: {msg}",
        );
        assert!(
            msg.contains("test-host"),
            "error must list the offending host: {msg}",
        );
    }

    #[test]
    fn validate_pin_source_url_ok_when_no_pins() {
        let c = base_config();
        let r = dummy_resolved();
        validate_pin_source_url(&c, &r, Some("any-commit")).unwrap();
    }

    #[test]
    fn validate_pin_source_url_ok_when_pin_matches_release_commit() {
        let c = base_config();
        let r = pin_resolved(Some(fresh_pin("matching-commit", None)));
        validate_pin_source_url(&c, &r, Some("matching-commit")).unwrap();
    }

    #[test]
    fn validate_pin_source_url_ok_when_url_is_set() {
        let mut c = base_config();
        c.pin_source_url = Some("git+ssh://example/fleet".into());
        let r = pin_resolved(Some(fresh_pin("frozen-abc", None)));
        validate_pin_source_url(&c, &r, Some("current-def")).unwrap();
    }
}
