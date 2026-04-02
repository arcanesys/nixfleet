# WiFi Provisioning

## Purpose

Bootstrap WiFi connectivity on first boot using encrypted credentials. The framework provides the `hostSpec.wifiNetworks` option; the consuming fleet wires it to their secrets tool.

## Location

- `modules/_shared/host-spec-module.nix` (`wifiNetworks` option)

## How It Works

1. Host declares `wifiNetworks = ["home"];` in hostSpec
2. The fleet's secrets module decrypts `wifi-<name>` credentials at boot
3. A systemd service copies the `.nmconnection` file to NetworkManager's directory
4. NetworkManager picks up the connection and connects automatically

## Fleet Implementation Pattern

```nix
# In a fleet secrets module:
age.secrets."wifi-home" = {
  file = "${secretsRepo}/wifi-home.age";
  path = "/run/agenix/wifi-home";
};

# Systemd service to bootstrap WiFi
systemd.services."bootstrap-wifi" = {
  after = [ "agenix.service" ];
  before = [ "NetworkManager.service" ];
  # Copy .nmconnection from decrypted secret to NM directory
};
```

The framework provides the `wifiNetworks` flag; the fleet implements the actual provisioning.

## Links

- [Secrets Overview](README.md)
