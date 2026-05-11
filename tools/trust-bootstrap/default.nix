# nixfleet-trust-bootstrap - operator tool that mints the offline fleet
# root CA and signs the TPM-bound issuance CA cert. Standing workflows:
# new-fleet stand-up, annual issuance-CA renewal, TPM rotation, disaster
# recovery.
#
# writeShellApplication pins runtime tools (openssl, ssh, jq,
# coreutils) so the script behaves consistently across operator
# workstations regardless of host OS (NixOS / macOS / Linux).
{pkgs}:
pkgs.writeShellApplication {
  name = "nixfleet-trust-bootstrap";
  runtimeInputs = with pkgs; [
    openssl
    openssh
    jq
    coreutils
  ];
  text = builtins.readFile ./bootstrap.sh;
  # writeShellApplication runs `shellcheck` on the body. The script
  # uses heredocs and printf hex escapes that shellcheck will analyse
  # cleanly - no excludes needed.
}
