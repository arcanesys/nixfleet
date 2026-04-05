# Pre-Merge Validation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Validate PRs #19, #20, #21 pre-merge by adding an agent path-info guard, VM integration tests, and updating nixfleet-demo.

**Architecture:** Integration worktree merges all 3 PRs. Agent guard added to `nix.rs`. Two VM test files added. nixfleet-demo branch overrides to worktree path and adds Attic/backup/policy config.

**Tech Stack:** Rust (agent), Nix (VM tests, modules), NixOS test framework (`pkgs.testers.nixosTest`)

---

## File Map

### nixfleet (integration worktree)

| Action | File | Responsibility |
|--------|------|---------------|
| Modify | `agent/src/nix.rs` | Add `nix path-info` guard in `fetch_closure()` |
| Create | `modules/tests/vm-agent-rebuild.nix` | VM tests: no-cache + missing path guard |
| Modify | `modules/tests/_lib/helpers.nix` | Add Attic modules to test node imports |
| Modify | `flake-module.nix` | Register new VM test as flake check |

### nixfleet-demo (validate/pre-merge branch)

| Action | File | Responsibility |
|--------|------|---------------|
| Modify | `flake.nix` | Override nixfleet input to local worktree path |
| Modify | `fleet.nix` | Add `cache-01` host, update `db-01` backup config |
| Create | `modules/attic.nix` | Attic server config for cache-01, client config for agents |
| Modify | `modules/secrets.nix` | Add `attic-signing-key.age`, `restic-password.age` |
| Modify | `modules/vm-network.nix` | Add `cache-01` at 10.0.100.6 |
| Create | `hosts/cache-01/hardware-configuration.nix` | QEMU guest profile (same pattern as other hosts) |
| Create | `hosts/cache-01/disk-config.nix` | Disko btrfs layout (same pattern as other hosts) |
| Create | `secrets/attic-signing-key.age` | Encrypted Attic signing key |
| Create | `secrets/restic-password.age` | Encrypted restic password |
| Modify | `secrets/recipients.nix` | Add new secrets to recipients map |

## Design Note: Test A (Attic Pipeline)

Test A from the spec (full Attic pipeline VM test) is deferred to manual validation via nixfleet-demo VMs. Building a NixOS system closure inside a VM test requires nix builds inside the test VM (full `/nix/store` write access, significant disk/memory, slow). The nixfleet-demo fleet provides a better test environment for this: spin up all 6 VMs, push a real build to Attic, deploy via CLI with a rollout policy, observe the agent fetch and apply. Tests B and C are automated since they don't require in-VM builds.

---

### Task 1: Create Integration Worktree

**Files:**
- None (git operations only)

- [ ] **Step 1: Create worktree and merge PR branches**

```bash
cd /home/s33d/dev/nix-org/nixfleet
git worktree add ../nixfleet-validate validate/pre-merge --no-track -b validate/pre-merge
cd ../nixfleet-validate
git merge --no-edit feat/cli-mtls-bootstrap
git merge --no-edit feat/phase3-framework-infra
git merge --no-edit feat/rollout-policies-history-schedule
```

- [ ] **Step 2: Verify merge succeeded**

Run: `cd /home/s33d/dev/nix-org/nixfleet-validate && git log --oneline -10`
Expected: All 3 PR merge commits visible on top of main.

- [ ] **Step 3: Verify eval passes after merge**

Run: `nix flake check --no-build`
Expected: All checks evaluate without errors.

- [ ] **Step 4: Verify Rust tests pass**

Run: `cargo test --workspace`
Expected: All tests pass (200+ tests).

---

### Task 2: Agent `nix path-info` Guard

**Files:**
- Modify: `agent/src/nix.rs:18-38` (the `fetch_closure` function)

- [ ] **Step 1: Write the failing test**

Add to `agent/src/nix.rs` in the `#[cfg(test)] mod tests` block:

```rust
#[test]
fn test_path_info_command_construction() {
    let store_path = "/nix/store/abc123-nixos-system";
    // Verify the command we'd construct for path-info is correct
    let args = ["path-info", store_path];
    assert_eq!(args[0], "path-info");
    assert_eq!(args[1], store_path);
}
```

- [ ] **Step 2: Run test to verify it passes**

Run: `cargo test --package nixfleet-agent -- tests::test_path_info_command_construction -v`
Expected: PASS (this is a structural test, not an integration test)

- [ ] **Step 3: Implement the path-info guard**

Replace the no-cache branch in `fetch_closure()`. Change `agent/src/nix.rs`:

Old code (lines 32-37):
```rust
    } else {
        debug!(
            store_path,
            "No cache URL — assuming closure is available locally"
        );
    }
```

New code:
```rust
    } else {
        info!(store_path, "No cache URL — verifying path exists locally");
        let output = Command::new("nix")
            .args(["path-info", store_path])
            .output()
            .await
            .context("failed to spawn nix path-info")?;

        if !output.status.success() {
            anyhow::bail!(
                "store path {store_path} not found locally and no cache URL configured"
            );
        }
    }
```

- [ ] **Step 4: Run all agent tests**

Run: `cargo test --package nixfleet-agent`
Expected: All tests pass. The path-info guard is an integration behavior — tested in VM tests.

- [ ] **Step 5: Run clippy**

Run: `cargo clippy --package nixfleet-agent -- -D warnings`
Expected: No warnings.

- [ ] **Step 6: Commit**

```bash
git add agent/src/nix.rs
git commit -m "feat(agent): add nix path-info guard before apply

When no cache URL is configured, verify the store path exists
locally before attempting switch-to-configuration. Fails
immediately to Idle if the path is missing, preventing
confusing switch errors."
```

---

### Task 3: Update Test Helpers for Attic Modules

**Files:**
- Modify: `modules/tests/_lib/helpers.nix:12-13`

- [ ] **Step 1: Add Attic module imports to helpers**

The test helpers need to import the Attic modules so VM test nodes can enable them. Add after line 13 (`controlPlaneModule`):

```nix
  atticServerModule = ../../scopes/nixfleet/_attic-server.nix;
  atticClientModule = ../../scopes/nixfleet/_attic-client.nix;
```

And add them to the `imports` list in `mkTestNode` (after `controlPlaneModule` on line 73):

```nix
        atticServerModule
        atticClientModule
```

- [ ] **Step 2: Verify eval still passes**

Run: `nix flake check --no-build`
Expected: All existing checks still evaluate. New modules are disabled by default so no behavior changes.

- [ ] **Step 3: Commit**

```bash
git add modules/tests/_lib/helpers.nix
git commit -m "test: add Attic modules to VM test helpers

Include attic-server and attic-client modules in mkTestNode
so VM tests can enable them."
```

---

### Task 4: VM Integration Tests

**Files:**
- Create: `modules/tests/vm-agent-rebuild.nix`
- Modify: `flake-module.nix` (register the new check)

- [ ] **Step 1: Create the test file with shared infrastructure**

Create `modules/tests/vm-agent-rebuild.nix`:

```nix
# Tier A — VM agent rebuild tests: verify the full fetch → apply → verify pipeline.
#
# Test A: Attic pipeline — CP + Attic server + agent. Agent fetches closure from Attic,
#         applies via switch-to-configuration, health checks pass.
# Test B: No-cache — CP + agent. Closure pre-seeded in agent store via nix copy.
#         Agent verifies path exists, applies, health checks pass.
# Test C: Missing path guard — CP + agent. Non-existent store path, no cache URL.
#         Agent detects missing path, reports error, stays at old generation.
#
# Run: nix build .#checks.x86_64-linux.vm-agent-rebuild --no-link
{inputs, ...}: {
  perSystem = {
    pkgs,
    system,
    lib,
    ...
  }: let
    helpers = import ./_lib/helpers.nix {inherit lib;};

    mkTestNode = helpers.mkTestNode {
      inherit inputs;
      hostSpecModule = ../_shared/host-spec-module.nix;
    };

    defaultTestSpec = helpers.defaultTestSpec;

    # Build-time TLS certs: fleet CA + CP server cert + agent client cert.
    testCerts =
      pkgs.runCommand "nixfleet-rebuild-test-certs" {
        nativeBuildInputs = [pkgs.openssl];
      } ''
        mkdir -p $out

        # Fleet CA
        openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 \
          -keyout $out/ca-key.pem -out $out/ca.pem -days 365 -nodes \
          -subj '/CN=nixfleet-rebuild-test-ca'

        # CP server cert
        openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 \
          -keyout $out/cp-key.pem -out $out/cp-csr.pem -nodes \
          -subj '/CN=cp' \
          -addext 'subjectAltName=DNS:cp,DNS:localhost'
        openssl x509 -req -in $out/cp-csr.pem -CA $out/ca.pem -CAkey $out/ca-key.pem \
          -CAcreateserial -out $out/cp-cert.pem -days 365 \
          -copy_extensions copyall

        # Agent client cert
        openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 \
          -keyout $out/agent-key.pem -out $out/agent-csr.pem -nodes \
          -subj '/CN=agent'
        openssl x509 -req -in $out/agent-csr.pem -CA $out/ca.pem -CAkey $out/ca-key.pem \
          -CAcreateserial -out $out/agent-cert.pem -days 365

        rm -f $out/*.csr.pem $out/*.srl
      '';

    # Attic signing key pair (generated at build time for tests)
    atticSigningKey =
      pkgs.runCommand "nixfleet-attic-test-key" {
        nativeBuildInputs = [pkgs.nix];
      } ''
        mkdir -p $out
        nix-store --generate-binary-cache-key test-cache $out/signing-key.sec $out/signing-key.pub
      '';

    # CP node: shared between all test variants
    cpNode = mkTestNode {
      hostSpecValues =
        defaultTestSpec
        // {
          hostName = "cp";
        };
      extraModules = [
        ({pkgs, ...}: {
          environment.etc."nixfleet-tls/ca.pem".source = "${testCerts}/ca.pem";
          environment.etc."nixfleet-tls/cp-cert.pem".source = "${testCerts}/cp-cert.pem";
          environment.etc."nixfleet-tls/cp-key.pem".source = "${testCerts}/cp-key.pem";

          services.nixfleet-control-plane = {
            enable = true;
            openFirewall = true;
            tls = {
              cert = "/etc/nixfleet-tls/cp-cert.pem";
              key = "/etc/nixfleet-tls/cp-key.pem";
              clientCa = "/etc/nixfleet-tls/ca.pem";
            };
          };

          environment.systemPackages = [pkgs.sqlite pkgs.python3];
        })
      ];
    };

    # Agent node builder: non-dry-run agent with mTLS and systemd health check
    mkRebuildAgent = {
      extraAgentConfig ? {},
      extraModules ? [],
    }:
      mkTestNode {
        hostSpecValues =
          defaultTestSpec
          // {
            hostName = "agent";
          };
        extraModules =
          [
            ({pkgs, ...}: {
              security.pki.certificateFiles = ["${testCerts}/ca.pem"];

              environment.etc."nixfleet-tls/ca.pem".source = "${testCerts}/ca.pem";
              environment.etc."nixfleet-tls/agent-cert.pem".source = "${testCerts}/agent-cert.pem";
              environment.etc."nixfleet-tls/agent-key.pem".source = "${testCerts}/agent-key.pem";

              # Marker file to detect the "current" system (pre-deploy)
              environment.etc."nixfleet-test-marker".text = "v1";

              services.nixfleet-agent =
                {
                  enable = true;
                  controlPlaneUrl = "https://cp:8080";
                  machineId = "agent";
                  pollInterval = 2;
                  healthInterval = 5;
                  dryRun = false; # Real rebuilds!
                  tags = ["test"];
                  tls = {
                    clientCert = "/etc/nixfleet-tls/agent-cert.pem";
                    clientKey = "/etc/nixfleet-tls/agent-key.pem";
                  };
                }
                // extraAgentConfig;

              environment.systemPackages = [pkgs.python3];
            })
          ]
          ++ extraModules;
      };

    # Shared test script helpers
    testPreamble = ''
      import json

      TEST_KEY = "test-admin-key"
      KEY_HASH = "944650a7cd0f9e14d5c4fb15edbffb7fa45fb9ed36a4fa9be3d7e5476ae51bd9"
      AUTH = f"-H 'Authorization: Bearer {TEST_KEY}'"
      CURL = "curl -sf --cacert /etc/nixfleet-tls/ca.pem --cert /etc/nixfleet-tls/cp-cert.pem --key /etc/nixfleet-tls/cp-key.pem"
      API = "https://localhost:8080"
    '';

    # Helper: bootstrap CP with test API key and register agent
    bootstrapScript = ''
      # Start CP and bootstrap
      cp.start()
      cp.wait_for_unit("nixfleet-control-plane.service")
      cp.wait_for_open_port(8080)

      cp.succeed(
          f"sqlite3 /var/lib/nixfleet-cp/state.db "
          f"\"INSERT INTO api_keys (key_hash, name, role) VALUES ('{KEY_HASH}', 'test-admin', 'admin')\""
      )

      # Register agent
      cp.succeed(
          f"{CURL} -X POST {API}/api/v1/machines/agent/register "
          f"{AUTH} "
          f"-H 'Content-Type: application/json' "
          f"-d '{{\"tags\": [\"test\"]}}'"
      )
    '';
  in
    lib.optionalAttrs (system == "x86_64-linux") {
      checks = {
        # --- Test B + C: No-cache path + missing path guard ---
        # Combined into one test for efficiency (same 2-node setup)
        vm-agent-rebuild = pkgs.testers.nixosTest {
          name = "vm-agent-rebuild";

          nodes.cp = cpNode;

          nodes.agent = mkRebuildAgent {};

          testScript = ''
            ${testPreamble}
            ${bootstrapScript}

            # Start agent
            agent.start()
            agent.wait_for_unit("nixfleet-agent.service")

            # Wait for agent to register with CP
            cp.wait_until_succeeds(
                f"{CURL} {AUTH} {API}/api/v1/machines "
                f"| python3 -c \"import sys,json; machines=json.load(sys.stdin); "
                f"assert any(m['id'] == 'agent' for m in machines), 'agent not registered'\"",
                timeout=60,
            )

            # --- Test B: No-cache, pre-seeded store path ---
            # Get agent's current system store path
            current_gen = agent.succeed("readlink /run/current-system").strip()

            # Set the agent's own current generation as desired (it's already in the store)
            # This tests the path-info guard succeeds and the agent reports up-to-date
            set_gen_body = json.dumps({"hash": current_gen})
            cp.succeed(
                f"{CURL} -X POST {API}/api/v1/machines/agent/set-generation "
                f"{AUTH} "
                f"-H 'Content-Type: application/json' "
                f"-d '{set_gen_body}'"
            )

            # Agent should poll, find path exists (nix path-info succeeds), and report up-to-date
            cp.wait_until_succeeds(
                f"{CURL} {AUTH} {API}/api/v1/machines "
                f"| python3 -c \"import sys,json; machines=json.load(sys.stdin); "
                f"agent = [m for m in machines if m['id'] == 'agent'][0]; "
                f"assert agent.get('current_generation') == '{current_gen}', "
                f"f'Expected {current_gen}, got {{agent.get(\\\"current_generation\\\")}}'\"",
                timeout=30,
            )

            # --- Test C: Missing path guard ---
            # Set a fabricated store path that doesn't exist
            fake_path = "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-nixos-system-fake"
            fake_body = json.dumps({"hash": fake_path})
            cp.succeed(
                f"{CURL} -X POST {API}/api/v1/machines/agent/set-generation "
                f"{AUTH} "
                f"-H 'Content-Type: application/json' "
                f"-d '{fake_body}'"
            )

            # Agent should try to fetch, nix path-info fails, agent stays at current gen
            # Wait a few poll cycles and verify agent did NOT switch
            import time
            time.sleep(10)  # 5 poll cycles at 2s interval

            # Agent should still be at original generation (not the fake one)
            actual_gen = agent.succeed("readlink /run/current-system").strip()
            assert actual_gen == current_gen, f"Agent switched unexpectedly! Expected {current_gen}, got {actual_gen}"

            # Verify agent logged the error (check journal)
            agent.succeed(
                "journalctl -u nixfleet-agent.service --no-pager "
                "| grep -q 'not found locally and no cache URL configured'"
            )
          '';
        };
      };
    };
}
```

- [ ] **Step 2: Register the test in flake-module.nix**

Find the imports list in `flake-module.nix` where other test files are imported and add:

```nix
./modules/tests/vm-agent-rebuild.nix
```

- [ ] **Step 3: Verify eval passes**

Run: `nix flake check --no-build`
Expected: `vm-agent-rebuild` appears in checks but doesn't build yet.

- [ ] **Step 4: Commit**

```bash
git add modules/tests/vm-agent-rebuild.nix flake-module.nix
git commit -m "test(vm): add agent rebuild integration tests

Test B: no-cache path with pre-seeded store path — agent verifies
via nix path-info, reports up-to-date.

Test C: missing path guard — agent detects non-existent store path,
stays at current generation, logs error."
```

---

### Task 5: nixfleet-demo Branch Setup

**Files:**
- Modify: `nixfleet-demo/flake.nix`

- [ ] **Step 1: Create validate branch in nixfleet-demo**

```bash
cd /home/s33d/dev/nix-org/nixfleet-demo
git checkout -b validate/pre-merge
```

- [ ] **Step 2: Override flake input to integration worktree**

Change `flake.nix` line 5 from:
```nix
    nixfleet.url = "github:abstracts33d/nixfleet";
```

To:
```nix
    nixfleet.url = "path:/home/s33d/dev/nix-org/nixfleet-validate";
```

- [ ] **Step 3: Update flake lock**

Run: `nix flake update nixfleet`
Expected: Lock file updates to point at local worktree. No errors.

- [ ] **Step 4: Verify existing hosts still evaluate**

Run: `nix flake check --no-build`
Expected: All 5 existing hosts evaluate cleanly with the merged nixfleet.

- [ ] **Step 5: Commit**

```bash
git add flake.nix flake.lock
git commit -m "chore: override nixfleet input to local integration worktree

Temporary override for pre-merge validation of PRs #19, #20, #21.
Will revert to github:abstracts33d/nixfleet after PRs merge."
```

---

### Task 6: Add cache-01 Host to Demo

**Files:**
- Create: `nixfleet-demo/hosts/cache-01/hardware-configuration.nix`
- Create: `nixfleet-demo/hosts/cache-01/disk-config.nix`
- Modify: `nixfleet-demo/modules/vm-network.nix`
- Modify: `nixfleet-demo/fleet.nix`

- [ ] **Step 1: Create hardware-configuration.nix**

Copy the pattern from any existing host (e.g., `hosts/cp-01/hardware-configuration.nix`). Create `hosts/cache-01/hardware-configuration.nix`:

```nix
# QEMU guest hardware profile for cache-01 VM.
{
  lib,
  modulesPath,
  ...
}: {
  imports = [(modulesPath + "/profiles/qemu-guest.nix")];
  boot.loader.systemd-boot.enable = true;
  boot.loader.efi.canTouchEfiVariables = true;
}
```

- [ ] **Step 2: Create disk-config.nix**

Copy from an existing non-impermanent host (e.g., `hosts/db-01/disk-config.nix`). Create `hosts/cache-01/disk-config.nix`:

```nix
# Disko btrfs layout for cache-01 (non-impermanent).
{
  disko.devices.disk.main = {
    type = "disk";
    device = "/dev/vda";
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
            mountOptions = ["umask=0077"];
          };
        };
        root = {
          size = "100%";
          content = {
            type = "btrfs";
            extraArgs = ["-f"];
            subvolumes = {
              "@root" = {
                mountpoint = "/";
                mountOptions = ["compress=zstd" "noatime"];
              };
              "@nix" = {
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
```

- [ ] **Step 3: Add cache-01 to vm-network.nix**

Add `cache-01` to the IP assignment map and `/etc/hosts` in `modules/vm-network.nix`. Add to the hosts attrset:

```nix
"cache-01" = "10.0.100.6";
```

And add to the hosts file entries:

```nix
"10.0.100.6" = ["cache-01"];
```

(Follow the exact pattern of the existing entries in the file.)

- [ ] **Step 4: Add cache-01 to fleet.nix**

Add the host definition to `fleet.nix` in `flake.nixosConfigurations`. Add after the `mon-01` block:

```nix
    cache-01 = mkHost {
      hostName = "cache-01";
      platform = "x86_64-linux";
      hostSpec = orgDefaults;
      modules =
        hostModules "cache-01"
        ++ [
          {
            services.nixfleet-attic-server = {
              enable = true;
              openFirewall = true;
              signingKeyFile = "/run/agenix/attic-signing-key";
            };
          }
        ];
    };
```

- [ ] **Step 5: Verify eval**

Run: `nix flake check --no-build`
Expected: All 6 hosts evaluate (may fail on missing secrets — that's Task 7).

- [ ] **Step 6: Commit**

```bash
git add hosts/cache-01/ modules/vm-network.nix fleet.nix
git commit -m "feat: add cache-01 host with Attic binary cache server

Dedicated host for Attic binary cache, serving closures to
fleet agents. VLAN IP 10.0.100.6."
```

---

### Task 7: Add Secrets and Attic Module to Demo

**Files:**
- Create: `nixfleet-demo/modules/attic.nix`
- Modify: `nixfleet-demo/modules/secrets.nix`
- Modify: `nixfleet-demo/secrets/recipients.nix`
- Create: `nixfleet-demo/secrets/attic-signing-key.age`
- Create: `nixfleet-demo/secrets/restic-password.age`

- [ ] **Step 1: Generate and encrypt secrets**

Generate the Attic signing key and restic password, then encrypt with age:

```bash
cd /home/s33d/dev/nix-org/nixfleet-demo

# Generate Attic signing key
nix-store --generate-binary-cache-key demo-cache secrets/attic-signing-key.sec secrets/attic-signing-key.pub

# Encrypt the signing key
age -R secrets/recipients.nix -o secrets/attic-signing-key.age secrets/attic-signing-key.sec
rm secrets/attic-signing-key.sec

# Generate and encrypt restic password
openssl rand -hex 32 | age -R secrets/recipients.nix -o secrets/restic-password.age -
```

Note: If the age encryption uses a different method in this project (agenix rekey, etc.), follow the existing pattern in `secrets/`. Check `secrets/recipients.nix` for the exact format.

- [ ] **Step 2: Update recipients.nix**

Add the new secret files to `secrets/recipients.nix` following the existing pattern. Add entries for:
- `"attic-signing-key.age"` — recipients: all hosts (or just cache-01)
- `"restic-password.age"` — recipients: db-01

- [ ] **Step 3: Create modules/attic.nix**

```nix
# Attic binary cache configuration for the demo fleet.
# Server on cache-01, client on all agent hosts.
agenixModule: {
  config,
  lib,
  ...
}: let
  agentHosts = ["web-01" "web-02" "db-01" "mon-01"];
  hostName = config.networking.hostName;
  isAgent = builtins.elem hostName agentHosts;
  signingPublicKey = builtins.readFile ../secrets/attic-signing-key.pub;
in {
  # Attic client on all agent hosts
  services.nixfleet-attic-client = lib.mkIf isAgent {
    enable = true;
    cacheUrl = "http://cache-01:8081";
    publicKey = signingPublicKey;
  };
}
```

Note: The Attic server config is already inline in `fleet.nix` (Task 6). This module handles the client side.

- [ ] **Step 4: Update modules/secrets.nix**

Add the new agenix secret declarations following the existing pattern. Add:

```nix
age.secrets.attic-signing-key = {
  file = ../secrets/attic-signing-key.age;
  owner = "root";
};
```

And for db-01 specifically:

```nix
age.secrets.restic-password = lib.mkIf (config.networking.hostName == "db-01") {
  file = ../secrets/restic-password.age;
  owner = "root";
};
```

(Follow the exact pattern of existing secret declarations in the file.)

- [ ] **Step 5: Add attic.nix to fleetModules in fleet.nix**

Add to the `fleetModules` list in `fleet.nix`:

```nix
(import ./modules/attic.nix inputs.agenix.nixosModules.default)
```

Or if `attic.nix` doesn't need the agenix module argument (depends on implementation), add as a plain import:

```nix
./modules/attic.nix
```

- [ ] **Step 6: Update db-01 with restic backend**

In `fleet.nix`, update db-01's backup config from:

```nix
nixfleet.backup = {
  enable = true;
  schedule = "*-*-* 03:00:00";
};
```

To:

```nix
nixfleet.backup = {
  enable = true;
  schedule = "*-*-* 03:00:00";
  backend = "restic";
  restic = {
    repository = "/var/lib/backup/restic-repo";
    passwordFile = config.age.secrets.restic-password.path;
  };
};
```

Note: The `config` reference requires the db-01 module block to use a function form `{config, ...}:`.

- [ ] **Step 7: Verify eval**

Run: `nix flake check --no-build`
Expected: All 6 hosts evaluate cleanly.

- [ ] **Step 8: Commit**

```bash
git add modules/attic.nix modules/secrets.nix secrets/ fleet.nix
git commit -m "feat: add Attic cache + restic backup to demo fleet

- Attic signing key and restic password as agenix secrets
- Attic client configured on all agent hosts
- db-01 backup uses restic backend
- cache-01 Attic server secret wiring"
```

---

### Task 8: Final Verification

**Files:** None (verification only)

- [ ] **Step 1: Verify nixfleet integration worktree**

```bash
cd /home/s33d/dev/nix-org/nixfleet-validate
nix flake check --no-build
cargo test --workspace
```

Expected: All eval checks pass, all Rust tests pass.

- [ ] **Step 2: Verify nixfleet-demo**

```bash
cd /home/s33d/dev/nix-org/nixfleet-demo
nix flake check --no-build
```

Expected: All 6 hosts evaluate cleanly.

- [ ] **Step 3: Present results for review**

Present to user:
- nixfleet worktree: files changed, commits, eval + cargo test results
- nixfleet-demo: files changed, commits, eval results
- List of commands for manual VM testing (build-vm, start-vm)
- Cherry-pick plan for after validation

Do NOT push or create PRs — wait for user review.
