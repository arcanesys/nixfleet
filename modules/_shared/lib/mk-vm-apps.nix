# mkVmApps — generate VM helper apps for fleet repos.
#
# Usage in fleet flake.nix:
#   apps = nixfleet.lib.mkVmApps { inherit pkgs; };
#
# Returns: { spawn-qemu, test-vm, spawn-utm } app definitions
# that operate on the calling repo's nixosConfigurations.
{inputs}: {pkgs}: let
  system = pkgs.stdenv.hostPlatform.system;
  isLinux = builtins.elem system ["x86_64-linux" "aarch64-linux"];
  isDarwin = builtins.elem system ["aarch64-darwin" "x86_64-darwin"];
  lib = pkgs.lib;
  mkScript = name: description: text: {
    type = "app";
    program = "${pkgs.writeShellScriptBin name text}/bin/${name}";
    meta.description = description;
  };
  nixos-anywhere-bin =
    if inputs.nixos-anywhere.packages ? ${system}
    then "${inputs.nixos-anywhere.packages.${system}.default}/bin/nixos-anywhere"
    else "echo 'nixos-anywhere not available on ${system}'; exit 1";
in
  lib.optionalAttrs isLinux {
    "spawn-qemu" = mkScript "spawn-qemu" "Launch a QEMU VM for NixFleet hosts" ''
      set -euo pipefail

      GREEN='\033[1;32m'
      YELLOW='\033[1;33m'
      RED='\033[1;31m'
      NC='\033[0m'

      PATH=${lib.makeBinPath (with pkgs; [qemu coreutils openssh nix git virt-viewer])}:''$PATH
      export LIBGL_DRIVERS_PATH="${pkgs.mesa}/lib/dri"
      export __EGL_VENDOR_LIBRARY_DIRS="${pkgs.mesa}/share/glvnd/egl_vendor.d"

      DISK="qemu-disk.qcow2"
      ISO=""
      RAM="4096"
      CPUS="2"
      SSH_PORT="2222"
      DISK_SIZE="20G"
      MODE="graphical"
      PERSISTENT=0
      HOST=""
      REBUILD=0

      usage() {
        echo "Usage: nix run .#spawn-qemu [-- [options]]"
        echo ""
        echo "QEMU VM launcher for NixFleet hosts."
        echo ""
        echo "Basic mode (default):"
        echo "  Boot a QEMU VM from an existing disk or ISO."
        echo ""
        echo "Persistent mode (--persistent -h HOST):"
        echo "  Build, install, and launch a named host with a persistent disk."
        echo "  Disk stored in ~/.local/share/nixfleet/vms/<HOST>.qcow2."
        echo "  Reuse on subsequent runs, or --rebuild to reinstall."
        echo ""
        echo "Options:"
        echo "  --iso PATH        Boot from ISO (for manual install)"
        echo "  --disk PATH       Disk image path (default: qemu-disk.qcow2)"
        echo "  --ram MB          RAM in MB (default: 4096)"
        echo "  --cpus N          CPU count (default: 2)"
        echo "  --ssh-port N      Host port for SSH forwarding (default: 2222)"
        echo "  --disk-size S     Disk size for new images (default: 20G)"
        echo "  --console         Headless mode (serial console, no GUI)"
        echo "  --graphical       GPU-accelerated GUI via SPICE (default)"
        echo "  --persistent      Persistent mode: install HOST, keep disk across runs"
        echo "  -h HOST           Host config to install (requires --persistent)"
        echo "  --rebuild         Wipe and reinstall (only with --persistent)"
        echo ""
        echo "Examples:"
        echo "  nix run .#spawn-qemu -- --iso iso/nixos-x86_64.iso    # boot from ISO"
        echo "  nix run .#spawn-qemu                                   # boot existing disk"
        echo "  nix run .#spawn-qemu -- --console                      # headless mode"
        echo "  nix run .#spawn-qemu -- --persistent -h web-02         # install + persistent disk"
        echo "  nix run .#spawn-qemu -- --persistent -h web-02 --rebuild  # reinstall from scratch"
        exit 0
      }

      while [[ ''$# -gt 0 ]]; do
        case "''$1" in
          --iso) ISO="''$2"; shift 2 ;;
          --disk) DISK="''$2"; shift 2 ;;
          --ram) RAM="''$2"; shift 2 ;;
          --cpus) CPUS="''$2"; shift 2 ;;
          --ssh-port) SSH_PORT="''$2"; shift 2 ;;
          --disk-size) DISK_SIZE="''$2"; shift 2 ;;
          --console) MODE="console"; shift ;;
          --graphical) MODE="graphical"; shift ;;
          --persistent) PERSISTENT=1; shift ;;
          -h) HOST="''$2"; shift 2 ;;
          --rebuild) REBUILD=1; shift ;;
          --help) usage ;;
          *) echo -e "''${RED}Unknown option: ''$1''${NC}"; usage ;;
        esac
      done

      setup_gl() {
        if [ ! -d /run/opengl-driver/lib/gbm ]; then
          sudo mkdir -p /run/opengl-driver/lib
          sudo ln -sf ${pkgs.mesa}/lib/gbm /run/opengl-driver/lib/gbm
        fi
      }

      boot_vm() {
        local disk="$1"
        echo -e "''${GREEN}VM: ''${CPUS} CPUs, ''${RAM}MB RAM, SSH on localhost:''${SSH_PORT} (''${MODE})''${NC}"

        DISPLAY_ARGS=""
        CLEANUP=""
        if [ "''$MODE" = "console" ]; then
          DISPLAY_ARGS="-nographic"
          echo -e "''${YELLOW}Press Ctrl-A X to exit QEMU''${NC}"
        else
          DISPLAY_ARGS="-device virtio-vga-gl -display egl-headless,rendernode=/dev/dri/renderD128 -spice port=5900,disable-ticketing=on"
          setup_gl
          (sleep 3 && remote-viewer spice://localhost:5900 2>/dev/null) &
          VIEWER_PID=''$!
          trap "kill ''$VIEWER_PID 2>/dev/null; ''${CLEANUP:-}" EXIT
        fi

        exec qemu-system-x86_64 \
          -enable-kvm -m "''$RAM" -smp "''$CPUS" \
          -drive file="$disk",format=qcow2,if=virtio \
          -nic user,model=virtio-net-pci,hostfwd=tcp::''${SSH_PORT}-:22 \
          ''$DISPLAY_ARGS \
          -bios ${pkgs.OVMF.fd}/FV/OVMF.fd
      }

      # ── Persistent mode: install HOST, keep disk ──
      if [ "''$PERSISTENT" = "1" ]; then
        if [ -z "''$HOST" ]; then
          echo -e "''${RED}--persistent requires -h HOST''${NC}"
          exit 1
        fi

        DISK_DIR="''${XDG_DATA_HOME:-''$HOME/.local/share}/nixfleet/vms"
        mkdir -p "''$DISK_DIR"
        DISK="''$DISK_DIR/''$HOST.qcow2"
        SSH_OPTS="-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=2"

        # If disk exists and no rebuild, just boot
        if [ -f "''$DISK" ] && [ "''$REBUILD" = "0" ]; then
          echo -e "''${GREEN}Booting existing VM: ''$HOST (''$DISK)''${NC}"
          boot_vm "''$DISK"
        fi

        # Build ISO, install, then boot
        echo -e "''${YELLOW}[1/5] Building custom ISO...''${NC}"
        ISO_PATH=$(nix build .#iso --no-link --print-out-paths)
        ISO_FILE=$(find "''$ISO_PATH/iso" -name '*.iso' | head -1)
        [ -z "''$ISO_FILE" ] && echo -e "''${RED}No ISO found''${NC}" && exit 1
        echo -e "''${GREEN}ISO: ''$ISO_FILE''${NC}"

        echo -e "''${YELLOW}[2/5] Creating disk: ''$DISK (''$DISK_SIZE)...''${NC}"
        rm -f "''$DISK"
        qemu-img create -f qcow2 "''$DISK" "''$DISK_SIZE"

        echo -e "''${YELLOW}[3/5] Booting from ISO (headless install)...''${NC}"
        PIDFILE="''$(mktemp)"
        qemu-system-x86_64 -enable-kvm -m "''$RAM" -smp "''$CPUS" \
          -drive file="''$DISK",format=qcow2,if=virtio \
          -nic user,model=virtio-net-pci,hostfwd=tcp::''${SSH_PORT}-:22 \
          -display none -serial null \
          -bios ${pkgs.OVMF.fd}/FV/OVMF.fd \
          -cdrom "''$ISO_FILE" -boot d -daemonize -pidfile "''$PIDFILE"

        echo -e "''${YELLOW}Waiting for SSH (timeout 120s)...''${NC}"
        ELAPSED=0
        while ! ssh ''$SSH_OPTS -p "''$SSH_PORT" root@localhost true 2>/dev/null; do
          sleep 1; ELAPSED=$((ELAPSED + 1))
          [ "''$ELAPSED" -ge 120 ] && echo -e "''${RED}SSH timeout''${NC}" && kill "$(cat "''$PIDFILE")" 2>/dev/null && exit 1
        done
        echo -e "''${GREEN}SSH ready (''${ELAPSED}s)''${NC}"

        EXTRA_FILES=$(mktemp -d)
        EXTRA_FILES_ARGS=""
        KEY_SRC="''$HOME/.keys/id_ed25519"
        [ ! -f "''$KEY_SRC" ] && KEY_SRC="''$HOME/.ssh/id_ed25519"
        if [ -f "''$KEY_SRC" ]; then
          VM_USER="''$(nix eval ".#nixosConfigurations.''$HOST.config.hostSpec.userName" --raw 2>/dev/null || echo "root")"
          for prefix in "persist/home/''$VM_USER" "home/''$VM_USER"; do
            mkdir -p "''$EXTRA_FILES/''$prefix/.keys"
            cp "''$KEY_SRC" "''$EXTRA_FILES/''$prefix/.keys/id_ed25519"
            chmod 600 "''$EXTRA_FILES/''$prefix/.keys/id_ed25519"
          done
          EXTRA_FILES_ARGS="--extra-files ''$EXTRA_FILES"
          echo -e "''${GREEN}Provisioning agenix key for ''$VM_USER''${NC}"
        fi

        echo -e "''${YELLOW}[4/5] Installing ''$HOST via nixos-anywhere...''${NC}"
        ${nixos-anywhere-bin} --flake ".#''$HOST" --ssh-port "''$SSH_PORT" --no-reboot ''$EXTRA_FILES_ARGS root@localhost
        rm -rf "''$EXTRA_FILES"

        kill "$(cat "''$PIDFILE")" 2>/dev/null || true
        rm -f "''$PIDFILE"
        sleep 2

        echo -e "''${YELLOW}[5/5] Launching graphical VM...''${NC}"
        boot_vm "''$DISK"
      fi

      # ── Basic mode: simple QEMU boot ──
      if [ ! -f "''$DISK" ]; then
        echo -e "''${YELLOW}Creating disk image: ''$DISK (''$DISK_SIZE)...''${NC}"
        qemu-img create -f qcow2 "''$DISK" "''$DISK_SIZE"
      fi

      BOOT_ARGS=""
      if [ -n "''$ISO" ]; then
        [ ! -f "''$ISO" ] && echo -e "''${RED}Error: ISO not found: ''$ISO''${NC}" && exit 1
        BOOT_ARGS="-cdrom ''$ISO -boot d"
        echo -e "''${YELLOW}Booting from ISO: ''$ISO''${NC}"
      else
        echo -e "''${YELLOW}Booting from disk: ''$DISK''${NC}"
      fi

      echo -e "''${GREEN}VM: ''${CPUS} CPUs, ''${RAM}MB RAM, SSH on localhost:''${SSH_PORT} (''${MODE})''${NC}"

      DISPLAY_ARGS=""
      CLEANUP=""
      if [ "''$MODE" = "console" ]; then
        DISPLAY_ARGS="-nographic"
        echo -e "''${YELLOW}Press Ctrl-A X to exit QEMU''${NC}"
      else
        DISPLAY_ARGS="-device virtio-vga-gl -display egl-headless,rendernode=/dev/dri/renderD128 -spice port=5900,disable-ticketing=on"
        setup_gl
        (sleep 3 && remote-viewer spice://localhost:5900 2>/dev/null) &
        VIEWER_PID=''$!
        trap "kill ''$VIEWER_PID 2>/dev/null; ''${CLEANUP:-}" EXIT
      fi

      qemu-system-x86_64 \
        -enable-kvm -m "''$RAM" -smp "''$CPUS" \
        -drive file="''$DISK",format=qcow2,if=virtio \
        -nic user,model=virtio-net-pci,hostfwd=tcp::''${SSH_PORT}-:22 \
        ''$DISPLAY_ARGS \
        -bios ${pkgs.OVMF.fd}/FV/OVMF.fd \
        ''$BOOT_ARGS
    '';

    "test-vm" = mkScript "test-vm" "Automated VM test cycle: build, install, verify" ''
      set -euo pipefail

      GREEN='\033[1;32m'
      YELLOW='\033[1;33m'
      RED='\033[1;31m'
      NC='\033[0m'

      PATH=${lib.makeBinPath (with pkgs; [qemu coreutils openssh nix git])}:''$PATH

      HOST="edge-01"
      KEEP=0
      SSH_PORT="2222"
      RAM="4096"
      CPUS="2"

      usage() {
        echo "Usage: nix run .#test-vm [-- [options]]"
        echo ""
        echo "Automated VM test cycle: build ISO -> boot -> install -> verify -> cleanup"
        echo ""
        echo "Options:"
        echo "  -h HOST        Host config to install (default: edge-01)"
        echo "  --keep         Keep temp dir and disk after test"
        echo "  --ssh-port N   Host port for SSH (default: 2222)"
        echo "  --ram MB       RAM in MB (default: 4096)"
        echo "  --cpus N       CPU count (default: 2)"
        echo "  --help         Show this help"
        echo ""
        echo "Examples:"
        echo "  nix run .#test-vm                          # test with 'edge-01' host"
        echo "  nix run .#test-vm -- -h web-02             # test with web-02 host"
        echo "  nix run .#test-vm -- -h edge-01 --keep     # keep disk for inspection"
        exit 0
      }

      while [[ ''$# -gt 0 ]]; do
        case "''$1" in
          -h) HOST="''$2"; shift 2 ;;
          --keep) KEEP=1; shift ;;
          --ssh-port) SSH_PORT="''$2"; shift 2 ;;
          --ram) RAM="''$2"; shift 2 ;;
          --cpus) CPUS="''$2"; shift 2 ;;
          --help) usage ;;
          *) echo -e "''${RED}Unknown option: ''$1''${NC}"; exit 1 ;;
        esac
      done

      TMPDIR=$(mktemp -d -t test-vm-XXXXXX)
      DISK="''$TMPDIR/disk.qcow2"
      PIDFILE="''$TMPDIR/qemu.pid"
      SSH_OPTS="-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=2"

      cleanup() {
        [ -f "''$PIDFILE" ] && kill "$(cat "''$PIDFILE")" 2>/dev/null || true
        [ "''$KEEP" = "0" ] && rm -rf "''$TMPDIR"
      }
      trap cleanup EXIT

      echo -e "''${YELLOW}[1/6] Building custom ISO...''${NC}"
      ISO_PATH=$(nix build .#iso --no-link --print-out-paths)
      ISO_FILE=$(find "''$ISO_PATH/iso" -name '*.iso' | head -1)
      [ -z "''$ISO_FILE" ] && echo -e "''${RED}No ISO found''${NC}" && exit 1

      echo -e "''${YELLOW}[2/6] Creating ephemeral disk...''${NC}"
      qemu-img create -f qcow2 "''$DISK" 20G

      echo -e "''${YELLOW}[3/6] Booting QEMU...''${NC}"
      qemu-system-x86_64 -enable-kvm -m "''$RAM" -smp "''$CPUS" \
        -drive file="''$DISK",format=qcow2,if=virtio \
        -nic user,model=virtio-net-pci,hostfwd=tcp::''${SSH_PORT}-:22 \
        -display none -serial null \
        -bios ${pkgs.OVMF.fd}/FV/OVMF.fd \
        -cdrom "''$ISO_FILE" -boot d -daemonize -pidfile "''$PIDFILE"

      ELAPSED=0
      while ! ssh ''$SSH_OPTS -p "''$SSH_PORT" root@localhost true 2>/dev/null; do
        sleep 1; ELAPSED=$((ELAPSED + 1))
        [ "''$ELAPSED" -ge 120 ] && echo -e "''${RED}SSH timeout''${NC}" && exit 1
      done

      EXTRA_FILES=$(mktemp -d)
      KEY_SRC="''$HOME/.keys/id_ed25519"
      [ ! -f "''$KEY_SRC" ] && KEY_SRC="''$HOME/.ssh/id_ed25519"
      EXTRA_FILES_ARGS=""
      if [ -f "''$KEY_SRC" ]; then
        VM_USER="''$(nix eval ".#nixosConfigurations.''$HOST.config.hostSpec.userName" --raw 2>/dev/null || echo "root")"
        for prefix in "persist/home/''$VM_USER" "home/''$VM_USER"; do
          mkdir -p "''$EXTRA_FILES/''$prefix/.keys"
          cp "''$KEY_SRC" "''$EXTRA_FILES/''$prefix/.keys/id_ed25519"
          chmod 600 "''$EXTRA_FILES/''$prefix/.keys/id_ed25519"
        done
        EXTRA_FILES_ARGS="--extra-files ''$EXTRA_FILES"
      fi

      echo -e "''${YELLOW}[4/6] Installing ''$HOST...''${NC}"
      ${nixos-anywhere-bin} --flake ".#''$HOST" --ssh-port "''$SSH_PORT" --no-reboot ''$EXTRA_FILES_ARGS root@localhost
      rm -rf "''$EXTRA_FILES"

      echo -e "''${YELLOW}[5/6] Rebooting from disk...''${NC}"
      kill "$(cat "''$PIDFILE")" 2>/dev/null || true; sleep 2
      qemu-system-x86_64 -enable-kvm -m "''$RAM" -smp "''$CPUS" \
        -drive file="''$DISK",format=qcow2,if=virtio \
        -nic user,model=virtio-net-pci,hostfwd=tcp::''${SSH_PORT}-:22 \
        -display none -serial null \
        -bios ${pkgs.OVMF.fd}/FV/OVMF.fd \
        -daemonize -pidfile "''$PIDFILE"

      ELAPSED=0
      while ! ssh ''$SSH_OPTS -p "''$SSH_PORT" root@localhost true 2>/dev/null; do
        sleep 1; ELAPSED=$((ELAPSED + 1))
        [ "''$ELAPSED" -ge 180 ] && echo -e "''${RED}SSH timeout after install''${NC}" && exit 1
      done

      echo -e "''${YELLOW}[6/6] Verifying...''${NC}"
      FAILURES=0
      VM_HOSTNAME=$(ssh ''$SSH_OPTS -p "''$SSH_PORT" root@localhost hostname 2>/dev/null)
      [ "''$VM_HOSTNAME" = "''$HOST" ] && echo -e "  hostname: ''${GREEN}OK''${NC}" || { echo -e "  hostname: ''${RED}FAIL''${NC}"; FAILURES=$((FAILURES+1)); }
      [ "$(ssh ''$SSH_OPTS -p "''$SSH_PORT" root@localhost systemctl is-active multi-user.target 2>/dev/null)" = "active" ] && echo -e "  multi-user: ''${GREEN}OK''${NC}" || { echo -e "  multi-user: ''${RED}FAIL''${NC}"; FAILURES=$((FAILURES+1)); }
      [ "$(ssh ''$SSH_OPTS -p "''$SSH_PORT" root@localhost systemctl is-active sshd 2>/dev/null)" = "active" ] && echo -e "  sshd: ''${GREEN}OK''${NC}" || { echo -e "  sshd: ''${RED}FAIL''${NC}"; FAILURES=$((FAILURES+1)); }
      [ "''$FAILURES" -gt 0 ] && echo -e "''${RED}Verification failed''${NC}" && exit 1
      echo -e "''${GREEN}All checks passed!''${NC}"
    '';
  }
  // lib.optionalAttrs isDarwin {
    "spawn-utm" = mkScript "spawn-utm" "Manage UTM VMs on macOS" ''
      set -euo pipefail
      PATH=${lib.makeBinPath (with pkgs; [coreutils openssh])}:''$PATH

      VM_NAME="nixos"
      HOST=""
      ACTION="setup"

      while [[ ''$# -gt 0 ]]; do
        case "''$1" in
          --name) VM_NAME="''$2"; shift 2 ;;
          --host) HOST="''$2"; shift 2 ;;
          --start) ACTION="start"; shift ;;
          --ip) ACTION="ip"; shift ;;
          --help|-h) echo "Usage: nix run .#spawn-utm [-- --host HOST --ip --start]"; exit 0 ;;
          *) echo "Unknown: ''$1"; exit 1 ;;
        esac
      done

      UTMCTL="/Applications/UTM.app/Contents/MacOS/utmctl"
      [ ! -x "''$UTMCTL" ] && echo "UTM not found" && exit 1

      get_ip() { ''$UTMCTL ip-address "''$VM_NAME" 2>/dev/null | head -1; }

      case "''$ACTION" in
        ip) IP=$(get_ip); [ -n "''$IP" ] && echo "''$IP" || exit 1 ;;
        start) ''$UTMCTL start "''$VM_NAME" 2>/dev/null || true
          for i in $(seq 1 30); do
            IP=$(get_ip); [ -n "''$IP" ] && echo "VM at ''$IP" && exit 0; sleep 2
          done; echo "Could not detect IP" ;;
        setup) echo "UTM Setup: create VM, boot ISO, passwd, then: nix run .#spawn-utm -- --ip" ;;
      esac
    '';
  }
