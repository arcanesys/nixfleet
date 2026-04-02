# mkHost — the single NixFleet API function.
# Returns a nixosSystem or darwinSystem.
# Closure over framework inputs (nixpkgs, home-manager, disko, etc.).
{
  inputs,
  lib,
}: let
  hostSpecModule = ../host-spec-module.nix;

  # Import scope modules as plain attrsets
  baseScope = import ../../scopes/_base.nix;
  impermanenceScope = import ../../scopes/_impermanence.nix;

  # Core modules (plain NixOS/Darwin modules)
  coreNixos = ../../core/_nixos.nix;
  coreDarwin = ../../core/_darwin.nix;

  # Service modules (auto-included, disabled by default)
  agentModule = ../../scopes/nixfleet/_agent.nix;
  controlPlaneModule = ../../scopes/nixfleet/_control-plane.nix;

  backupCmd = ''mv {} {}.nbkp.$(date +%Y%m%d%H%M%S) && ls -t {}.nbkp.* 2>/dev/null | tail -n +6 | xargs -r rm -f'';

  isDarwinPlatform = platform:
    builtins.elem platform ["aarch64-darwin" "x86_64-darwin"];
in
  {
    hostName,
    platform,
    stateVersion ? "24.11",
    hostSpec ? {},
    modules ? [],
    isVm ? false,
  }: let
    isDarwin = isDarwinPlatform platform;

    # Merge hostName + isDarwin into hostSpec (always present).
    # isDarwin must be in effectiveHostSpec so HM modules also see it
    # (the system-level {hostSpec.isDarwin = true;} only applies to the
    # Darwin system config, not to the separate HM module evaluation).
    effectiveHostSpec =
      {inherit hostName;}
      // hostSpec
      // lib.optionalAttrs isDarwin {inherit isDarwin;};

    # Framework NixOS modules injected by mkHost
    frameworkNixosModules =
      [
        {nixpkgs.hostPlatform = platform;}
        hostSpecModule
        {hostSpec = lib.mapAttrs (_: v: lib.mkDefault v) effectiveHostSpec;}
        # Override hostName without mkDefault (must match)
        {hostSpec.hostName = hostName;}
        inputs.disko.nixosModules.disko
        inputs.impermanence.nixosModules.impermanence
        coreNixos
        baseScope.nixos
        impermanenceScope.nixos
        agentModule
        controlPlaneModule
      ]
      ++ lib.optionals isVm [
        ../../_hardware/qemu/disk-config.nix
        ../../_hardware/qemu/hardware-configuration.nix
        ({
          lib,
          pkgs,
          ...
        }: {
          services.spice-vdagentd.enable = true;
          networking.useDHCP = lib.mkForce true;
          environment.variables.LIBGL_ALWAYS_SOFTWARE = "1";
          environment.systemPackages = [pkgs.mesa];
        })
      ];

    # Framework Darwin modules injected by mkHost
    frameworkDarwinModules = [
      {nixpkgs.hostPlatform = platform;}
      hostSpecModule
      {hostSpec = lib.mapAttrs (_: v: lib.mkDefault v) effectiveHostSpec;}
      {hostSpec.hostName = hostName;}
      {hostSpec.isDarwin = true;}
      coreDarwin
      baseScope.darwin
    ];

    # Home-Manager modules
    hmModules =
      [
        hostSpecModule
        baseScope.homeManager
      ]
      ++ lib.optionals (!isDarwin) [
        impermanenceScope.hmLinux
      ];

    # Build NixOS system
    # Framework inputs passed via specialArgs. Fleet modules access these as
    # the `inputs` argument. Fleet-specific customization uses hostSpec
    # extensions and plain NixOS modules — no separate input namespace.
    buildNixos = inputs.nixpkgs.lib.nixosSystem {
      specialArgs = {inherit inputs;};
      modules =
        frameworkNixosModules
        ++ [
          inputs.home-manager.nixosModules.home-manager
          {
            home-manager = {
              useGlobalPkgs = true;
              useUserPackages = true;
              backupCommand = backupCmd;
              users.${effectiveHostSpec.userName} = {
                imports =
                  hmModules
                  ++ [{hostSpec = effectiveHostSpec;}];
                home = {
                  inherit stateVersion;
                  username = effectiveHostSpec.userName;
                  homeDirectory = "/home/${effectiveHostSpec.userName}";
                  enableNixpkgsReleaseCheck = false;
                };
                systemd.user.startServices = "sd-switch";
              };
            };
          }
        ]
        ++ modules;
    };

    # Build Darwin system
    buildDarwin = inputs.darwin.lib.darwinSystem {
      specialArgs = {inherit inputs;};
      modules =
        frameworkDarwinModules
        ++ [
          inputs.home-manager.darwinModules.home-manager
          {
            home-manager = {
              useGlobalPkgs = true;
              backupCommand = backupCmd;
              users.${effectiveHostSpec.userName} = {
                imports =
                  hmModules
                  ++ [{hostSpec = effectiveHostSpec;}];
                home = {
                  inherit stateVersion;
                  username = effectiveHostSpec.userName;
                  homeDirectory = "/Users/${effectiveHostSpec.userName}";
                  enableNixpkgsReleaseCheck = false;
                };
              };
            };
          }
        ]
        ++ modules;
    };
  in
    if isDarwin
    then buildDarwin
    else buildNixos
