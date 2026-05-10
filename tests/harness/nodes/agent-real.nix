{
  lib,
  pkgs,
  testCerts,
  controlPlaneHost,
  controlPlanePort,
  harnessMicrovmDefaults,
  agentHostName,
  agentPkg,
  signedFixture,
  pollIntervalSecs ? 10,
  # Optional OpenSSH-format ed25519 private key for /etc/ssh/ssh_host_ed25519_key.
  # When provided, the agent's evidence_signer reads it and signs
  # last_confirmed_at attestations with a pubkey matching the host's
  # declaration in fleet.nix (#43 contract).
  sshHostKey ? null,
  ...
}: {
  imports = [
    ../../../contracts/trust.nix
    ../../../contracts/persistence.nix
    ../../../modules/scopes/nixfleet/_agent.nix
  ];

  microvm = harnessMicrovmDefaults;

  # Default firewall startup blocks multi-user.target for 3+ minutes
  # under concurrent fleet-N boot.
  networking.firewall.enable = false;

  # microvm.nix uses networkd but won't auto-configure DHCP on user-net.
  networking.useNetworkd = lib.mkDefault true;
  systemd.network.networks."10-vm-net" = {
    matchConfig.Name = "en* eth*";
    networkConfig.DHCP = "yes";
    # FOOTGUN: RequiredForOnline=routable; default "degraded" fires before DHCP, masking failures.
    linkConfig.RequiredForOnline = "routable";
  };

  environment.etc =
    {
      "nixfleet-agent/ca.pem".source = "${testCerts}/ca.pem";
      "nixfleet-agent/${agentHostName}-cert.pem".source = "${testCerts}/${agentHostName}-cert.pem";
      "nixfleet-agent/${agentHostName}-key.pem".source = "${testCerts}/${agentHostName}-key.pem";
      "nixfleet-agent/test-trust.json".source = "${signedFixture}/test-trust.json";
    }
    // lib.optionalAttrs (sshHostKey != null) {
      # mode 0600 on the etc symlink - the agent's evidence_signer
      # opens this directly; ssh-key-derived agents (RFC-0003 §2) don't
      # need anything else on the SSH side because sshd is disabled in
      # the harness microvms.
      "ssh/ssh_host_ed25519_key".source = sshHostKey;
      "ssh/ssh_host_ed25519_key".mode = "0600";
    };

  networking.hosts."${controlPlaneHost}" = ["cp"];

  services.nixfleet-agent = {
    enable = true;
    package = agentPkg;
    controlPlaneUrl = "https://cp:${toString controlPlanePort}";
    machineId = agentHostName;
    pollInterval = pollIntervalSecs;
    trustFile = "/etc/nixfleet-agent/test-trust.json";
    stateDir = "/var/lib/nixfleet-agent";
    tls = {
      caCert = "/etc/nixfleet-agent/ca.pem";
      clientCert = "/etc/nixfleet-agent/${agentHostName}-cert.pem";
      clientKey = "/etc/nixfleet-agent/${agentHostName}-key.pem";
    };
  };

  # GOTCHA: ExecStartPre waits for default route; network-online.target fires prematurely on user-net (ENETUNREACH).
  systemd.services.nixfleet-agent.serviceConfig = {
    StandardOutput = "journal+console";
    StandardError = "journal+console";
    ExecStartPre = "${pkgs.bash}/bin/bash -c 'for i in $(seq 1 60); do ${pkgs.iproute2}/bin/ip route show default | grep -q . && exit 0; sleep 1; done; echo \"agent: no default route after 60s\" >&2; exit 1'";
  };

  system.stateVersion = lib.mkDefault "24.11";
}
