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
    # VM lifecycle apps (build-vm, start-vm, stop-vm, clean-vm, test-vm, provision) from mkVmApps.

    apps =
      {
        "validate" = mkScript "validate" "Run format checks, eval tests, and host builds" ''
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

          while [[ ''${#} -gt 0 ]]; do
            case "''${1}" in
              --fast) FAST=1; shift ;;
              --vm) VM=1; shift ;;
              *) echo "Unknown option: ''${1}"; exit 1 ;;
            esac
          done

          check() {
            local name="$1"
            shift
            printf "%-30s" "$name"
            if OUTPUT=$("$@" 2>&1); then
              echo -e "''${GREEN}OK''${NC}"
              PASS=$((PASS + 1))
            else
              echo -e "''${RED}FAIL''${NC}"
              echo "$OUTPUT" | tail -3
              FAIL=$((FAIL + 1))
            fi
          }

          check_eval() {
            local name="$1"
            local attr="$2"
            printf "%-30s" "$name"
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
                for t in vm-core vm-minimal vm-fleet; do
                  check "$t" nix build ".#checks.${system}.$t" --no-link
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
