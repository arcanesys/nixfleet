# mkVmApps — generate VM lifecycle apps for fleet repos.
#
# Usage in fleet flake.nix:
#   apps = nixfleet.lib.mkVmApps { inherit pkgs; };
#
# Returns: { build-vm, start-vm, stop-vm, clean-vm, test-vm, provision }
#
# ── Shared bash helpers (injected into every script via sharedHelpers) ───────
#
# GREEN/YELLOW/RED/NC — ANSI colour codes
# VM_DIR              — ''${XDG_DATA_HOME:-$HOME/.local/share}/nixfleet/vms
# SSH_OPTS            — StrictHostKeyChecking=no, UserKnownHostsFile=/dev/null, ConnectTimeout=2
#
# assign_port HOST
#   Sets SSH_PORT from sorted nixosConfigurations index (base 2201).
#   Honours PORT_OVERRIDE env var.
#
# wait_ssh PORT TIMEOUT_SECONDS
#   Polls SSH until ready, exits 1 on timeout.
#
# provision_identity_key HOST [KEY_PATH]
#   Copies an identity key into a temp dir for nixos-anywhere --extra-files.
#   Resolution order: explicit arg > ~/.keys/id_ed25519 > ~/.ssh/id_ed25519 > skip (warning).
#   Sets: EXTRA_FILES_DIR, EXTRA_FILES_ARGS
#
# build_iso
#   Runs `nix build .#iso`, sets ISO_FILE.
#
# all_hosts
#   Prints sorted nixosConfigurations names, one per line.
#
# ── Platform helpers ─────────────────────────────────────────────────────────
#
# qemuBin    — qemu-system-{x86_64,aarch64} for the current system
# qemuAccel  — -enable-kvm (Linux) | -accel hvf (Darwin)
# basePkgs   — [qemu coreutils openssh nix git]
# mkScript   — name -> description -> bash text -> flake app attrset
# nixos-anywhere-bin — path to nixos-anywhere (Linux only, Task 6)
#
# ─────────────────────────────────────────────────────────────────────────────
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

  qemuBin =
    {
      "x86_64-linux" = "qemu-system-x86_64";
      "aarch64-linux" = "qemu-system-aarch64";
      "aarch64-darwin" = "qemu-system-aarch64";
      "x86_64-darwin" = "qemu-system-x86_64";
    }.${
      system
    } or (throw "unsupported system: ${system}");

  qemuAccel =
    if isLinux
    then "-enable-kvm"
    else if isDarwin
    then "-accel hvf"
    else throw "unsupported system: ${system}";

  basePkgs = with pkgs; [qemu coreutils openssh nix git];

  sharedHelpers = ''
    GREEN='\033[1;32m'
    YELLOW='\033[1;33m'
    RED='\033[1;31m'
    NC='\033[0m'

    VM_DIR="''${XDG_DATA_HOME:-''$HOME/.local/share}/nixfleet/vms"
    SSH_OPTS="-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=2"

    assign_port() {
      local host="$1"
      if [ -n "''${PORT_OVERRIDE:-}" ]; then
        SSH_PORT="''$PORT_OVERRIDE"
        return
      fi
      local hosts
      hosts=$(nix eval .#nixosConfigurations --apply 'x: builtins.concatStringsSep "\n" (builtins.sort builtins.lessThan (builtins.attrNames x))' --raw 2>/dev/null)
      local idx=0
      while IFS= read -r name; do
        if [ "$name" = "$host" ]; then
          SSH_PORT=$((2201 + idx))
          return
        fi
        idx=$((idx + 1))
      done <<< "$hosts"
      echo -e "''${RED}Host '$host' not found in nixosConfigurations''${NC}" >&2
      exit 1
    }

    wait_ssh() {
      local port="$1" timeout="$2"
      local elapsed=0
      while ! ssh ''$SSH_OPTS -p "$port" root@localhost true 2>/dev/null; do
        sleep 1
        elapsed=$((elapsed + 1))
        if [ "$elapsed" -ge "$timeout" ]; then
          echo -e "''${RED}SSH timeout after ''${timeout}s''${NC}" >&2
          return 1
        fi
      done
      echo -e "''${GREEN}SSH ready (''${elapsed}s)''${NC}"
    }

    provision_identity_key() {
      local host="$1"
      local explicit_key="''${2:-}"
      EXTRA_FILES_DIR=$(mktemp -d)
      EXTRA_FILES_ARGS=""

      local key_src=""
      if [ -n "$explicit_key" ]; then
        if [ ! -f "$explicit_key" ]; then
          echo -e "''${RED}Identity key not found: $explicit_key''${NC}" >&2
          exit 1
        fi
        key_src="$explicit_key"
      elif [ -f "''$HOME/.keys/id_ed25519" ]; then
        key_src="''$HOME/.keys/id_ed25519"
      elif [ -f "''$HOME/.ssh/id_ed25519" ]; then
        key_src="''$HOME/.ssh/id_ed25519"
      fi

      if [ -n "$key_src" ]; then
        local vm_user
        vm_user="$(nix eval ".#nixosConfigurations.''${host}.config.hostSpec.userName" --raw 2>/dev/null || echo "root")"
        for prefix in "persist/home/$vm_user" "home/$vm_user"; do
          mkdir -p "''$EXTRA_FILES_DIR/$prefix/.keys"
          cp "$key_src" "''$EXTRA_FILES_DIR/$prefix/.keys/id_ed25519"
          chmod 600 "''$EXTRA_FILES_DIR/$prefix/.keys/id_ed25519"
        done
        EXTRA_FILES_ARGS="--extra-files ''$EXTRA_FILES_DIR"
        echo -e "''${GREEN}Provisioning identity key for ''$vm_user (from $key_src)''${NC}"
      else
        echo -e "''${YELLOW}No identity key found — secrets requiring host decryption will not work''${NC}"
        echo -e "''${YELLOW}Provide one with --identity-key PATH, or place at ~/.keys/id_ed25519''${NC}"
      fi
    }

    build_iso() {
      echo -e "''${YELLOW}Building custom ISO...''${NC}"
      local iso_path
      iso_path=$(nix build .#iso --no-link --print-out-paths)
      ISO_FILE=$(find "''$iso_path/iso" -name '*.iso' | head -1)
      if [ -z "''$ISO_FILE" ]; then
        echo -e "''${RED}No ISO found in output''${NC}" >&2
        exit 1
      fi
      echo -e "''${GREEN}ISO: ''$ISO_FILE''${NC}"
    }

    all_hosts() {
      nix eval .#nixosConfigurations --apply 'x: builtins.concatStringsSep "\n" (builtins.sort builtins.lessThan (builtins.attrNames x))' --raw 2>/dev/null
    }
  '';
in
  lib.optionalAttrs (isLinux || isDarwin) {
    # ── build-vm (Task 2) ──
    build-vm = mkScript "build-vm" "Install a VM host via nixos-anywhere (ISO boot + disko)" ''
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
      DISK_SIZE="20G"

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
        echo "  --disk-size S      Disk size (default: 20G)" >&2
        exit 1
      fi

      build_one() {
        local host="$1"
        echo -e "''${YELLOW}==> Building VM for host: $host''${NC}"

        assign_port "$host"
        echo -e "''${GREEN}SSH port: ''$SSH_PORT''${NC}"

        local disk_path="''$VM_DIR/''${host}.qcow2"
        mkdir -p "''$VM_DIR"

        if [[ -f "''$disk_path" && $REBUILD -eq 0 ]]; then
          echo -e "''${YELLOW}Disk already exists at ''$disk_path — skipping (use --rebuild to reinstall)''${NC}"
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
          -display none -serial null \
          -bios ${pkgs.OVMF.fd}/FV/OVMF.fd \
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
          --no-reboot \
          ''$EXTRA_FILES_ARGS \
          root@localhost
        [ -n "''${EXTRA_FILES_DIR:-}" ] && rm -rf "''$EXTRA_FILES_DIR"

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
        while IFS= read -r host; do
          [[ -n "$host" ]] && build_one "$host"
        done <<< "$(all_hosts)"
      else
        build_one "$HOST"
      fi
    '';

    # ── start-vm (Task 3) ──
    "start-vm" = mkScript "start-vm" "Start an installed VM as a headless daemon" ''
      set -euo pipefail
      export PATH="${lib.makeBinPath basePkgs}:$PATH"

      ${sharedHelpers}

      HOST=""
      ALL=0
      PORT_OVERRIDE=""
      RAM=1024
      CPUS=2

      while [[ $# -gt 0 ]]; do
        case "$1" in
          -h|--host) HOST="$2"; shift 2 ;;
          --all) ALL=1; shift ;;
          --ssh-port) PORT_OVERRIDE="$2"; shift 2 ;;
          --ram) RAM="$2"; shift 2 ;;
          --cpus) CPUS="$2"; shift 2 ;;
          *) echo "Unknown argument: $1" >&2; exit 1 ;;
        esac
      done

      [[ $ALL -eq 0 && -z "$HOST" ]] && echo -e "''${RED}Specify -h HOST or --all''${NC}" && exit 1

      start_one() {
        local host="$1"
        assign_port "$host"
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
          -nic user,model=virtio-net-pci,hostfwd=tcp::''$SSH_PORT-:22 \
          -display none -serial null \
          -bios ${pkgs.OVMF.fd}/FV/OVMF.fd \
          -daemonize -pidfile "$pidfile"

        echo -e "''${GREEN}[$host] Started on port ''$SSH_PORT — ssh -p ''$SSH_PORT root@localhost''${NC}"
      }

      if [[ $ALL -eq 1 ]]; then
        while IFS= read -r host; do
          [[ -n "$host" ]] && [ -f "''$VM_DIR/$host.qcow2" ] && start_one "$host"
        done <<< "$(all_hosts)"
      else
        start_one "$HOST"
      fi
    '';

    # ── stop-vm (Task 4) ──
    "stop-vm" = mkScript "stop-vm" "Stop a running VM daemon" ''
      set -euo pipefail
      export PATH="${lib.makeBinPath (with pkgs; [coreutils])}:$PATH"

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
        while IFS= read -r host; do
          [[ -n "$host" ]] && stop_one "$host"
        done <<< "$(all_hosts)"
      else
        stop_one "$HOST"
      fi
    '';

    # ── clean-vm (Task 4) ──
    "clean-vm" = mkScript "clean-vm" "Delete VM disk, pidfile, and port file" ''
      set -euo pipefail
      export PATH="${lib.makeBinPath (with pkgs; [coreutils])}:$PATH"

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
        while IFS= read -r host; do
          [[ -n "$host" ]] && clean_one "$host"
        done <<< "$(all_hosts)"
      else
        clean_one "$HOST"
      fi
    '';
    # ── test-vm (Task 5) ──
    "test-vm" = mkScript "test-vm" "End-to-end VM test: build, install, verify, cleanup" ''
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
      qemu-img create -f qcow2 "''$DISK" 20G

      echo -e "''${YELLOW}[3/6] Installing ''$HOST...''${NC}"
      ${qemuBin} \
        ${qemuAccel} \
        -m "''$RAM" \
        -smp "''$CPUS" \
        -drive file="''$DISK",format=qcow2,if=virtio \
        -nic user,model=virtio-net-pci,hostfwd=tcp::''${SSH_PORT}-:22 \
        -display none -serial null \
        -bios ${pkgs.OVMF.fd}/FV/OVMF.fd \
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
      [ -n "''${EXTRA_FILES_DIR:-}" ] && rm -rf "''$EXTRA_FILES_DIR"

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
        -bios ${pkgs.OVMF.fd}/FV/OVMF.fd \
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
    '';
  }
  // lib.optionalAttrs isLinux {
    # ── provision (Task 6, Linux-only — nixos-anywhere path: ${nixos-anywhere-bin}) ──
    "provision" = mkScript "provision" "Install NixOS on real hardware via nixos-anywhere" ''
      set -euo pipefail
      export PATH="${lib.makeBinPath (with pkgs; [coreutils openssh nix git])}:$PATH"

      ${sharedHelpers}

      HOST=""
      TARGET=""
      TARGET_SSH_PORT=22
      IDENTITY_KEY=""

      while [[ $# -gt 0 ]]; do
        case "$1" in
          -h|--host) HOST="$2"; shift 2 ;;
          --target) TARGET="$2"; shift 2 ;;
          --ssh-port) TARGET_SSH_PORT="$2"; shift 2 ;;
          --identity-key) IDENTITY_KEY="$2"; shift 2 ;;
          *) echo "Unknown argument: $1" >&2; exit 1 ;;
        esac
      done

      [ -z "$HOST" ] && echo -e "''${RED}-h HOST is required''${NC}" && exit 1
      [ -z "$TARGET" ] && echo -e "''${RED}--target USER@IP is required''${NC}" && exit 1

      echo -e "''${YELLOW}[1/4] Provisioning identity key...''${NC}"
      KEY_SRC=""
      if [ -n "''${IDENTITY_KEY:-}" ]; then
        [ ! -f "''$IDENTITY_KEY" ] && echo -e "''${RED}Identity key not found: ''$IDENTITY_KEY''${NC}" && exit 1
        KEY_SRC="''$IDENTITY_KEY"
      elif [ -f "''$HOME/.keys/id_ed25519" ]; then
        KEY_SRC="''$HOME/.keys/id_ed25519"
      elif [ -f "''$HOME/.ssh/id_ed25519" ]; then
        KEY_SRC="''$HOME/.ssh/id_ed25519"
      fi

      if [ -n "$KEY_SRC" ]; then
        VM_USER="$(nix eval ".#nixosConfigurations.''${HOST}.config.hostSpec.userName" --raw 2>/dev/null || echo "root")"
        for prefix in "/persist/home/$VM_USER" "/home/$VM_USER"; do
          ssh ''$SSH_OPTS -p "''$TARGET_SSH_PORT" "''$TARGET" "mkdir -p $prefix/.keys && chmod 700 $prefix/.keys"
          scp ''$SSH_OPTS -P "''$TARGET_SSH_PORT" "$KEY_SRC" "''$TARGET:$prefix/.keys/id_ed25519"
          ssh ''$SSH_OPTS -p "''$TARGET_SSH_PORT" "''$TARGET" "chmod 600 $prefix/.keys/id_ed25519"
        done
        echo -e "''${GREEN}Key provisioned for ''$VM_USER (from $KEY_SRC)''${NC}"
      else
        echo -e "''${YELLOW}No identity key found — skipping''${NC}"
        echo -e "''${YELLOW}Provide one with --identity-key PATH, or place at ~/.keys/id_ed25519''${NC}"
      fi

      echo -e "''${YELLOW}[2/4] Installing $HOST via nixos-anywhere...''${NC}"
      ${nixos-anywhere-bin} --flake ".#''$HOST" --ssh-port "''$TARGET_SSH_PORT" "''$TARGET"

      echo -e "''${YELLOW}[3/4] Waiting for reboot...''${NC}"
      sleep 10
      ELAPSED=0
      TARGET_HOST="$(echo "''$TARGET" | cut -d@ -f2)"
      TARGET_USER="$(echo "''$TARGET" | cut -d@ -f1)"
      while ! ssh ''$SSH_OPTS -p "''$TARGET_SSH_PORT" "''${TARGET_USER}@''${TARGET_HOST}" true 2>/dev/null; do
        sleep 1
        ELAPSED=$((ELAPSED + 1))
        if [ "$ELAPSED" -ge 300 ]; then
          echo -e "''${YELLOW}Host did not come back within 300s — check manually''${NC}"
          break
        fi
      done
      [ "$ELAPSED" -lt 300 ] && echo -e "''${GREEN}Host is back online (''${ELAPSED}s)''${NC}"

      echo -e "''${YELLOW}[4/4] Collecting SSH host key...''${NC}"
      HOST_KEY=$(ssh-keyscan -p "''$TARGET_SSH_PORT" "''$TARGET_HOST" 2>/dev/null | grep ed25519 | head -1) || true
      if [ -n "''${HOST_KEY:-}" ]; then
        echo -e "''${GREEN}SSH host public key:''${NC}"
        echo "''$HOST_KEY"
        echo ""
        echo -e "''${YELLOW}Add this to your secrets recipients and rekey''${NC}"
      else
        echo -e "''${YELLOW}Could not collect host key — retrieve manually with ssh-keyscan''${NC}"
      fi
    '';
  }
