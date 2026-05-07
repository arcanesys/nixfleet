{
  platform,
  pkgs,
}: let
  inherit (platform) mkScript sharedHelpers;
  lib = pkgs.lib;
in
  mkScript "clean-vm" "Delete VM disk, pidfile, and port file" ''
    set -euo pipefail
    export PATH="${lib.makeBinPath (with pkgs; [coreutils nix])}:$PATH"

    ${sharedHelpers}

    HOST=""
    ALL=0

    while [[ $# -gt 0 ]]; do
      case "$1" in
        -h|--host) HOST="$2"; shift 2 ;;
        --all) ALL=1; shift ;;
        *) echo "Unknown argument: $1" >&2; exit 1 ;;
      esac
    done

    [[ $ALL -eq 0 && -z "$HOST" ]] && echo -e "''${RED}Specify -h HOST or --all''${NC}" && exit 1

    clean_one() {
      local host="$1"
      local disk="''$VM_DIR/$host.qcow2"
      local pidfile="''$VM_DIR/$host.pid"
      local portfile="''$VM_DIR/$host.port"

      # Stop if running
      if [ -f "$pidfile" ]; then
        local pid
        pid=$(cat "$pidfile")
        kill "$pid" 2>/dev/null || true
        rm -f "$pidfile"
      fi

      local cleaned=0
      [ -f "$disk" ] && rm -f "$disk" && cleaned=1
      [ -f "$portfile" ] && rm -f "$portfile"

      if [ "$cleaned" = "1" ]; then
        echo -e "''${GREEN}[$host] Cleaned''${NC}"
      else
        echo -e "''${YELLOW}[$host] Nothing to clean''${NC}"
      fi
    }

    if [[ $ALL -eq 1 ]]; then
      mapfile -t hosts_arr <<< "$(all_hosts)"
      for host in "''${hosts_arr[@]}"; do
        [[ -n "$host" ]] && clean_one "$host"
      done
    else
      clean_one "$HOST"
    fi
  ''
