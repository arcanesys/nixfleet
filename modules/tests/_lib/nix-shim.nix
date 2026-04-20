# modules/tests/_lib/nix-shim.nix
#
# Returns a derivation producing a minimal `nix` shim that intercepts the
# subprocess calls `cli::release::create` makes. The shim is installed on
# test nodes that need to run `nixfleet release create` without a real
# flake evaluation.
#
# Arguments (curried):
#   {pkgs, lib}      - nixpkgs + lib
#   {hosts}          - list of attrs { name, platform, tags, storePath }
#
# The shim dispatches on argv[1] (the first sub-command) and returns canned
# JSON/text matching what real `nix eval` / `nix build` would print.
# `nix copy` is delegated to the real `nix` so the binary-cache transfer
# actually happens. Unknown subcommands also fall through to real `nix`.
#
# The shim must be updated if cli::release::create grows new nix invocations.
{
  pkgs,
  lib,
}: {hosts}: let
  hostnamesJson = builtins.toJSON (map (h: h.name) hosts);

  platformCases = lib.concatMapStringsSep "\n        " (h: ''
    "${h.name}") printf '%s' "${h.platform}" ;;'')
  hosts;

  tagsCases = lib.concatMapStringsSep "\n        " (h: ''
    "${h.name}") printf '%s' '${builtins.toJSON h.tags}' ;;'')
  hosts;

  storePathCases = lib.concatMapStringsSep "\n        " (h: ''
    "${h.name}") printf '%s\n' "${h.storePath}" ;;'')
  hosts;
in
  pkgs.writeShellApplication {
    name = "nix";
    runtimeInputs = [];
    # The shim is called as `nix <subcommand> <args...>`. We dispatch on $1.
    # grep usage is deliberate: the arg shapes we match are stable and narrow.
    #
    # CRITICAL: fall-through paths MUST call the real nix by its immutable
    # store path (`${pkgs.nix}/bin/nix`), not `/run/current-system/sw/bin/nix`.
    # The shim is installed via `environment.systemPackages`, which means
    # its own bin/nix lives under `/run/current-system/sw/bin/nix` - the
    # SAME path that the system's real nix binary was supposed to occupy.
    # On systems where the shim wins the path collision, delegating to
    # `/run/current-system/sw/bin/nix` is an infinite exec loop of the
    # shim calling itself. Hardcoding `${pkgs.nix}/bin/nix` sidesteps the
    # collision entirely.
    text = ''
      cmd="''${1:-}"
      case "$cmd" in
        eval)
          # nix eval <flake>#nixosConfigurations --apply builtins.attrNames --json
          # nix eval <flake>#darwinConfigurations --apply builtins.attrNames --json
          if printf '%s\n' "$@" | grep -q 'attrNames'; then
            if printf '%s\n' "$@" | grep -q 'darwinConfigurations'; then
              printf '%s\n' '[]'
            else
              printf '%s\n' '${hostnamesJson}'
            fi
            exit 0
          fi

          # nix eval <flake>#nixosConfigurations.<host>.pkgs.system --raw
          if printf '%s\n' "$@" | grep -q '\.pkgs\.system'; then
            target=$(printf '%s\n' "$@" \
              | grep -oE 'nixosConfigurations\.[^. ]+' \
              | head -n1 | cut -d. -f2)
            case "$target" in
              ${platformCases}
              *) echo "nix-shim: unknown host for pkgs.system: $target" >&2; exit 1 ;;
            esac
            exit 0
          fi

          # nix eval <flake>#nixosConfigurations.<host>.config.services.nixfleet-agent.tags --json
          if printf '%s\n' "$@" | grep -q 'nixfleet-agent\.tags'; then
            target=$(printf '%s\n' "$@" \
              | grep -oE 'nixosConfigurations\.[^. ]+' \
              | head -n1 | cut -d. -f2)
            case "$target" in
              ${tagsCases}
              *) printf '%s\n' '[]' ;;
            esac
            exit 0
          fi

          # Unknown eval - return empty JSON.
          printf '%s\n' '{}'
          exit 0
          ;;
        build)
          # nix build <flake>#nixosConfigurations.<host>.config.system.build.toplevel \
          #   --print-out-paths --no-link
          target=$(printf '%s\n' "$@" \
            | grep -oE 'nixosConfigurations\.[^. ]+' \
            | head -n1 | cut -d. -f2)
          case "$target" in
            ${storePathCases}
            *) echo "nix-shim: unknown host for build: $target" >&2; exit 1 ;;
          esac
          exit 0
          ;;
        copy)
          # Delegate to the real nix - we want the actual binary-cache transfer.
          exec ${pkgs.nix}/bin/nix "$@"
          ;;
        flake)
          # nix flake metadata ... --json  (used by flake_revision)
          printf '%s\n' '{"revision":"deadbeefcafe"}'
          exit 0
          ;;
        *)
          # Unknown subcommand - pass through to real nix.
          exec ${pkgs.nix}/bin/nix "$@"
          ;;
      esac
    '';
  }
