# Tier A - VM integration tests: boot a NixOS VM and assert runtime state.
# Only runs on x86_64-linux. Gated behind `nix run .#validate -- --vm`.
# Each test is a `pkgs.testers.runNixOSTest` that boots a VM and runs a Python test script.
{inputs, ...}: {
  perSystem = {
    pkgs,
    lib,
    system,
    ...
  }: let
    helpers = import ./_lib/helpers.nix {inherit lib pkgs inputs;};

    mkTestNode = helpers.mkTestNode {
      hostSpecModule = ../_shared/host-spec-module.nix;
    };

    defaultTestSpec = helpers.defaultTestSpec;
  in
    # VM tests require x86_64-linux (KVM).
    lib.optionalAttrs (system == "x86_64-linux") {
      checks = {
        # --- vm-core: multi-user, SSH, NetworkManager, firewall, user/groups ---
        vm-core = pkgs.testers.runNixOSTest {
          node.specialArgs = {inherit inputs;};
          name = "vm-core";
          nodes.machine = mkTestNode {
            hostSpecValues = defaultTestSpec;
          };
          testScript = ''
            machine.wait_for_unit("multi-user.target")
            machine.wait_for_unit("sshd")
            machine.wait_for_unit("nftables.service")
            machine.succeed("nft list ruleset | grep -q 'chain input'")
            machine.succeed("id testuser")
            machine.succeed("groups testuser | grep -q testuser")
            machine.succeed("su - testuser -c 'which git'")
          '';
        };

        # --- vm-minimal: negative assertions proving the role doesn't
        # pull in fleet-specific DE/dev scopes (those live outside
        # nixfleet-scopes entirely) ---
        vm-minimal = pkgs.testers.runNixOSTest {
          node.specialArgs = {inherit inputs;};
          name = "vm-minimal";
          nodes.machine = mkTestNode {
            hostSpecValues = defaultTestSpec;
          };
          testScript = ''
            machine.wait_for_unit("multi-user.target")

            # Core always present (from core/nixos.nix)
            machine.succeed("su - testuser -c 'which git'")

            # No graphical (DE scopes don't live in nixfleet-scopes)
            machine.fail("which niri")

            # Docker should not be present (no dev scope in framework)
            machine.fail("systemctl is-active docker")
          '';
        };
      };
    };
}
