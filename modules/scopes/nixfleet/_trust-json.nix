# Shared: builds the JSON payload for /etc/nixfleet/{agent,cp}/trust.json
# from config.nixfleet.trust. Shape must match crates/nixfleet-proto's
# TrustConfig — see crates/nixfleet-proto/src/trust.rs and
# docs/trust-root-flow.md §3.4.
#
# The proto expects typed `{algorithm, public}` submodules for keys in
# KeySlot. modules/_trust.nix declares `ciReleaseKey` that way already,
# but `orgRootKey` still uses the legacy bare-string keySlotType (per
# CONTRACTS §II #3 org root keys are always ed25519, so a single string
# was sufficient). This helper promotes the bare-string slot into the
# proto struct shape so the binaries can deserialize the emission.
#
# `atticCacheKey` emits as a flat string (only `.current`) per proto's
# AtticKeySlot(String) newtype — see docs/trust-root-flow.md §3.2.
# Rotation for the attic key is handled by re-signing the closure
# history, not by a second active key, so `.previous` is unused on wire.
{trust}: let
  wrapEd25519 = key:
    if key == null
    then null
    else {
      algorithm = "ed25519";
      public = key;
    };
in {
  schemaVersion = 1;
  ciReleaseKey = trust.ciReleaseKey;
  atticCacheKey = trust.atticCacheKey.current;
  orgRootKey = {
    current = wrapEd25519 trust.orgRootKey.current;
    previous = wrapEd25519 trust.orgRootKey.previous;
    rejectBefore = trust.orgRootKey.rejectBefore;
  };
}
