{
  platform,
  pkgs,
}: let
  inherit (platform) qemuBin qemuAccel qemuFirmware basePkgs nixos-anywhere-bin mkScript sharedHelpers;
  lib = pkgs.lib;
in
  mkScript "test-vm" "End-to-end VM test: build, install, verify, cleanup" ''
    set -euo pipefail
    export PATH="${lib.makeBinPath basePkgs}:$PATH"

    ${sharedHelpers}

    HOST=""
    KEEP=0
    PORT_OVERRIDE="2299"
    IDENTITY_KEY=""
    RAM=4096
    CPUS=2

    while [[ $# -gt 0 ]]; do
      case "$1" in
        -h|--host) HOST="$2"; shift 2 ;;
        --keep) KEEP=1; shift ;;
        --ssh-port) PORT_OVERRIDE="$2"; shift 2 ;;
        --identity-key) IDENTITY_KEY="$2"; shift 2 ;;
        --ram) RAM="$2"; shift 2 ;;
        --cpus) CPUS="$2"; shift 2 ;;
        *) echo "Unknown argument: $1" >&2; exit 1 ;;
      esac
    done

    [ -z "$HOST" ] && echo -e "''${RED}-h HOST is required''${NC}" && exit 1

    SSH_PORT="''$PORT_OVERRIDE"
    WORK_DIR=$(mktemp -d -t test-vm-XXXXXX)
    DISK="''$WORK_DIR/disk.qcow2"
    PIDFILE="''$WORK_DIR/qemu.pid"

    cleanup() {
      [ -f "''$PIDFILE" ] && kill "$(cat "''$PIDFILE")" 2>/dev/null || true
      if [ "$KEEP" = "0" ]; then
        rm -rf "''$WORK_DIR"
      else
        echo -e "''${YELLOW}Kept: ''$WORK_DIR''${NC}"
      fi
    }
    trap cleanup EXIT

    echo -e "''${YELLOW}[1/6] Building ISO...''${NC}"
    build_iso

    echo -e "''${YELLOW}[2/6] Creating ephemeral disk...''${NC}"
    qemu-img create -f qcow2 "''$DISK" 5G

    echo -e "''${YELLOW}[3/6] Installing ''$HOST...''${NC}"
    ${qemuBin} \
      ${qemuAccel} \
      -m "''$RAM" \
      -smp "''$CPUS" \
      -drive file="''$DISK",format=qcow2,if=virtio \
      -nic user,model=virtio-net-pci,hostfwd=tcp::''${SSH_PORT}-:22 \
      -display none -serial null \
      -bios ${qemuFirmware} \
      -cdrom "''$ISO_FILE" -boot d \
      -daemonize -pidfile "''$PIDFILE"

    wait_ssh "''$SSH_PORT" 120

    provision_identity_key "$HOST" "''${IDENTITY_KEY:-}"

    echo -e "''${YELLOW}[4/6] Running nixos-anywhere...''${NC}"
    ${nixos-anywhere-bin} \
      --flake ".#$HOST" \
      --ssh-port "''$SSH_PORT" \
      --no-reboot \
      ''$EXTRA_FILES_ARGS \
      root@localhost
    [ -n "''${EXTRA_FILES_DIR:-}" ] && rm -rf "''$EXTRA_FILES_DIR" || true

    echo -e "''${YELLOW}[5/6] Rebooting from disk...''${NC}"
    kill "$(cat "''$PIDFILE")" 2>/dev/null || true
    sleep 2
    ${qemuBin} \
      ${qemuAccel} \
      -m "''$RAM" \
      -smp "''$CPUS" \
      -drive file="''$DISK",format=qcow2,if=virtio \
      -nic user,model=virtio-net-pci,hostfwd=tcp::''${SSH_PORT}-:22 \
      -display none -serial null \
      -bios ${qemuFirmware} \
      -daemonize -pidfile "''$PIDFILE"

    wait_ssh "''$SSH_PORT" 180

    echo -e "''${YELLOW}[6/6] Verifying...''${NC}"
    FAILURES=0
    VM_HOSTNAME=$(ssh ''$SSH_OPTS -p "''$SSH_PORT" root@localhost hostname 2>/dev/null)
    [ "''$VM_HOSTNAME" = "$HOST" ] && echo -e "  hostname: ''${GREEN}OK''${NC}" || { echo -e "  hostname: ''${RED}FAIL (got: ''$VM_HOSTNAME)''${NC}"; FAILURES=$((FAILURES+1)); }
    [ "$(ssh ''$SSH_OPTS -p "''$SSH_PORT" root@localhost systemctl is-active multi-user.target 2>/dev/null)" = "active" ] \
      && echo -e "  multi-user: ''${GREEN}OK''${NC}" || { echo -e "  multi-user: ''${RED}FAIL''${NC}"; FAILURES=$((FAILURES+1)); }
    [ "$(ssh ''$SSH_OPTS -p "''$SSH_PORT" root@localhost systemctl is-active sshd 2>/dev/null)" = "active" ] \
      && echo -e "  sshd: ''${GREEN}OK''${NC}" || { echo -e "  sshd: ''${RED}FAIL''${NC}"; FAILURES=$((FAILURES+1)); }

    if [ "$FAILURES" -gt 0 ]; then
      echo -e "''${RED}Verification failed ($FAILURES checks)''${NC}"
      exit 1
    fi
    echo -e "''${GREEN}All checks passed!''${NC}"
  ''
