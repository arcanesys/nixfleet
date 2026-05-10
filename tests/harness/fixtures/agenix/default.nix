# Harness-fixture only: the age identity is in source; never sign anything
# outside this test scope.
{pkgs, ...}: let
  # Random-looking so grep can't false-positive on dictionary words.
  plaintext = "nixfleet-harness-secret-DO-NOT-LEAK-7fa4c2";
  identity = "AGE-SECRET-KEY-12VTL09QP8DQ44Z6078XVV4LPVG7E6AY7KYLSW34Q0Y8MXPQVY99S3X5R2F";
  recipient = "age1r5272q6tgd2ys22u8efxcs63w7h5vc0u5q9ya3f0tckygwm23fdqvvlq0z";
in
  pkgs.runCommand "harness-agenix-fixture" {
    nativeBuildInputs = [pkgs.age];
    inherit plaintext identity recipient;
  } ''
    set -euo pipefail
    mkdir -p "$out"
    printf '%s\n' "$identity" > "$out/identity.txt"
    chmod 600 "$out/identity.txt"
    printf '%s' "$plaintext" | age -r "$recipient" > "$out/secret.age"
    printf '%s' "$plaintext" > "$out/plaintext.txt"
  ''
