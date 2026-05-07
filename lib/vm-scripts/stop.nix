{
  platform,
  pkgs,
}: let
  inherit (platform) mkScript sharedHelpers;
  lib = pkgs.lib;
in
  mkScript "stop-vm" "Stop a running VM daemon" ''
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

    stop_one() {
      local host="$1"
      local pidfile="''$VM_DIR/$host.pid"

      if [ ! -f "$pidfile" ]; then
        echo -e "''${YELLOW}[$host] Not running (no pidfile)''${NC}"
        return 0
      fi

      local pid
      pid=$(cat "$pidfile")
      if kill "$pid" 2>/dev/null; then
        echo -e "''${GREEN}[$host] Stopped (PID $pid)''${NC}"
      else
        echo -e "''${YELLOW}[$host] Process $pid already dead''${NC}"
      fi
      rm -f "$pidfile"
    }

    if [[ $ALL -eq 1 ]]; then
      mapfile -t hosts_arr <<< "$(all_hosts)"
      for host in "''${hosts_arr[@]}"; do
        [[ -n "$host" ]] && stop_one "$host"
      done
    else
      stop_one "$HOST"
    fi
  ''
