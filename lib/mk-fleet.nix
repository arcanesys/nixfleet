# LOADBEARING: output is canonicalized to JCS before signing - no floats, opaque derivations, or attrsets with significant iteration order.
{lib}: let
  inherit (lib) mkOption types;

  # LOADBEARING: selector precedence is `not` > `and` > base OR over (tags, tagsAny, hosts, channel, all); `not`/`and` are recursive.
  selectorType = types.submodule {
    options = {
      tags = mkOption {
        type = types.listOf types.str;
        default = [];
        description = "Host has ALL listed tags.";
      };
      tagsAny = mkOption {
        type = types.listOf types.str;
        default = [];
        description = "Host has ANY listed tag.";
      };
      hosts = mkOption {
        type = types.listOf types.str;
        default = [];
      };
      channel = mkOption {
        type = types.nullOr types.str;
        default = null;
      };
      all = mkOption {
        type = types.bool;
        default = false;
      };
      and = mkOption {
        type = types.listOf selectorType;
        default = [];
        description = "Host matches ALL listed sub-selectors (intersection).";
      };
      not = mkOption {
        type = types.nullOr selectorType;
        default = null;
        description = "Host matches iff it does NOT match the given sub-selector (negation).";
      };
    };
  };

  # Issue #88: declarative commit pin. Same shape on host, tag, channel  -
  # most-specific-wins resolution lives in `resolvePin` below. `expiresAt`
  # is RFC3339 / ISO-8601 string; date arithmetic happens in
  # `nixfleet-release` (chrono) - pure Nix has no robust date parsing.
  pinType = types.submodule {
    options = {
      commit = mkOption {
        type = types.str;
        description = ''
          Source-control rev the host's closure should be built from.
          MUST be a full 40-char SHA - `nixfleet-release` passes this
          verbatim to `nix build "<source>?rev=<commit>#..."`, and Nix's
          flake-ref parser rejects short SHAs / tag names with
          `hash has wrong length for hash algorithm 'sha1'`. Resolve
          short refs operator-side (`git rev-parse <ref>`) before
          declaring the pin.
        '';
      };
      reason = mkOption {
        type = types.str;
        description = ''
          Free-form operator note (CVE ref, audit window, debug
          context). Surfaced verbatim in `nixfleet status` and
          dashboards; not parsed.
        '';
      };
      expiresAt = mkOption {
        type = types.nullOr types.str;
        default = null;
        example = "2026-06-01T00:00:00Z";
        description = ''
          RFC3339 hard expiry. `nixfleet-release` filters expired pins
          at release time so they stop affecting the build path. Hosts
          with an expired pin fall back to the current release commit.
          `null` means no expiry - pin holds until the operator removes
          it from the declaration.
        '';
      };
    };
  };

  hostType = types.submodule {
    options = {
      system = mkOption {type = types.str;};
      configuration = mkOption {
        type = types.unspecified;
        description = "A nixosConfiguration.";
      };
      tags = mkOption {
        type = types.listOf types.str;
        default = [];
      };
      channel = mkOption {type = types.str;};
      pubkey = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = ''
          Host SSH ed25519 public key (OpenSSH format). Used by the control
          plane to verify probe-output signatures and bind the host's mTLS
          client cert at enrollment. `null` means the host has not been
          enrolled yet; it appears in the fleet schema but signed artifacts
          from it cannot be verified.
        '';
      };
      pin = mkOption {
        type = types.nullOr pinType;
        default = null;
        description = ''
          Per-host commit pin (issue #88). Most-specific level: when set,
          overrides any tag- or channel-level pin the host is otherwise
          eligible for. See `mkFleet`'s pin precedence: host > tag > channel.
        '';
      };
    };
  };

  revocationType = types.submodule {
    options = {
      hostname = mkOption {
        type = types.str;
        description = "Hostname whose certs are being revoked.";
      };
      notBefore = mkOption {
        type = types.str;
        description = ''
          RFC3339 timestamp. Any cert for `hostname` whose
          notBefore is strictly older than this is rejected at
          mTLS handshake time.
        '';
      };
      reason = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Free-form operator note (decommissioned, compromised, rotated, etc.).";
      };
      revokedBy = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Who declared the revocation. Surfaces in audit logs.";
      };
    };
  };

  bootstrapNonceType = types.submodule {
    options = {
      nonce = mkOption {
        type = types.str;
        description = ''
          Hex-encoded nonce from the token's claims. Matches
          `BootstrapToken.claims.nonce` exactly. CP refuses any
          `/v1/enroll` whose nonce is not present in the signed
          allowlist.
        '';
      };
      hostname = mkOption {
        type = types.str;
        description = ''
          Host this nonce is valid for. Must match the token's
          `claims.hostname`; defends against mis-targeted token swap.
        '';
      };
      expiresAt = mkOption {
        type = types.str;
        description = ''
          RFC3339 timestamp. Authoritative validity window - may be
          tighter than the token's own `expires_at` claim.
          `nixfleet-release` prunes entries with `expiresAt < signedAt`
          before signing the artifact.
        '';
      };
      mintedAt = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Optional audit trail: when the token was minted.";
      };
      mintedBy = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Optional audit trail: who minted the token.";
      };
    };
  };

  tagType = types.submodule {
    options = {
      description = mkOption {
        type = types.str;
        default = "";
      };
      pin = mkOption {
        type = types.nullOr pinType;
        default = null;
        description = ''
          Tag-scoped commit pin (issue #88). Applies to every host that
          carries this tag, unless overridden by a host-level pin. A
          host carrying multiple tags that BOTH have pins is rejected
          at eval time - operator must disambiguate (typically by
          moving one pin to a host-level declaration).
        '';
      };
    };
  };

  waveType = types.submodule {
    options = {
      selector = mkOption {type = selectorType;};
      soakMinutes = mkOption {
        type = types.int;
        default = 0;
      };
    };
  };

  policyType = types.submodule {
    options = {
      strategy = mkOption {type = types.enum ["canary" "all-at-once" "staged"];};
      waves = mkOption {
        type = types.listOf waveType;
        default = [];
      };
      healthGate = mkOption {
        type = types.attrs;
        default = {};
      };
      onHealthFailure = mkOption {
        type = types.enum ["halt" "rollback-and-halt"];
        default = "rollback-and-halt";
      };
    };
  };

  channelType = types.submodule {
    options = {
      description = mkOption {
        type = types.str;
        default = "";
      };
      pin = mkOption {
        type = types.nullOr pinType;
        default = null;
        description = ''
          Channel-scoped commit pin (issue #88). Applies to every host on
          this channel that does NOT carry a more-specific (host- or tag-
          level) pin. Useful for "freeze stable on commit X during the
          audit window" semantics.
        '';
      };
      rolloutPolicy = mkOption {type = types.str;};
      reconcileIntervalMinutes = mkOption {
        type = types.int;
        default = 30;
      };
      signingIntervalMinutes = mkOption {
        type = types.int;
        default = 60;
        description = ''
          How often CI re-signs `fleet.resolved` for this channel.
          Sets the replay-defense floor: a consumer accepts an artifact for
          at least this long before refresh is expected.
        '';
      };
      freshnessWindow = mkOption {
        type = types.int;
        description = ''
          Minutes a signed `fleet.resolved` artifact is accepted by agents
          after `meta.signedAt`. MUST be ≥ 2 × signingIntervalMinutes so a
          single missed signing run does not strand agents.
        '';
      };
      compliance = mkOption {
        type = types.submodule {
          options = {
            mode = mkOption {
              type = types.enum ["disabled" "permissive" "enforce"];
              default = "enforce";
              description = ''
                Compliance gate policy shared by the static gate
                (mk-fleet eval) and the runtime gate (agent
                post-activation).

                - `disabled`: gate not run.
                - `permissive`: failing static evidence emits a
                  `lib.warn` per failing host/control; eval succeeds.
                - `enforce`: failing static evidence throws at fleet
                  eval. Default - matches the prior `strict = true`
                  semantics.
              '';
            };
            frameworks = mkOption {
              type = types.listOf types.str;
              default = [];
            };
          };
        };
        default = {};
      };
    };
  };

  # Host-level DAG edge. `gated` waits for `gates` to reach Soaked /
  # Converged within the same rollout. Both hosts MUST be on the same
  # channel - cross-channel ordering is `channelEdges`'s job.
  hostEdgeType = types.submodule {
    options = {
      gated = mkOption {
        type = types.str;
        description = "Host whose dispatch is held until `gates` completes.";
      };
      gates = mkOption {
        type = types.str;
        description = "Host that must reach Soaked/Converged before `gated` dispatches.";
      };
      reason = mkOption {
        type = types.str;
        default = "";
      };
    };
  };

  # Cross-channel ordering edge. Canonical names match the host-level
  # `Edge` (gated/gates): `gates` is the predecessor that runs first;
  # `gated` is the dependent that holds until `gates` converges.
  #
  # `before`/`after` are accepted as deprecated aliases - older
  # fleet.nix files keep working without an atomic rename. The
  # validation block below errors if both legacy + canonical names
  # are set on the same edge so the operator picks one shape.
  channelEdgeType = types.submodule {
    options = {
      gates = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Predecessor channel - must converge before `gated` opens.";
      };
      gated = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Dependent channel - held until `gates` converges.";
      };
      before = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "DEPRECATED alias for `gates`. Update to `gates` on next edit.";
      };
      after = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "DEPRECATED alias for `gated`. Update to `gated` on next edit.";
      };
      reason = mkOption {
        type = types.str;
        default = "";
      };
    };
  };

  # Normalize a channelEdge record from either field naming to
  # `{gates, gated, reason}`. Errors when both shapes are set on
  # the same edge - the operator must pick one.
  normalizeChannelEdge = e: let
    g =
      if e.gates != null
      then e.gates
      else e.before;
    d =
      if e.gated != null
      then e.gated
      else e.after;
  in {
    gates = g;
    gated = d;
    reason = e.reason;
  };

  budgetType = types.submodule {
    options = {
      selector = mkOption {type = selectorType;};
      maxInFlight = mkOption {
        type = types.nullOr types.int;
        default = null;
      };
      maxInFlightPct = mkOption {
        type = types.nullOr types.int;
        default = null;
      };
    };
  };

  # LOADBEARING: edges are walked "gated -> gates" (dependent -> predecessor).
  # Operates on the NORMALIZED form (post `normalizeChannelEdge`) so legacy
  # `before`/`after` and canonical `gates`/`gated` shapes both flow through
  # the same DAG check.
  hasCycle = edges: let
    adj =
      lib.foldl' (
        acc: e: let
          current = acc.${e.gated} or [];
        in
          acc // {${e.gated} = current ++ [e.gates];}
      ) {}
      edges;
    nodes = lib.unique (map (e: e.gated) edges ++ map (e: e.gates) edges);
    visit = node: path: visited:
      if builtins.elem node path
      then {
        cycle = true;
        path = path ++ [node];
        visited = visited;
      }
      else if builtins.elem node visited
      then {
        cycle = false;
        path = path;
        visited = visited;
      }
      else let
        children = adj.${node} or [];
        walk = c: acc:
          if acc.cycle
          then acc
          else let
            r = visit c (path ++ [node]) acc.visited;
          in
            if r.cycle
            then r
            else {
              cycle = false;
              path = acc.path;
              visited = r.visited ++ [c];
            };
        result =
          lib.foldl' (a: c: walk c a) {
            cycle = false;
            path = [];
            visited = visited;
          }
          children;
      in
        if result.cycle
        then result
        else {
          cycle = false;
          path = [];
          visited = result.visited ++ [node];
        };
    scan = nodes:
      lib.foldl' (
        acc: n:
          if acc.cycle
          then acc
          else visit n [] acc.visited
      ) {
        cycle = false;
        path = [];
        visited = [];
      }
      nodes;
  in
    (scan nodes).cycle;

  resolveSelector = sel: hosts: let
    names = lib.attrNames hosts;
    matchHost = s: n: h:
      if s.not != null
      then !(matchHost s.not n h)
      else if s.and != []
      then lib.all (sub: matchHost sub n h) s.and
      else
        s.all
        || (s.hosts != [] && builtins.elem n s.hosts)
        || (s.channel != null && h.channel == s.channel)
        || (s.tags != [] && lib.all (t: builtins.elem t h.tags) s.tags)
        || (s.tagsAny != [] && lib.any (t: builtins.elem t h.tags) s.tagsAny);
  in
    builtins.filter (n: matchHost sel n hosts.${n}) names;

  checkInvariants = cfg: let
    hostNames = lib.attrNames cfg.hosts;
    channelNames = lib.attrNames cfg.channels;
    policyNames = lib.attrNames cfg.rolloutPolicies;

    hostChannelErrors =
      lib.concatMap (
        n:
          lib.optional (!builtins.elem cfg.hosts.${n}.channel channelNames)
          "host '${n}' references unknown channel '${cfg.hosts.${n}.channel}'"
      )
      hostNames;

    channelPolicyErrors =
      lib.concatMap (
        n:
          lib.optional (!builtins.elem cfg.channels.${n}.rolloutPolicy policyNames)
          "channel '${n}' references unknown rollout policy '${cfg.channels.${n}.rolloutPolicy}'"
      )
      channelNames;

    edgeErrors =
      lib.concatMap (
        e:
          lib.optional (!builtins.elem e.gated hostNames) "edge.gated references unknown host '${e.gated}'"
          ++ lib.optional (!builtins.elem e.gates hostNames) "edge.gates references unknown host '${e.gates}'"
          ++ lib.optional (
            (cfg.hosts.${e.gated}.channel or null)
            != null
            && (cfg.hosts.${e.gates}.channel or null) != null
            && cfg.hosts.${e.gated}.channel != cfg.hosts.${e.gates}.channel
          ) "edge.gated '${e.gated}' (channel '${cfg.hosts.${e.gated}.channel}') and edge.gates '${e.gates}' (channel '${cfg.hosts.${e.gates}.channel}') are on different channels; use channelEdges for cross-channel ordering"
      )
      cfg.edges;

    # Detect mixed-shape entries before normalization - operator must
    # pick one naming. Cleaner than silently picking gates+after.
    channelEdgeShapeErrors =
      lib.concatMap (
        e:
          lib.optional ((e.gates != null) && (e.before != null))
          "channelEdges entry sets both `gates` and `before` - pick one (canonical: `gates`)"
          ++ lib.optional ((e.gated != null) && (e.after != null))
          "channelEdges entry sets both `gated` and `after` - pick one (canonical: `gated`)"
          ++ lib.optional ((e.gates == null) && (e.before == null))
          "channelEdges entry must set `gates` (or legacy alias `before`)"
          ++ lib.optional ((e.gated == null) && (e.after == null))
          "channelEdges entry must set `gated` (or legacy alias `after`)"
      )
      cfg.channelEdges;

    # All downstream channelEdge-aware code reads from this normalized
    # list. Single shape ⇒ no per-site if-else for the legacy fields.
    normalizedChannelEdges = map normalizeChannelEdge cfg.channelEdges;

    channelEdgeErrors =
      lib.concatMap (
        e:
          lib.optional (!builtins.elem e.gates channelNames) "channelEdges.gates references unknown channel '${e.gates}'"
          ++ lib.optional (!builtins.elem e.gated channelNames) "channelEdges.gated references unknown channel '${e.gated}'"
          ++ lib.optional (e.gates == e.gated) "channelEdges entry has gates == gated ('${e.gates}'); use a wave-staged policy for intra-channel ordering instead"
      )
      normalizedChannelEdges;

    configurationErrors =
      lib.concatMap (
        n: let
          h = cfg.hosts.${n};
          isValid =
            builtins.isAttrs h.configuration
            && h.configuration ? config
            && h.configuration.config ? system
            && h.configuration.config.system ? build
            && h.configuration.config.system.build ? toplevel;
        in
          lib.optional (!isValid)
          "host '${n}' configuration is not a valid nixosConfiguration (missing config.system.build.toplevel)"
      )
      hostNames;

    complianceErrors =
      lib.concatMap (
        channelName: let
          c = cfg.channels.${channelName};
          bad = lib.filter (f: !(builtins.elem f cfg.complianceFrameworks)) c.compliance.frameworks;
        in
          map (f: "channel '${channelName}' references unknown compliance framework '${f}'") bad
      )
      (lib.attrNames cfg.channels);

    # `hasCycle` expects edges with `gates`/`gated`. Host-level `edges`
    # already use that schema; channelEdges flow through
    # `normalizedChannelEdges` so legacy `before`/`after` entries also
    # work without per-site translation.
    cycleErrors = lib.optional (hasCycle cfg.edges) "edges form a cycle; the DAG invariant is violated";

    channelCycleErrors = lib.optional (hasCycle normalizedChannelEdges) "channelEdges form a cycle; cross-channel ordering must be a DAG";

    freshnessErrors =
      lib.concatMap (
        channelName: let
          c = cfg.channels.${channelName};
        in
          lib.optional (c.freshnessWindow < 2 * c.signingIntervalMinutes)
          "channel '${channelName}': freshnessWindow (${toString c.freshnessWindow}) must be ≥ 2 × signingIntervalMinutes (${toString c.signingIntervalMinutes})"
      )
      (lib.attrNames cfg.channels);

    resolvedComplianceMode = channelName: cfg.channels.${channelName}.compliance.mode;

    # LOADBEARING: shared by enforce + permissive; only the action on failures differs (throw vs lib.warn).
    staticFailuresForChannels = channelNames: let
      hostsOnChannels =
        lib.filter (n: builtins.elem cfg.hosts.${n}.channel channelNames) (lib.attrNames cfg.hosts);
    in
      lib.concatMap (
        hostName: let
          host = cfg.hosts.${hostName};
          probes = host.configuration.config.compliance.evidence.probes or {};
          probeNames = lib.attrNames probes;
          # LOADBEARING: only static + both controls participate in the build-time gate.
          staticOrBoth =
            lib.filter (
              p: let
                t = probes.${p}.type or "runtime";
              in
                t == "static" || t == "both"
            )
            probeNames;
          failures =
            lib.filter (
              p: let
                ev = probes.${p}.staticEvidence or null;
              in
                ev != null && (ev.passed or true) == false
            )
            staticOrBoth;
          mode = resolvedComplianceMode host.channel;
        in
          map (p: "host '${hostName}' (channel '${host.channel}', ${mode}): static control '${p}' failed - ${lib.generators.toPretty {} (probes.${p}.staticEvidence.evidence or {})}") failures
      )
      hostsOnChannels;

    enforceChannels =
      lib.filter (n: resolvedComplianceMode n == "enforce") (lib.attrNames cfg.channels);
    staticComplianceErrors = staticFailuresForChannels enforceChannels;

    # Issue #88: ambiguity error when a host carries 2+ tags whose pins
    # both resolve. Eager: lives in checkInvariants so the harness's
    # tryEval (which doesn't force lazy mapAttrs thunks) catches it.
    pinTagConflictErrors =
      lib.concatMap (
        hostName: let
          host = cfg.hosts.${hostName};
          pinnedTagNames =
            lib.filter (
              t: (cfg.tags ? ${t}) && (cfg.tags.${t}.pin or null) != null
            )
            host.tags;
        in
          # Only conflicts when host doesn't have its own pin (host wins).
          lib.optional
          (host.pin == null && builtins.length pinnedTagNames > 1)
          "host '${hostName}' is in multiple tags with pins (${
            lib.concatStringsSep ", " pinnedTagNames
          }) - disambiguate by lifting one of these pins to a host-level declaration on '${hostName}', or removing the pin from one of the tags"
      )
      (lib.attrNames cfg.hosts);

    errs = hostChannelErrors ++ channelPolicyErrors ++ edgeErrors ++ channelEdgeShapeErrors ++ channelEdgeErrors ++ configurationErrors ++ complianceErrors ++ cycleErrors ++ channelCycleErrors ++ freshnessErrors ++ staticComplianceErrors ++ pinTagConflictErrors;
  in
    if errs == []
    then true
    else throw ("nixfleet invariant violations:\n  - " + lib.concatStringsSep "\n  - " errs);

  resolveFleet = cfg:
    assert checkInvariants cfg; let
      emptySelectorWarnings =
        lib.concatMap (
          policyName:
            lib.concatMap (
              w: let
                hosts = resolveSelector w.selector cfg.hosts;
              in
                lib.optional (hosts == [])
                "rollout policy '${policyName}' has a wave with a selector that resolves to zero hosts"
            )
            cfg.rolloutPolicies.${policyName}.waves
        )
        (lib.attrNames cfg.rolloutPolicies);

      budgetWarnings =
        lib.concatMap (
          b: let
            hosts = resolveSelector b.selector cfg.hosts;
            effectiveMax =
              if b.maxInFlight != null
              then b.maxInFlight
              else if b.maxInFlightPct != null
              then lib.max 1 ((builtins.length hosts * b.maxInFlightPct) / 100)
              else builtins.length hosts;
          in
            lib.optional (builtins.length hosts >= 10 && effectiveMax == 1)
            "disruption budget with maxInFlight=1 on ${toString (builtins.length hosts)} hosts will take long to complete"
        )
        cfg.disruptionBudgets;

      compliancePermissiveWarnings = let
        permissiveChannels =
          lib.filter (n: cfg.channels.${n}.compliance.mode == "permissive") (lib.attrNames cfg.channels);
        hostsOnChannels =
          lib.filter (n: builtins.elem cfg.hosts.${n}.channel permissiveChannels) (lib.attrNames cfg.hosts);
      in
        lib.concatMap (
          hostName: let
            host = cfg.hosts.${hostName};
            probes = host.configuration.config.compliance.evidence.probes or {};
            probeNames = lib.attrNames probes;
            staticOrBoth =
              lib.filter (
                p: let
                  t = probes.${p}.type or "runtime";
                in
                  t == "static" || t == "both"
              )
              probeNames;
            failures =
              lib.filter (
                p: let
                  ev = probes.${p}.staticEvidence or null;
                in
                  ev != null && (ev.passed or true) == false
              )
              staticOrBoth;
          in
            map (p: "[compliance:permissive] host '${hostName}' (channel '${host.channel}'): static control '${p}' failed - ${lib.generators.toPretty {} (probes.${p}.staticEvidence.evidence or {})}") failures
        )
        hostsOnChannels;

      # Issue #88: most-specific-wins pin resolution (host > tag > channel).
      # Multi-tag-conflict is rejected eagerly in `checkInvariants`, so by
      # the time we get here at most one tag pin can apply. `expiresAt`
      # filtering happens later in `nixfleet-release` (chrono-based RFC3339
      # comparison); pure Nix has no robust date parsing.
      resolvePin = hostName: let
        host = cfg.hosts.${hostName};
        pinnedTagNames =
          lib.filter (
            t: (cfg.tags ? ${t}) && (cfg.tags.${t}.pin or null) != null
          )
          host.tags;
        tagPin =
          if pinnedTagNames == []
          then null
          else cfg.tags.${builtins.head pinnedTagNames}.pin;
        channelPin = cfg.channels.${host.channel}.pin or null;
      in
        if host.pin != null
        then host.pin
        else if tagPin != null
        then tagPin
        else channelPin;

      allWarnings =
        emptySelectorWarnings
        ++ budgetWarnings
        ++ compliancePermissiveWarnings;

      # LOADBEARING: builtins.seq below forces the warning side-effect before return.
      emittedWarnings =
        lib.foldl' (acc: msg: lib.warn msg acc) null allWarnings;

      resolved = {
        schemaVersion = 1;
        meta = {
          schemaVersion = 1;
          signedAt = null;
          ciCommit = null;
          # signatureAlgorithm intentionally absent here - pre-stamp eval
          # has no signature, so claiming an algorithm would be a lie.
          # docs/design/contracts.md §V Pattern A: absent ≡ "ed25519" within
          # schemaVersion 1. `stamp_meta` populates the real algorithm at
          # CI signing time.
        };
        hosts =
          lib.mapAttrs (
            n: h: let
              pin = resolvePin n;
            in
              {
                inherit (h) system tags channel pubkey;
                closureHash = null; # Filled by CI from h.configuration.config.system.build.toplevel.
              }
              // lib.optionalAttrs (pin != null) {inherit pin;}
          )
          cfg.hosts;
        channels =
          lib.mapAttrs (_: c: {
            inherit (c) rolloutPolicy reconcileIntervalMinutes signingIntervalMinutes freshnessWindow compliance;
          })
          cfg.channels;
        rolloutPolicies = cfg.rolloutPolicies;
        waves =
          lib.mapAttrs (
            _: c:
              map (w: {
                hosts = resolveSelector w.selector cfg.hosts;
                soakMinutes = w.soakMinutes;
              })
              cfg.rolloutPolicies.${c.rolloutPolicy}.waves
          )
          cfg.channels;
        edges = cfg.edges;
        # Emit canonical {gates, gated, reason} regardless of whether the
        # operator wrote `before/after` or `gates/gated` in fleet.nix.
        # Old fleet.resolved.json bytes (legacy `before/after`) still
        # verify on upgraded CPs via the proto's serde alias.
        channelEdges = map normalizeChannelEdge cfg.channelEdges;
        # Selector preserved at wire level (was: expanded to hosts at eval).
        # Reconciler resolves dynamically so adding/removing a tagged host
        # doesn't require re-signing fleet.resolved. Pre-feat-channel-edges
        # consumers that read `hosts:[]` will see this field absent and must
        # be upgraded - the reconciler in this PR handles either shape.
        disruptionBudgets =
          map (b: {
            selector = b.selector;
            maxInFlight = b.maxInFlight;
            maxInFlightPct = b.maxInFlightPct;
          })
          cfg.disruptionBudgets;
      };
    in
      builtins.seq emittedWarnings resolved;

  # GOTCHA: signatureAlgorithm omitted defaults to ed25519 for backward compat.
  withSignature = {
    signedAt,
    ciCommit,
    signatureAlgorithm ? null,
  }: resolved:
    resolved
    // {
      meta =
        resolved.meta
        // {inherit signedAt ciCommit;}
        // lib.optionalAttrs (signatureAlgorithm != null) {inherit signatureAlgorithm;};
    };
  mkFleet = input: let
    evaluated = lib.evalModules {
      modules = [
        {
          options = {
            hosts = mkOption {
              type = types.attrsOf hostType;
              default = {};
            };
            tags = mkOption {
              type = types.attrsOf tagType;
              default = {};
            };
            channels = mkOption {
              type = types.attrsOf channelType;
              default = {};
            };
            rolloutPolicies = mkOption {
              type = types.attrsOf policyType;
              default = {};
            };
            edges = mkOption {
              type = types.listOf hostEdgeType;
              default = [];
              description = ''
                Per-host DAG ordering within a rollout. `gated` host's
                dispatch is held until `gates` host reaches
                Soaked/Converged within the same rollout. Both hosts
                must be on the same channel - cross-channel ordering
                is `channelEdges`'s job.
              '';
            };
            channelEdges = mkOption {
              type = types.listOf channelEdgeType;
              default = [];
              description = ''
                Cross-channel rollout ordering. A new rollout on channel
                `after` is held until the most-recent rollout on channel
                `before` reaches Converged. Validated at eval time:
                channels must exist and edges must form a DAG.

                Within-channel coordination uses `edges` (host-level);
                this is RFC-0002 §4.3's cross-channel primitive.
              '';
            };
            disruptionBudgets = mkOption {
              type = types.listOf budgetType;
              default = [];
            };
            complianceFrameworks = mkOption {
              type = types.listOf types.str;
              default = ["anssi-bp028" "nis2" "dora" "iso27001"];
              description = ''
                Known compliance frameworks accepted by channel.compliance.frameworks.
                Override only if using an out-of-tree compliance extension.
              '';
            };
            revocations = mkOption {
              type = types.listOf revocationType;
              default = [];
              description = ''
                Operator-declared agent-cert revocations. The release
                pipeline signs these alongside `fleet.resolved` so the
                CP can rebuild `cert_revocations` from empty state
                without a security regression. Empty list is the
                steady state - it still gets signed so a CP rebuild
                has a verifiable source.
              '';
            };
            bootstrapNonces = mkOption {
              type = types.listOf bootstrapNonceType;
              default = [];
              description = ''
                Operator-declared allowlist of valid bootstrap-token
                nonces. Closes the replay-after-DB-wipe vector
                (nixfleet#96): CP refuses /v1/enroll whose nonce is
                not in this signed list. Empty list = no enrolments
                accepted.

                Operator workflow:
                  1. `nixfleet mint-token --hostname X ...` (prints
                     a Nix snippet)
                  2. Paste snippet here, commit, push
                  3. CI signs the sidecar `bootstrap-nonces.json`
                  4. CP polls + applies (within 60s)
                  5. Deploy token to host, agent enrolls

                Entries can be left in this list as an audit log;
                `nixfleet-release` filters out entries with
                `expiresAt` in the past at sign time.
              '';
            };
          };
        }
        input
      ];
    };
  in
    evaluated.config
    // {
      resolved = resolveFleet evaluated.config;
      revocations = evaluated.config.revocations;
      bootstrapNonces = evaluated.config.bootstrapNonces;
    };

  # LOADBEARING: hosts/tags/channels strict-merge (collision throws); rolloutPolicies later-wins; edges/channelEdges/disruptionBudgets concat; complianceFrameworks union.
  mergeFleets = fleetInputs: let
    mergeStrict = kind: a: b:
      lib.foldl' (
        acc: name:
          if acc ? ${name}
          then throw "mergeFleets: ${kind} '${name}' is defined in multiple inputs"
          else acc // {${name} = b.${name};}
      )
      a (lib.attrNames b);
    step = acc: input: {
      hosts = mergeStrict "host" acc.hosts (input.hosts or {});
      tags = mergeStrict "tag" acc.tags (input.tags or {});
      channels = mergeStrict "channel" acc.channels (input.channels or {});
      rolloutPolicies = acc.rolloutPolicies // (input.rolloutPolicies or {});
      edges = acc.edges ++ (input.edges or []);
      channelEdges = acc.channelEdges ++ (input.channelEdges or []);
      disruptionBudgets = acc.disruptionBudgets ++ (input.disruptionBudgets or []);
    };
    empty = {
      hosts = {};
      tags = {};
      channels = {};
      rolloutPolicies = {};
      edges = [];
      channelEdges = [];
      disruptionBudgets = [];
    };
    merged = lib.foldl' step empty fleetInputs;
    specifiedFrameworks = lib.concatMap (i: i.complianceFrameworks or []) fleetInputs;
  in
    mkFleet (
      merged
      // lib.optionalAttrs (specifiedFrameworks != []) {
        complianceFrameworks = lib.unique specifiedFrameworks;
      }
    );
in {
  inherit mkFleet mergeFleets withSignature;
}
