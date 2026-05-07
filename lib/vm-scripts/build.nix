{
  platform,
  pkgs,
}: let
  inherit (platform) qemuBin qemuAccel qemuFirmware basePkgs nixos-anywhere-bin mkScript sharedHelpers;
  lib = pkgs.lib;
in
  mkScript "build-vm" "Install a VM host via nixos-anywhere (ISO boot + disko)" ''
    set -euo pipefail
    export PATH="${lib.makeBinPath basePkgs}:$PATH"

    ${sharedHelpers}

    HOST=""
    ALL=0
    REBUILD=0
    PORT_OVERRIDE=""
    IDENTITY_KEY=""
    RAM=4096
    CPUS=2
    DISK_SIZE="5G"
    VLAN_PORT=""

    while [[ $# -gt 0 ]]; do
      case "$1" in
        -h|--host) HOST="$2"; shift 2 ;;
        --all) ALL=1; shift ;;
        --rebuild) REBUILD=1; shift ;;
        --identity-key) IDENTITY_KEY="$2"; shift 2 ;;
        --ssh-port) PORT_OVERRIDE="$2"; shift 2 ;;
        --ram) RAM="$2"; shift 2 ;;
        --cpus) CPUS="$2"; shift 2 ;;
        --disk-size) DISK_SIZE="$2"; shift 2 ;;
        --vlan) VLAN_PORT="$2"; shift 2 ;;
        *) echo "Unknown argument: $1" >&2; exit 1 ;;
      esac
    done

    if [[ $ALL -eq 0 && -z "$HOST" ]]; then
      echo "Usage: nix run .#build-vm -- -h HOST [options]" >&2
      echo "       nix run .#build-vm -- --all [options]" >&2
      echo "" >&2
      echo "Options:" >&2
      echo "  -h HOST            Host to install" >&2
      echo "  --all              Install all hosts in nixosConfigurations" >&2
      echo "  --rebuild          Wipe and reinstall existing disk" >&2
      echo "  --identity-key PATH  Path to identity key for secrets decryption" >&2
      echo "  --ssh-port N       Override SSH port (default: auto-assigned)" >&2
      echo "  --ram MB           RAM in MB (default: 4096)" >&2
      echo "  --cpus N           CPU count (default: 2)" >&2
      echo "  --disk-size S      Disk size (default: 5G)" >&2
      echo "  --vlan PORT        Enable inter-VM VLAN on multicast port" >&2
      exit 1
    fi

    build_one() {
      local host="$1"
      echo -e "''${YELLOW}==> Building VM for host: $host''${NC}"

      assign_port "$host"
      compute_vm_ram "$host" 4096
      compute_vlan_args
      echo -e "''${GREEN}SSH port: ''$SSH_PORT''${NC}"

      local disk_path="''$VM_DIR/''${host}.qcow2"
      mkdir -p "''$VM_DIR"

      if [[ -f "''$disk_path" && $REBUILD -eq 0 ]]; then
        echo -e "''${YELLOW}Disk already exists at ''$disk_path - skipping (use --rebuild to reinstall)''${NC}"
        return 0
      fi

      if [[ -f "''$disk_path" ]]; then
        echo -e "''${YELLOW}Removing existing disk for rebuild...''${NC}"
        rm -f "''$disk_path"
      fi

      echo -e "''${YELLOW}Creating disk image (''${DISK_SIZE})...''${NC}"
      qemu-img create -f qcow2 "''$disk_path" "''$DISK_SIZE"

      echo -e "''${YELLOW}Booting ISO (headless)...''${NC}"
      ${qemuBin} \
        ${qemuAccel} \
        -m "''$RAM" \
        -smp "''$CPUS" \
        -drive file="''$disk_path",format=qcow2,if=virtio \
        -nic user,model=virtio-net-pci,hostfwd=tcp::"''$SSH_PORT"-:22 \
        ''$VLAN_ARGS \
        -display none -serial null \
        -bios ${qemuFirmware} \
        -cdrom "''$ISO_FILE" -boot d \
        -daemonize \
        -pidfile "''$VM_DIR/''${host}.pid"

      echo -e "''${YELLOW}Waiting for SSH...''${NC}"
      wait_ssh "''$SSH_PORT" 120

      provision_identity_key "$host" "''${IDENTITY_KEY:-}"

      echo -e "''${YELLOW}Installing via nixos-anywhere...''${NC}"
      ${nixos-anywhere-bin} \
        --flake ".#''${host}" \
        --ssh-port "''$SSH_PORT" \
        --phases kexec,disko,install \
        ''$EXTRA_FILES_ARGS \
        root@localhost
      [ -n "''${EXTRA_FILES_DIR:-}" ] && rm -rf "''$EXTRA_FILES_DIR" || true

      echo -e "''${YELLOW}Stopping ISO VM...''${NC}"
      if [[ -f "''$VM_DIR/''${host}.pid" ]]; then
        kill "$(cat "''$VM_DIR/''${host}.pid")" 2>/dev/null || true
        rm -f "''$VM_DIR/''${host}.pid"
      fi

      echo "''$SSH_PORT" > "''$VM_DIR/''${host}.port"
      echo -e "''${GREEN}==> ''${host} installed successfully (port ''$SSH_PORT)''${NC}"
    }

    build_iso

    if [[ $ALL -eq 1 ]]; then
      mapfile -t hosts_arr <<< "$(all_hosts)"
      for host in "''${hosts_arr[@]}"; do
        [[ -n "$host" ]] && build_one "$host"
      done
    else
      build_one "$HOST"
    fi
  ''
