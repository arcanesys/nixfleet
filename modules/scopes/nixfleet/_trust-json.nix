# LOADBEARING: shape must match proto::TrustConfig (consumed at runtime by agent + CP).
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
  # ciReleaseKey is already in submodule shape (`{algorithm, public}`)
  # so pass-through is direct. The new `successor` + `retireAt` fields
  # (nixfleet#63) inherit submodule defaults (null) and emit as JSON
  # null when unset — matches proto's `Option<...>` deserde.
  ciReleaseKey = trust.ciReleaseKey;
  cacheKeys = trust.cacheKeys;
  orgRootKey = {
    current = wrapEd25519 trust.orgRootKey.current;
    previous = wrapEd25519 trust.orgRootKey.previous;
    rejectBefore = trust.orgRootKey.rejectBefore;
    successor = wrapEd25519 trust.orgRootKey.successor;
    retireAt = trust.orgRootKey.retireAt;
  };
}
