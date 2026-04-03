# Tier A — VM integration tests: boot a NixOS VM and assert runtime state.
# Only runs on x86_64-linux. Gated behind `nix run .#validate -- --vm`.
# Each test is a `pkgs.testers.nixosTest` that boots a VM and runs a Python test script.
{inputs, ...}: {
  perSystem = {
    pkgs,
    system,
    lib,
    ...
  }: let
    helpers = import ./_lib/helpers.nix {inherit lib;};

    mkTestNode = helpers.mkTestNode {
      inherit inputs;
      hostSpecModule = ../_shared/host-spec-module.nix;
    };

    defaultTestSpec = helpers.defaultTestSpec;
  in
    lib.optionalAttrs (system == "x86_64-linux") {
      checks = {
        # --- vm-core: multi-user, SSH, NetworkManager, firewall, user/groups ---
        vm-core = pkgs.testers.nixosTest {
          name = "vm-core";
          nodes.machine = mkTestNode {
            hostSpecValues = defaultTestSpec;
          };
          testScript = ''
            machine.wait_for_unit("multi-user.target")
            machine.wait_for_unit("sshd")
            machine.wait_for_unit("NetworkManager")
            machine.succeed("nft list ruleset | grep -q 'chain input'")
            machine.succeed("id testuser")
            machine.succeed("groups testuser | grep -q wheel")
            machine.succeed("su - testuser -c 'which zsh'")
            machine.succeed("su - testuser -c 'which git'")
          '';
        };

        # --- vm-minimal: negative test (core only, no scopes) ---
        vm-minimal = pkgs.testers.nixosTest {
          name = "vm-minimal";
          nodes.machine = mkTestNode {
            hostSpecValues =
              defaultTestSpec
              // {
                isMinimal = true;
              };
          };
          testScript = ''
            machine.wait_for_unit("multi-user.target")

            # Core always present (from core/nixos.nix)
            machine.succeed("su - testuser -c 'which zsh'")
            machine.succeed("su - testuser -c 'which git'")

            # No graphical (no scope modules in framework)
            machine.fail("which niri")

            # Docker should not be present (no dev scope in framework)
            machine.fail("systemctl is-active docker")
          '';
        };
      };
    };
}
