# nixfleet-cp-bootstrap — operator bootstrap for Bundle C (#41).
#
# writeShellApplication pins runtime tools (openssl, ssh, jq,
# coreutils) so the script behaves consistently across operator
# workstations regardless of host OS (NixOS / macOS / Linux).
{pkgs}:
pkgs.writeShellApplication {
  name = "nixfleet-cp-bootstrap";
  runtimeInputs = with pkgs; [
    openssl
    openssh
    jq
    coreutils
  ];
  text = builtins.readFile ./bootstrap.sh;
  # writeShellApplication runs `shellcheck` on the body. The script
  # uses heredocs and printf hex escapes that shellcheck will analyse
  # cleanly — no excludes needed.
}
