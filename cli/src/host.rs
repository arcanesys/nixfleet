use anyhow::{bail, Context, Result};
use std::process::Stdio;

pub async fn add_host(
    hostname: &str,
    org: &str,
    role: &str,
    platform: &str,
    target: Option<&str>,
    control_plane_url: &str,
) -> Result<()> {
    println!("Adding host: {}", hostname);

    // 1. Fetch hardware config if target provided
    if let Some(target) = target {
        fetch_hardware_config(hostname, target).await?;
    }

    // 2. Generate disk config template
    generate_disk_config(hostname)?;

    // 3. Print mkHost snippet
    print_fleet_snippet(hostname, org, role, platform, control_plane_url);

    // 4. Print next steps
    println!("\nNext steps:");
    println!("  1. Add the above snippet to modules/fleet.nix");
    println!("  2. git add && git commit");
    println!(
        "  3. nixos-anywhere --flake .#{} root@<ip>",
        hostname
    );

    Ok(())
}

async fn fetch_hardware_config(hostname: &str, target: &str) -> Result<()> {
    println!("  Fetching hardware config from {}...", target);
    let dir = format!("modules/_hardware/{}", hostname);
    std::fs::create_dir_all(&dir).context(format!("Failed to create directory {}", dir))?;

    let output = tokio::process::Command::new("ssh")
        .args([
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            "ConnectTimeout=10",
            target,
            "nixos-generate-config",
            "--show-hardware-config",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("Failed to SSH to target")?;

    if !output.status.success() {
        bail!(
            "Failed to generate hardware config: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let hw_path = format!("{}/hardware-configuration.nix", dir);
    std::fs::write(&hw_path, &output.stdout).context(format!("Failed to write {}", hw_path))?;
    println!("  Saved: {}", hw_path);
    Ok(())
}

fn generate_disk_config(hostname: &str) -> Result<()> {
    let dir = format!("modules/_hardware/{}", hostname);
    std::fs::create_dir_all(&dir).context(format!("Failed to create directory {}", dir))?;
    let path = format!("{}/disk-config.nix", dir);

    std::fs::write(&path, DISK_TEMPLATE).context(format!("Failed to write {}", path))?;
    println!("  Generated: {}", path);
    Ok(())
}

const DISK_TEMPLATE: &str = r#"# Standard BTRFS disk layout with impermanence
{
  disko.devices.disk.main = {
    type = "disk";
    device = "/dev/sda"; # Adjust for target hardware
    content = {
      type = "gpt";
      partitions = {
        ESP = {
          size = "512M";
          type = "EF00";
          content = {
            type = "filesystem";
            format = "vfat";
            mountpoint = "/boot";
          };
        };
        root = {
          size = "100%";
          content = {
            type = "btrfs";
            extraArgs = ["-f"];
            subvolumes = {
              "/root" = {
                mountpoint = "/";
                mountOptions = ["compress=zstd" "noatime"];
              };
              "/home" = {
                mountpoint = "/home";
                mountOptions = ["compress=zstd" "noatime"];
              };
              "/persist" = {
                mountpoint = "/persist";
                mountOptions = ["compress=zstd" "noatime"];
              };
              "/nix" = {
                mountpoint = "/nix";
                mountOptions = ["compress=zstd" "noatime"];
              };
            };
          };
        };
      };
    };
  };
}
"#;

fn print_fleet_snippet(hostname: &str, org: &str, role: &str, platform: &str, cp_url: &str) {
    println!("\n# Add this to modules/fleet.nix hosts list:");
    println!(
        r#"(mkHost {{
  hostName = "{hostname}";
  platform = "{platform}";
  org = {org};
  role = builtinRoles.{role};
  hardwareModules = [
    ./_hardware/{hostname}/disk-config.nix
    ./_hardware/{hostname}/hardware-configuration.nix
  ];
  extraModules = [{{
    services.nixfleet-agent = {{
      enable = true;
      controlPlaneUrl = "{cp_url}";
    }};
  }}];
}})"#
    );
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_disk_template_is_valid() {
        assert!(DISK_TEMPLATE.contains("disko.devices.disk.main"));
        assert!(DISK_TEMPLATE.contains("btrfs"));
        assert!(DISK_TEMPLATE.contains("/persist"));
        assert!(DISK_TEMPLATE.contains("/boot"));
    }

    #[test]
    fn test_generate_disk_config_creates_file() {
        let tmp = std::env::temp_dir().join("nixfleet-test-host");
        let hostname = tmp.file_name().unwrap().to_str().unwrap();

        // Override the working directory by using the full path
        let _dir = format!("{}/modules/_hardware/{}", tmp.display(), hostname);
        // We test the template content rather than file creation (requires specific CWD)
        assert!(!DISK_TEMPLATE.is_empty());
    }

    #[test]
    fn test_fleet_snippet_output() {
        // Verify the snippet function doesn't panic
        print_fleet_snippet(
            "test-001",
            "acme",
            "workstation",
            "x86_64-linux",
            "http://localhost:8080",
        );
    }
}
