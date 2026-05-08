//! Layered config resolution: file < env < flag.

use std::path::PathBuf;

use nixfleet_cli::config::{
    default_config_path, load_file, resolve, ConfigError, FileConfig, Overrides,
};
use tempfile::TempDir;

#[test]
fn file_round_trips_through_toml() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    let written = FileConfig {
        cp_url: Some("https://cp.example.com:8080".into()),
        ca_cert: Some(PathBuf::from("/etc/nixfleet/ca.pem")),
        client_cert: Some(PathBuf::from("/etc/nixfleet/op.pem")),
        client_key: Some(PathBuf::from("/etc/nixfleet/op.key")),
    };
    written.save(&path).unwrap();

    let parsed = load_file(&path).unwrap();
    assert_eq!(parsed.cp_url.as_deref(), Some("https://cp.example.com:8080"));
    assert_eq!(parsed.ca_cert.as_deref(), Some(std::path::Path::new("/etc/nixfleet/ca.pem")));
}

#[test]
fn flags_override_env_override_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    FileConfig {
        cp_url: Some("https://from-file:8080".into()),
        ca_cert: Some(PathBuf::from("/file/ca.pem")),
        client_cert: Some(PathBuf::from("/file/cert.pem")),
        client_key: Some(PathBuf::from("/file/key.pem")),
    }
    .save(&path)
    .unwrap();

    let overrides = Overrides {
        cp_url: Some("https://from-flag:8080".into()),
        ca_cert: None,
        client_cert: None,
        client_key: None,
    };
    let env = Overrides {
        cp_url: Some("https://from-env:8080".into()),
        ca_cert: Some(PathBuf::from("/env/ca.pem")),
        client_cert: None,
        client_key: None,
    };
    let resolved = resolve(Some(&path), &env, &overrides).unwrap();
    assert_eq!(resolved.cp_url, "https://from-flag:8080"); // flag wins
    assert_eq!(resolved.ca_cert, std::path::Path::new("/env/ca.pem")); // env wins over file
    assert_eq!(resolved.client_cert, std::path::Path::new("/file/cert.pem")); // file wins when no override
}

#[test]
fn missing_field_returns_structured_error() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    FileConfig {
        cp_url: Some("https://cp:8080".into()),
        ca_cert: None,
        client_cert: None,
        client_key: None,
    }
    .save(&path)
    .unwrap();
    let err = resolve(Some(&path), &Overrides::default(), &Overrides::default()).unwrap_err();
    match err {
        ConfigError::Missing { field } => assert_eq!(field, "ca_cert"),
        other => panic!("expected Missing, got {other:?}"),
    }
}

#[test]
fn missing_file_treated_as_empty_layer() {
    let resolved = resolve(
        Some(std::path::Path::new("/nonexistent/path/that/should/not/exist.toml")),
        &Overrides {
            cp_url: Some("https://cp:8080".into()),
            ca_cert: Some(PathBuf::from("/x/ca.pem")),
            client_cert: Some(PathBuf::from("/x/cert.pem")),
            client_key: Some(PathBuf::from("/x/key.pem")),
        },
        &Overrides::default(),
    )
    .unwrap();
    assert_eq!(resolved.cp_url, "https://cp:8080");
}

#[test]
fn default_path_under_xdg_config_home() {
    // Just verify the function returns *some* path under $HOME or
    // $XDG_CONFIG_HOME. Don't assert exact, since that depends on the env.
    let p = default_config_path();
    let s = p.to_string_lossy();
    assert!(
        s.ends_with("nixfleet/config.toml"),
        "expected path to end with nixfleet/config.toml, got {s}",
    );
}

#[test]
fn run_config_init_writes_then_round_trips() {
    use nixfleet_cli::run_config_init;
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("nixfleet").join("config.toml");
    let written = run_config_init(
        &path,
        "https://cp:8080".into(),
        PathBuf::from("/x/ca.pem"),
        PathBuf::from("/x/cert.pem"),
        PathBuf::from("/x/key.pem"),
        false,
    )
    .unwrap();
    assert_eq!(written, path);
    assert!(path.exists());
    let reloaded = load_file(&path).unwrap();
    assert_eq!(reloaded.cp_url.as_deref(), Some("https://cp:8080"));
}

#[test]
fn run_config_init_refuses_overwrite_without_force() {
    use nixfleet_cli::run_config_init;
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "# pre-existing").unwrap();
    let err = run_config_init(
        &path,
        "https://cp:8080".into(),
        PathBuf::from("/x/ca.pem"),
        PathBuf::from("/x/cert.pem"),
        PathBuf::from("/x/key.pem"),
        false,
    )
    .unwrap_err();
    assert!(err.to_string().contains("already exists"));
    // File untouched
    let body = std::fs::read_to_string(&path).unwrap();
    assert_eq!(body, "# pre-existing");
}

#[test]
fn run_config_init_overwrites_with_force() {
    use nixfleet_cli::run_config_init;
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "# pre-existing").unwrap();
    run_config_init(
        &path,
        "https://cp:8080".into(),
        PathBuf::from("/x/ca.pem"),
        PathBuf::from("/x/cert.pem"),
        PathBuf::from("/x/key.pem"),
        true,
    )
    .unwrap();
    let reloaded = load_file(&path).unwrap();
    assert_eq!(reloaded.cp_url.as_deref(), Some("https://cp:8080"));
}
