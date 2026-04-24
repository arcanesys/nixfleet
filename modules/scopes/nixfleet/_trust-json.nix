# Shared: builds the JSON payload for /etc/nixfleet/{agent,cp}/trust.json
# from config.nixfleet.trust. Shape must match crates/nixfleet-proto's
# TrustConfig — see crates/nixfleet-proto/src/trust.rs and
# docs/trust-root-flow.md §3.4.
#
# `ciReleaseKey` is already in proto shape on the option side (typed
# {algorithm, public} submodules per CONTRACTS §II #1) and passes
# through unchanged.
#
# `atticCacheKey` and `orgRootKey` store bare-string key material on the
# option side (keySlotType in modules/_trust.nix). They're pinned to one
# algorithm each per CONTRACTS §II #2 (attic-native) / §II #3 (ed25519),
# so the algorithm doesn't need to be declared per-slot. This helper
# emits both slots as `{current, previous, rejectBefore}` objects
# matching proto's AtticKeySlot / KeySlot shape, and promotes orgRootKey
# strings into typed TrustedPubkey entries.
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
  atticCacheKey = {
    current = trust.atticCacheKey.current;
    previous = trust.atticCacheKey.previous;
    rejectBefore = trust.atticCacheKey.rejectBefore;
  };
  orgRootKey = {
    current = wrapEd25519 trust.orgRootKey.current;
    previous = wrapEd25519 trust.orgRootKey.previous;
    rejectBefore = trust.orgRootKey.rejectBefore;
  };
}
