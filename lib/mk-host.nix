{
  inputs,
  lib,
}: let
  hostSpecModule = ../contracts/host-spec.nix;

  coreNixos = ../modules/core/_nixos.nix;
  coreDarwin = ../modules/core/_darwin.nix;

  agentModule = ../modules/scopes/nixfleet/_agent.nix;
  agentDarwinModule = ../modules/scopes/nixfleet/_agent-darwin.nix;
  controlPlaneModule = ../modules/scopes/nixfleet/_control-plane.nix;
  cacheModule = ../modules/scopes/nixfleet/_cache.nix;
  microvmHostModule = ../modules/scopes/nixfleet/_microvm-host.nix;
  operatorModule = ../modules/scopes/nixfleet/_operator.nix;

  # Schema only; auto-imported so service modules can contribute to
  # nixfleet.persistence.directories via option merging without an impl.
  persistenceModule = ../contracts/persistence.nix;

  isDarwinPlatform = platform:
    builtins.elem platform ["aarch64-darwin" "x86_64-darwin"];
in
  {
    hostName,
    platform,
    stateVersion ? "24.11",
    hostSpec ? {},
    modules ? [],
    # Passthrough for consumer code; framework no longer reads it.
    isVm ? false,
    # LOADBEARING: extraInputs merged BENEATH framework inputs so framework wins (inputs.self -> nixfleet).
    extraInputs ? {},
  }: let
    isDarwin = isDarwinPlatform platform;

    effectiveHostSpec =
      {inherit hostName;}
      // hostSpec
      // lib.optionalAttrs isDarwin {inherit isDarwin;};

    frameworkNixosModules =
      [
        {nixpkgs.hostPlatform = platform;}
        hostSpecModule
        {hostSpec = lib.mapAttrs (_: v: lib.mkDefault v) effectiveHostSpec;}
        # LOADBEARING: hostName is not mkDefault; must match exactly.
        {hostSpec.hostName = hostName;}
        persistenceModule
        coreNixos
        agentModule
        controlPlaneModule
        cacheModule
        microvmHostModule
        operatorModule
      ]
      ++ lib.optionals isVm [
        ../tests/fixtures/qemu/disk-config.nix
        ../tests/fixtures/qemu/hardware-configuration.nix
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

    frameworkDarwinModules = [
      {nixpkgs.hostPlatform = platform;}
      hostSpecModule
      {hostSpec = lib.mapAttrs (_: v: lib.mkDefault v) effectiveHostSpec;}
      {hostSpec.hostName = hostName;}
      {hostSpec.isDarwin = true;}
      coreDarwin
      agentDarwinModule
      operatorModule
    ];

    effectiveInputs = extraInputs // inputs;

    buildNixos = inputs.nixpkgs.lib.nixosSystem {
      specialArgs = {inputs = effectiveInputs;};
      modules = [{system.stateVersion = lib.mkDefault stateVersion;}] ++ frameworkNixosModules ++ modules;
    };

    # stateVersion is Darwin-specific (integer); consumers set it themselves.
    buildDarwin = inputs.darwin.lib.darwinSystem {
      specialArgs = {inputs = effectiveInputs;};
      modules = frameworkDarwinModules ++ modules;
    };
  in
    if isDarwin
    then buildDarwin
    else buildNixos
