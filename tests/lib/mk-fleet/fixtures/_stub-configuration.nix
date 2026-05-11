# Stub mimics a nixosConfiguration enough to satisfy host.configuration.
{}: {
  config.system.build.toplevel = {
    outPath = "/nix/store/0000000000000000000000000000000000000000-stub";
    drvPath = "/nix/store/0000000000000000000000000000000000000000-stub.drv";
  };
}
