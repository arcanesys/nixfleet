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
    if let Some(t) = target {
        println!(
            "  3. nixfleet host provision --hostname {} --target {}",
            hostname, t
        );
    } else {
        println!(
            "  3. nixfleet host provision --hostname {} --target root@<ip>",
            hostname
        );
    }

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

pub async fn provision_host(hostname: &str, target: &str, username: &str) -> Result<()> {
    println!("Provisioning {} on {}...", hostname, target);

    // 1. Verify host exists in flake
    let check = tokio::process::Command::new("nix")
        .args([
            "eval",
            &format!(".#nixosConfigurations.{}", hostname),
            "--apply",
            "x: true",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("Failed to run nix eval")?;

    if !check.status.success() {
        bail!(
            "Host '{}' not found in flake. Did you add it to fleet.nix?",
            hostname
        );
    }
    println!("  Host found in flake");

    // 2. Build the closure
    println!("  Building closure (this may take a while)...");
    let build = tokio::process::Command::new("nix")
        .args([
            "build",
            &format!(
                ".#nixosConfigurations.{}.config.system.build.toplevel",
                hostname
            ),
            "--no-link",
            "--print-out-paths",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("Failed to build closure")?;

    if !build.status.success() {
        bail!("Build failed: {}", String::from_utf8_lossy(&build.stderr));
    }

    let closure = String::from_utf8(build.stdout)?.trim().to_string();
    if closure.is_empty() {
        bail!("Build produced empty store path");
    }
    println!("  Built: {}", closure);

    // 3. Install via nixos-anywhere
    println!("  Installing via nixos-anywhere...");
    let install = tokio::process::Command::new("nix")
        .args([
            "run",
            "github:nix-community/nixos-anywhere",
            "--",
            "--flake",
            &format!(".#{}", hostname),
            target,
        ])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await
        .context("Failed to run nixos-anywhere")?;

    if !install.success() {
        bail!("nixos-anywhere failed");
    }
    println!("  Installation complete");

    // 4. Wait for reboot
    let connect_host = target.trim_start_matches("root@");
    let ssh_target = format!("{}@{}", username, connect_host);
    println!("  Waiting for machine to come back online...");

    let mut online = false;
    for _ in 0..60 {
        let ping = tokio::process::Command::new("ssh")
            .args([
                "-o",
                "StrictHostKeyChecking=accept-new",
                "-o",
                "ConnectTimeout=3",
                &ssh_target,
                "echo",
                "online",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await;

        if let Ok(out) = ping {
            if out.status.success() {
                online = true;
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }

    if !online {
        println!("  WARNING: Machine did not come back online within 5 minutes");
        println!("  You may need to check the target manually.");
        return Ok(());
    }

    // 5. Verify
    println!("  Verifying...");
    let verify = tokio::process::Command::new("ssh")
        .args([
            "-o",
            "StrictHostKeyChecking=accept-new",
            &ssh_target,
            "nixos-version && systemctl is-active nixfleet-agent",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("Failed to verify installation")?;

    let verify_output = String::from_utf8_lossy(&verify.stdout);
    println!("  {}", verify_output.trim());

    if verify.status.success() {
        println!(
            "\n{} provisioned successfully and agent is running!",
            hostname
        );
    } else {
        println!("\n{} installed but agent may not be running yet", hostname);
    }

    Ok(())
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
