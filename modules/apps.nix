{inputs, ...}: let
  mkVmApps = import ./_shared/lib/mk-vm-apps.nix {inherit inputs;};
in {
  perSystem = {
    pkgs,
    system,
    ...
  }: let
    isLinux = builtins.elem system ["x86_64-linux" "aarch64-linux"];
    mkScript = name: description: text: {
      type = "app";
      program = "${pkgs.writeShellScriptBin name text}/bin/${name}";
      meta.description = description;
    };
  in {
    # devShell is defined in agent-package.nix (crane-based)

    # Deployment is now standard: nixos-anywhere, nixos-rebuild, darwin-rebuild.
    # Removed: install, build-switch, docs, launch-vm, rollback, spawn-qemu, spawn-utm (ADR-004).
    # VM lifecycle apps (build-vm, start-vm, stop-vm, clean-vm, test-vm) from mkVmApps.

    apps =
      {
        "validate" = mkScript "validate" "Single entry point for the whole test suite (format + eval + hosts + VM + Rust + clippy)" ''
          set -uo pipefail

          # Propagate Ctrl+C to all child processes (nix build, cargo, etc.)
          trap 'kill 0; exit 130' INT TERM

          GREEN='\033[1;32m'
          RED='\033[1;31m'
          YELLOW='\033[1;33m'
          NC='\033[0m'

          PASS=0
          FAIL=0
          SKIP=0
          FAST=0
          VM=0
          RUST=0

          # Default: fast mode (format + eval + host builds). Flags add tiers.
          #
          #   (none)    fast: format + flake check + eval-* + host builds
          #   --vm      + every vm-* check (slow - minutes per check)
          #   --rust    + cargo test --workspace + cargo clippy --workspace
          #             + nix build of each rust package (runs test suite
          #               under the nix build sandbox, catches
          #               environment-dependent test failures that dev-shell
          #               cargo test misses)
          #   --all     everything
          #
          # --all is the intended single entry point for "test everything"
          # (CI, pre-merge, pre-release). Don't list individual nix build /
          # cargo commands in docs - point at this script.
          while [[ ''${#} -gt 0 ]]; do
            case "''${1}" in
              --fast) FAST=1; shift ;;
              --vm) VM=1; shift ;;
              --rust) RUST=1; shift ;;
              --all) VM=1; RUST=1; shift ;;
              *) echo "Unknown option: ''${1}"; exit 1 ;;
            esac
          done

          check() {
            local name="$1"
            shift
            printf "%-48s" "$name"
            if OUTPUT=$("$@" 2>&1); then
              echo -e "''${GREEN}OK''${NC}"
              PASS=$((PASS + 1))
            else
              echo -e "''${RED}FAIL''${NC}"
              # Strip nix evaluation warnings from error output
              echo "$OUTPUT" | grep -v '^evaluation warning:' | grep -v '^[[:space:]]\{20,\}' | tail -10
              FAIL=$((FAIL + 1))
            fi
          }

          # Schedule every derivation in a batch in ONE `nix build`
          # invocation so nix can parallelise independent builds across
          # CPU cores. Per-target PASS/FAIL reporting then re-runs
          # `nix build` on each one individually - those calls are
          # sub-second cache hits against the already-realised store
          # paths, but keep the granular line-per-target output.
          #
          # `--keep-going` lets independent targets keep building after
          # one fails, so a single broken host does not block the rest
          # and the final summary lists every real failure.
          #
          # Arguments: a space-separated list of installable strings
          # (everything that would go after `nix build`).
          prebuild_parallel() {
            # Build all targets in one invocation so nix can parallelise.
            # stderr is discarded to suppress evaluation warnings
            # (impermanence UID/GID noise from test hosts). Failures are
            # tolerated - the per-target `check` calls give PASS/FAIL.
            nix build --no-link --keep-going "$@" >/dev/null 2>&1 || true
          }

          # Order: fastest/most-likely-to-fail first.
          # 1. Format    (~2s)  - style issues
          # 2. Flake     (~5s)  - eval errors across all outputs
          # 3. Eval      (~5s)  - module logic
          # 4. Rust test (~15s) - code bugs (fast with nextest + merged binaries)
          # 5. Rust lint  (~5s) - warnings/style
          # 6. Hosts    (~30s+) - NixOS config errors
          # 7. Packages  (~1m)  - sandbox-specific failures
          # 8. VM tests (min+)  - full integration, slowest

          echo "=== Formatting ==="
          check "nix fmt" nix fmt -- --fail-on-change

          echo ""
          echo "=== Flake Check (eval-only) ==="
          check "nix flake check --no-build" nix flake check --no-build --quiet

          echo ""
          echo "=== Examples Eval ==="
          for example_flake in examples/*/flake.nix; do
            example_dir="$(dirname "$example_flake")"
            example_name="$(basename "$example_dir")"
            check "example: $example_name" nix flake check --no-build --quiet "./$example_dir"
          done

          echo ""
          echo "=== Eval Tests ==="
          ${
            if isLinux
            then ''
              EVAL_TESTS="eval-hostspec-defaults eval-ssh-hardening eval-username-override eval-locale-timezone eval-ssh-authorized eval-password-files"
              EVAL_ATTRS=""
              for t in $EVAL_TESTS; do
                EVAL_ATTRS="$EVAL_ATTRS .#checks.${system}.$t"
              done
              prebuild_parallel $EVAL_ATTRS
              for t in $EVAL_TESTS; do
                check "$t" nix build ".#checks.${system}.$t" --no-link --quiet
              done
            ''
            else ''
              echo -e "''${YELLOW}SKIP (Linux-only checks)''${NC}"
              SKIP=$((SKIP + 1))
            ''
          }

          if [ "$RUST" = "1" ]; then
            echo ""
            echo "=== Rust Tests ==="
            check "cargo nextest run" \
              nix develop --command cargo nextest run --workspace

            echo ""
            echo "=== Rust Lint ==="
            check "cargo clippy --workspace -D warnings" \
              nix develop --command cargo clippy --workspace --all-targets -- -D warnings
          fi

          echo ""
          echo "=== NixOS Host Builds ==="
          HOSTS=$(nix eval .#nixosConfigurations --apply 'x: builtins.concatStringsSep " " (builtins.attrNames x)' --raw 2>/dev/null)
          HOST_ATTRS=""
          for host in $HOSTS; do
            HOST_ATTRS="$HOST_ATTRS .#nixosConfigurations.$host.config.system.build.toplevel"
          done
          prebuild_parallel $HOST_ATTRS
          for host in $HOSTS; do
            check "$host" nix build ".#nixosConfigurations.$host.config.system.build.toplevel" --no-link --quiet
          done

          ${
            if isLinux
            then ''
              if [ "$RUST" = "1" ]; then
                echo ""
                echo "=== Rust Package Builds (nix sandbox) ==="
                prebuild_parallel \
                  .#checks.${system}.workspace-tests \
                  .#packages.${system}.nixfleet-agent \
                  .#packages.${system}.nixfleet-control-plane \
                  .#packages.${system}.nixfleet-cli
                check "workspace-tests" nix build ".#checks.${system}.workspace-tests" --no-link
                for pkg in nixfleet-agent nixfleet-control-plane nixfleet-cli; do
                  check "package: $pkg" \
                    nix build ".#packages.${system}.$pkg" --no-link
                done
              fi

              if [ "$VM" = "1" ]; then
                echo ""
                echo "=== VM Integration Tests ==="
                VM_TESTS=$(nix eval ".#checks.${system}" \
                    --apply 'cs: builtins.concatStringsSep " " (builtins.filter (n: builtins.match "vm-.*" n != null) (builtins.attrNames cs))' \
                    --raw 2>/dev/null)
                VM_ATTRS=""
                for t in $VM_TESTS; do
                  VM_ATTRS="$VM_ATTRS .#checks.${system}.$t"
                done
                prebuild_parallel $VM_ATTRS
                for t in $VM_TESTS; do
                  check "$t" nix build ".#checks.${system}.$t" --no-link --quiet
                done
              fi
            ''
            else ""
          }

          echo ""
          echo "==================================="
          echo -e "''${GREEN}Passed: $PASS''${NC}  ''${RED}Failed: $FAIL''${NC}  ''${YELLOW}Skipped: $SKIP''${NC}"
          if [ "$FAIL" -gt 0 ]; then exit 1; fi
        '';
      }
      // (mkVmApps {inherit pkgs;});
  };
}
