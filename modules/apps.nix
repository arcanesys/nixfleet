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
        "validate" = mkScript "validate" "Run format, eval, host, VM, and Rust tests" ''
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
          #   --vm      also build all VM tests under checks.${system} (slow)
          #   --rust    also run `cargo test --workspace` (medium)
          #   --all     shorthand for --vm --rust
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
            printf "%-40s" "$name"
            if OUTPUT=$("$@" 2>&1); then
              echo -e "''${GREEN}OK''${NC}"
              PASS=$((PASS + 1))
            else
              echo -e "''${RED}FAIL''${NC}"
              echo "$OUTPUT" | tail -5
              FAIL=$((FAIL + 1))
            fi
          }

          check_eval() {
            local name="$1"
            local attr="$2"
            printf "%-40s" "$name"
            if nix eval "$attr" --apply 'x: x.config.system.build.toplevel.name or "ok"' 2>/dev/null 1>/dev/null; then
              echo -e "''${GREEN}OK (eval)''${NC}"
              PASS=$((PASS + 1))
            else
              echo -e "''${YELLOW}SKIP (cross-platform)''${NC}"
              SKIP=$((SKIP + 1))
            fi
          }

          echo "=== Formatting ==="
          check "nix fmt" nix fmt -- --fail-on-change

          echo ""
          echo "=== Eval Tests ==="
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
            echo "=== Rust Tests ==="
            # `cargo test --workspace` runs every crate's unit tests
            # and every integration test under control-plane/tests and
            # cli/tests. Runs inside the dev shell so rustc/cargo are
            # on PATH even when invoked from outside `nix develop`.
            check "cargo test --workspace" \
              nix develop --command cargo test --workspace --quiet
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
