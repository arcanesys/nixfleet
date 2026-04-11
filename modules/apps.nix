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
    devShells.default = pkgs.mkShell {
      nativeBuildInputs = with pkgs; [
        bashInteractive
        git
        age

        # Rust toolchain
        cargo
        rustc
        clippy
        rustfmt
        rust-analyzer
      ];
      shellHook = ''
        export EDITOR=vim
        git config core.hooksPath .githooks 2>/dev/null || true
      '';
    };

    # Deployment is now standard: nixos-anywhere, nixos-rebuild, darwin-rebuild.
    # Removed: install, build-switch, docs, launch-vm, rollback, spawn-qemu, spawn-utm (ADR-004).
    # VM lifecycle apps (build-vm, start-vm, stop-vm, clean-vm, test-vm) from mkVmApps.

    apps =
      {
        "validate" = mkScript "validate" "Single entry point for the whole test suite (format + eval + hosts + VM + Rust + clippy)" ''
          set -uo pipefail

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
          #   --vm      + every vm-* check (slow — minutes per check)
          #   --rust    + cargo test --workspace + cargo clippy --workspace
          #             + nix build of each rust package (runs test suite
          #               under the nix build sandbox, catches
          #               environment-dependent test failures that dev-shell
          #               cargo test misses)
          #   --all     everything
          #
          # --all is the intended single entry point for "test everything"
          # (CI, pre-merge, pre-release). Don't list individual nix build /
          # cargo commands in docs — point at this script.
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
              echo "$OUTPUT" | tail -10
              FAIL=$((FAIL + 1))
            fi
          }

          echo "=== Formatting ==="
          check "nix fmt" nix fmt -- --fail-on-change

          echo ""
          echo "=== Flake Check (every flake output, eval-only) ==="
          # `nix flake check --no-build` evaluates every output in the
          # flake (apps, packages, devShells, nixosConfigurations,
          # checks) and confirms they type-check / have no eval errors.
          # Cheaper than building anything and catches attrset drift
          # across crates + modules + the validate app itself.
          check "nix flake check --no-build" nix flake check --no-build

          echo ""
          echo "=== Eval Tests (explicit eval-* derivations) ==="
          ${
            if isLinux
            then ''
              for t in eval-hostspec-defaults eval-ssh-hardening eval-username-override eval-locale-timezone eval-ssh-authorized eval-password-files; do
                check "$t" nix build ".#checks.${system}.$t" --no-link
              done
            ''
            else ''
              echo -e "''${YELLOW}SKIP (Linux-only checks)''${NC}"
              SKIP=$((SKIP + 1))
            ''
          }

          echo ""
          echo "=== NixOS Test Hosts (build) ==="
          HOSTS=$(nix eval .#nixosConfigurations --apply 'x: builtins.concatStringsSep " " (builtins.attrNames x)' --raw 2>/dev/null)
          for host in $HOSTS; do
            check "$host" nix build ".#nixosConfigurations.$host.config.system.build.toplevel" --no-link
          done

          ${
            if isLinux
            then ''
              if [ "$VM" = "1" ]; then
                echo ""
                echo "=== VM Integration Tests ==="
                # Discover all vm-* checks dynamically so new scenario
                # subtests are picked up automatically without touching
                # this script.
                VM_TESTS=$(nix eval ".#checks.${system}" \
                    --apply 'cs: builtins.concatStringsSep " " (builtins.filter (n: builtins.match "vm-.*" n != null) (builtins.attrNames cs))' \
                    --raw 2>/dev/null)
                for t in $VM_TESTS; do
                  check "$t" nix build ".#checks.${system}.$t" --no-link
                done
              fi
            ''
            else ""
          }

          if [ "$RUST" = "1" ]; then
            echo ""
            echo "=== Rust Tests (dev shell) ==="
            # `cargo test --workspace` runs every crate's unit tests
            # and every integration test under control-plane/tests and
            # cli/tests. Runs inside the dev shell so rustc/cargo are
            # on PATH even when invoked from outside `nix develop`.
            check "cargo test --workspace" \
              nix develop --command cargo test --workspace --quiet

            echo ""
            echo "=== Rust Lint (dev shell) ==="
            # clippy with -D warnings catches dead code, unused
            # dependencies, and style regressions that cargo test does
            # not. Run against the workspace with all targets so tests
            # and examples are linted too.
            check "cargo clippy --workspace -D warnings" \
              nix develop --command cargo clippy --workspace --all-targets -- -D warnings

            ${
            if isLinux
            then ''
              echo ""
              echo "=== Rust Package Builds (nix sandbox) ==="
              # Rust packages defined in flake-module.nix build their
              # crates inside the nix build sandbox, which runs
              # `cargo test` under a restricted environment (no
              # /run/current-system, no network, no host /nix/store
              # mutations). Some tests pass in the dev shell but
              # fail inside the sandbox due to environment leakage
              # (see the agent run_loop_scenarios fix in commit
              # ff49f5c). Running these catches those divergences.
              for pkg in nixfleet-agent nixfleet-control-plane nixfleet-cli; do
                check "package: $pkg" \
                  nix build ".#packages.${system}.$pkg" --no-link
              done
            ''
            else ""
          }
          fi

          echo ""
          echo "==================================="
          echo -e "''${GREEN}Passed: $PASS''${NC}  ''${RED}Failed: $FAIL''${NC}  ''${YELLOW}Skipped: $SKIP''${NC}"
          if [ "$FAIL" -gt 0 ]; then exit 1; fi
        '';
      }
      // (mkVmApps {inherit pkgs;});
  };
}
