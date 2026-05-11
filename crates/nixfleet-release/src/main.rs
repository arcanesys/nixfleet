//! `nixfleet-release` binary. Exit: 0 ok / NoChange, 1 config-or-build,
//! 2 push-or-sign-hook, 3 smoke-verify.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use nixfleet_release::{GitPushTarget, HostsSpec, ReleaseConfig, RunOutcome};

#[derive(Parser, Debug)]
#[command(
    name = "nixfleet-release",
    about = "Produce a signed fleet.resolved.json release"
)]
struct Cli {
    /// `auto` | `auto:exclude=foo,bar` | comma-separated explicit list.
    #[arg(long, default_value = "auto")]
    hosts: String,

    #[arg(long, default_value = ".")]
    build_flake: PathBuf,

    #[arg(long, default_value = ".#fleet.resolved")]
    fleet_resolved_attr: String,

    /// Env: NIXFLEET_HOST, NIXFLEET_PATH, NIXFLEET_CLOSURE_HASH.
    #[arg(long, env = "NIXFLEET_PUSH_CMD")]
    push_cmd: Option<String>,

    /// Required. Env: NIXFLEET_INPUT (canonical bytes), NIXFLEET_OUTPUT
    /// (where the hook writes the raw signature).
    #[arg(long, env = "NIXFLEET_SIGN_CMD")]
    sign_cmd: String,

    /// `ed25519` | `ecdsa-p256`.
    #[arg(long, default_value = "ed25519", env = "NIXFLEET_SIGNATURE_ALGORITHM")]
    signature_algorithm: String,

    #[arg(long, default_value = "releases")]
    release_dir: PathBuf,

    /// Signature is `<name>.sig`.
    #[arg(long, default_value = "fleet.resolved.json")]
    artifact_name: String,

    #[arg(long)]
    git_commit: bool,

    /// `<remote>:<branch>`. Implies `--git-commit`.
    #[arg(long, value_name = "REMOTE:BRANCH")]
    git_push: Option<String>,

    /// Substitutions: `{sha}`, `{sha:0:8}`, `{ts}`.
    #[arg(long, default_value = "chore(ci): release {sha:0:8} [skip ci]")]
    commit_template: String,

    #[arg(long, env = "NIXFLEET_GIT_USER_NAME")]
    git_user_name: Option<String>,

    #[arg(long, env = "NIXFLEET_GIT_USER_EMAIL")]
    git_user_email: Option<String>,

    /// Structural smoke verify; default on.
    #[arg(long = "smoke-verify", default_value_t = true, action = clap::ArgAction::Set)]
    smoke_verify: bool,

    /// Reuse `meta.signedAt` when closureHashes match.
    #[arg(long)]
    reuse_unchanged_signature: bool,

    /// Flake attr yielding the revocations list. Unset → no artifact.
    #[arg(long)]
    revocations_attr: Option<String>,

    /// Source URL the pinned-host build path uses as `nix build "<url>?rev=<commit>#..."`.
    /// Required iff any non-expired host pin specifies a commit different from
    /// the current release commit (issue #88). Typical: `git+ssh://lab:222/abstracts33d/fleet`.
    #[arg(long)]
    pin_source_url: Option<String>,

    /// `pretty` | `json`.
    #[arg(long, default_value = "pretty")]
    log_format: String,
}

fn parse_hosts_spec(spec: &str) -> Result<HostsSpec, String> {
    if spec == "auto" {
        return Ok(HostsSpec::Auto);
    }
    if let Some(rest) = spec.strip_prefix("auto:exclude=") {
        let exc: Vec<String> = rest
            .split(',')
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
        return Ok(HostsSpec::AutoExclude(exc));
    }
    let list: Vec<String> = spec
        .split(',')
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    if list.is_empty() {
        return Err("hosts spec is empty".into());
    }
    Ok(HostsSpec::Explicit(list))
}

fn parse_git_push(s: &str) -> Result<GitPushTarget, String> {
    let (remote, branch) = s
        .split_once(':')
        .ok_or_else(|| format!("--git-push expects REMOTE:BRANCH, got {s:?}"))?;
    Ok(GitPushTarget {
        remote: remote.to_string(),
        branch: branch.to_string(),
    })
}

fn init_tracing(format: &str) {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,nixfleet_release=info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    match format {
        "json" => {
            let _ = builder.json().try_init();
        }
        _ => {
            let _ = builder.try_init();
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(&cli.log_format);

    let hosts = match parse_hosts_spec(&cli.hosts) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("nixfleet-release: --hosts: {e}");
            return ExitCode::from(1);
        }
    };

    let git_push = match cli.git_push.as_deref().map(parse_git_push) {
        Some(Ok(t)) => Some(t),
        Some(Err(e)) => {
            eprintln!("nixfleet-release: {e}");
            return ExitCode::from(1);
        }
        None => None,
    };

    let git_commit = cli.git_commit || git_push.is_some();

    let config = ReleaseConfig {
        flake_dir: cli.build_flake,
        fleet_resolved_attr: cli.fleet_resolved_attr,
        hosts,
        push_cmd: cli.push_cmd,
        sign_cmd: cli.sign_cmd,
        signature_algorithm: cli.signature_algorithm,
        release_dir: cli.release_dir,
        artifact_name: cli.artifact_name,
        git_commit,
        git_push,
        commit_template: cli.commit_template,
        git_user_name: cli.git_user_name,
        git_user_email: cli.git_user_email,
        smoke_verify: cli.smoke_verify,
        reuse_unchanged_signature: cli.reuse_unchanged_signature,
        revocations_attr: cli.revocations_attr,
        pin_source_url: cli.pin_source_url,
    };

    match nixfleet_release::run(&config) {
        Ok(RunOutcome::Released { commit_sha, hosts }) => {
            tracing::info!(
                hosts = hosts.len(),
                commit = commit_sha.as_deref().unwrap_or("(none)"),
                "release ok"
            );
            ExitCode::SUCCESS
        }
        Ok(RunOutcome::NoChange) => {
            tracing::info!("no release change");
            ExitCode::SUCCESS
        }
        Err(err) => {
            // Classify by message keyword for CI alerting.
            let msg = format!("{err:#}");
            let exit = if msg.contains("smoke verify") {
                3
            } else if msg.contains("sign hook") || msg.contains("push hook") {
                2
            } else {
                1
            };
            eprintln!("nixfleet-release: {msg}");
            ExitCode::from(exit)
        }
    }
}
