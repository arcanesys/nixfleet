# Integration test: validates the mkHost consumption pattern.
#
# Simulates a client repo consuming nixfleet.lib.nixfleet.mkHost.
# Proves that:
# 1. config.flake.lib.mkHost is accessible
# 2. mkHost produces valid nixosConfigurations
# 3. Core modules and scopes are available to hosts
# 4. Framework-level hostSpec options work through mkHost
#
# Run: nix build .#checks.x86_64-linux.integration-mock-client --no-link
{
  config,
  inputs,
  ...
}: let
  # Access mkHost via config.flake.lib (same as fleet.nix and examples)
  mkHost = config.flake.lib.mkHost;

  # Shared defaults (simulates a fleet's org let binding)
  mockDefaults = {
    userName = "testuser";
    timeZone = "America/New_York";
    locale = "en_US.UTF-8";
    keyboardLayout = "us";
  };

  # Build test hosts - no role import, just mkHost mechanism.
  # Represents a "bare" consumption pattern where the fleet doesn't
  # opt into any nixfleet-scopes roles yet.
  mockHost = mkHost {
    hostName = "mock-host";
    platform = "x86_64-linux";
    isVm = true;
    hostSpec = mockDefaults;
  };

  mockOverride = mkHost {
    hostName = "mock-override";
    platform = "x86_64-linux";
    isVm = true;
    hostSpec =
      mockDefaults
      // {
        userName = "override-user";
        timeZone = "Europe/London";
      };
  };

  # Assertions
  mockCfg = mockHost;
  overrideCfg = mockOverride;
  assert' = check: msg: {inherit check msg;};
in {
  flake.checks.x86_64-linux.integration-mock-client = let
    assertions = [
      # 1. Host evaluates and has hostSpec
      (assert' (mockCfg.config.hostSpec.hostName == "mock-host") "mock-host has correct hostName")
      (assert' (mockCfg.config.hostSpec.userName == "testuser") "userName from shared defaults propagates")

      # 2. Locale/timezone from shared defaults
      (assert' (mockCfg.config.time.timeZone == "America/New_York") "timeZone from shared defaults reaches NixOS config")
      (assert' (mockCfg.config.i18n.defaultLocale == "en_US.UTF-8") "locale from shared defaults reaches NixOS config")

      # 3. Framework mechanism: mkHost produces a valid nixosSystem
      # (isDarwin flag present as a platform marker)
      (assert' (mockCfg.config.hostSpec.isDarwin == false) "isDarwin defaults to false on Linux")

      # 4. Host-level override of shared defaults (mkDefault priority)
      (assert' (overrideCfg.config.hostSpec.userName == "override-user") "host-level userName overrides shared default")
      (assert' (overrideCfg.config.time.timeZone == "Europe/London") "host-level timeZone overrides shared default")
      (assert' (overrideCfg.config.hostSpec.hostName == "mock-override") "overridden host keeps its own hostName")
    ];
    failures = builtins.filter (a: !a.check) assertions;
    report =
      if failures == []
      then "All ${toString (builtins.length assertions)} integration assertions passed."
      else builtins.throw "Integration test failures:\n${builtins.concatStringsSep "\n" (map (f: "  FAIL: ${f.msg}") failures)}";
  in
    inputs.nixpkgs.legacyPackages.x86_64-linux.runCommand "integration-mock-client" {} ''
      echo "${report}"
      mkdir -p $out
      echo "${report}" > $out/result
    '';
}
