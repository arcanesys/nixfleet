{
  platform,
  pkgs,
}: let
  inherit (platform) qemuBin qemuAccel qemuFirmware basePkgs spicePkgs mkScript sharedHelpers;
  lib = pkgs.lib;
in
  mkScript "start-vm" "Start an installed VM (headless or graphical)" ''
    set -euo pipefail
    export PATH="${lib.makeBinPath basePkgs}:$PATH"

    ${sharedHelpers}

    HOST=""
    ALL=0
    PORT_OVERRIDE=""
    RAM=1024
    CPUS=2
    VLAN_PORT=""
    DISPLAY_MODE="none"

    while [[ $# -gt 0 ]]; do
      case "$1" in
        -h|--host) HOST="$2"; shift 2 ;;
        --all) ALL=1; shift ;;
        --ssh-port) PORT_OVERRIDE="$2"; shift 2 ;;
        --ram) RAM="$2"; shift 2 ;;
        --cpus) CPUS="$2"; shift 2 ;;
        --vlan) VLAN_PORT="$2"; shift 2 ;;
        --display) DISPLAY_MODE="$2"; shift 2 ;;
        *) echo "Unknown argument: $1" >&2; exit 1 ;;
      esac
    done

    [[ "''$DISPLAY_MODE" == "spice" ]] && export PATH="${lib.makeBinPath spicePkgs}:''$PATH"

    if [[ ''$ALL -eq 1 && "''$DISPLAY_MODE" != "none" ]]; then
      echo -e "''${RED}--display requires -h HOST (not --all)''${NC}" >&2
      exit 1
    fi

    [[ $ALL -eq 0 && -z "$HOST" ]] && echo -e "''${RED}Specify -h HOST or --all''${NC}" && exit 1

    start_one() {
      local host="$1"
      assign_port "$host"
      compute_vlan_args
      compute_extra_hostfwd_args "$host"
      compute_display_args
      local disk="''$VM_DIR/$host.qcow2"
      local pidfile="''$VM_DIR/$host.pid"

      if [ ! -f "$disk" ]; then
        echo -e "''${RED}[$host] No disk found. Run build-vm first.''${NC}" >&2
        return 1
      fi

      if [ -f "$pidfile" ] && kill -0 "$(cat "$pidfile")" 2>/dev/null; then
        echo -e "''${YELLOW}[$host] Already running (PID $(cat "$pidfile"))''${NC}"
        return 0
      fi

      rm -f "$pidfile"
      ${qemuBin} \
        ${qemuAccel} \
        -m "''$RAM" \
        -smp "''$CPUS" \
        -drive file="$disk",format=qcow2,if=virtio \
        -nic user,model=virtio-net-pci,hostfwd=tcp::''$SSH_PORT-:22''$EXTRA_HOSTFWD_ARGS \
        ''$VLAN_ARGS \
        ''$DISPLAY_ARGS \
        -bios ${qemuFirmware} \
        ''$DAEMONIZE_ARGS -pidfile "$pidfile"

      if [ -n "''$DAEMONIZE_ARGS" ]; then
        echo -e "''${GREEN}[$host] Started on port ''$SSH_PORT - ssh -p ''$SSH_PORT root@localhost''${NC}"
      else
        echo -e "''${GREEN}[$host] Running in foreground (port ''$SSH_PORT) - close the viewer to stop''${NC}"
      fi
    }

    if [[ $ALL -eq 1 ]]; then
      mapfile -t hosts_arr <<< "$(all_hosts)"
      for host in "''${hosts_arr[@]}"; do
        [[ -n "$host" ]] && [ -f "''$VM_DIR/$host.qcow2" ] && start_one "$host"
      done
    else
      start_one "$HOST"
    fi
  ''
