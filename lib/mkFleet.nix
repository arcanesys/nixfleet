# lib/mkFleet.nix
#
# Produces `fleet.resolved` per RFC-0001 §4.1 + docs/CONTRACTS.md §I #1.
# Output is canonicalized to JCS (RFC 8785) by `bin/nixfleet-canonicalize`
# (owned by Stream C) before signing — DO NOT introduce floats, opaque
# derivations, or attrsets whose iteration order is significant here.
{lib}: let
  inherit (lib) mkOption types;

  # --- Selector algebra (RFC-0001 §3) ---
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
    };
  };

  # --- Host ---
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
    };
  };

  tagType = types.submodule {
    options.description = mkOption {
      type = types.str;
      default = "";
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
            strict = mkOption {
              type = types.bool;
              default = true;
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

  edgeType = types.submodule {
    options = {
      before = mkOption {type = types.str;};
      after = mkOption {type = types.str;};
      reason = mkOption {
        type = types.str;
        default = "";
      };
    };
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

  # Tarjan-free cycle detection using iterative DFS marking.
  # Edges: { after = "a"; before = "b"; } means a must finish before b starts.
  # So we walk "after → before" edges.
  hasCycle = edges: let
    adj =
      lib.foldl' (
        acc: e: let
          current = acc.${e.after} or [];
        in
          acc // {${e.after} = current ++ [e.before];}
      ) {}
      edges;
    nodes = lib.unique (map (e: e.after) edges ++ map (e: e.before) edges);
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

  # --- Selector resolution: selector × hosts → [host-name] ---
  resolveSelector = sel: hosts: let
    names = lib.attrNames hosts;
    matches = n: let
      h = hosts.${n};
    in
      sel.all
      || (sel.hosts != [] && builtins.elem n sel.hosts)
      || (sel.channel != null && h.channel == sel.channel)
      || (sel.tags != [] && lib.all (t: builtins.elem t h.tags) sel.tags)
      || (sel.tagsAny != [] && lib.any (t: builtins.elem t h.tags) sel.tagsAny);
  in
    builtins.filter matches names;

  # --- Invariant checks (RFC-0001 §4.2) ---
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
          lib.optional (!builtins.elem e.before hostNames) "edge.before references unknown host '${e.before}'"
          ++ lib.optional (!builtins.elem e.after hostNames) "edge.after references unknown host '${e.after}'"
      )
      cfg.edges;

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

    cycleErrors = lib.optional (hasCycle cfg.edges) "edges form a cycle; the DAG invariant is violated";

    freshnessErrors =
      lib.concatMap (
        channelName: let
          c = cfg.channels.${channelName};
        in
          lib.optional (c.freshnessWindow < 2 * c.signingIntervalMinutes)
          "channel '${channelName}': freshnessWindow (${toString c.freshnessWindow}) must be ≥ 2 × signingIntervalMinutes (${toString c.signingIntervalMinutes})"
      )
      (lib.attrNames cfg.channels);

    errs = hostChannelErrors ++ channelPolicyErrors ++ edgeErrors ++ configurationErrors ++ complianceErrors ++ cycleErrors ++ freshnessErrors;
  in
    if errs == []
    then true
    else throw ("nixfleet invariant violations:\n  - " + lib.concatStringsSep "\n  - " errs);

  # --- Resolved projection (RFC-0001 §4.1) ---
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

      allWarnings = emptySelectorWarnings ++ budgetWarnings;

      # Force the warnings side effect before returning the resolved value.
      # `lib.warn` prints to stderr during eval and returns its second arg.
      emittedWarnings =
        lib.foldl' (acc: msg: lib.warn msg acc) null allWarnings;

      resolved = {
        schemaVersion = 1;
        meta = {
          schemaVersion = 1;
          signedAt = null;
          ciCommit = null;
        };
        hosts =
          lib.mapAttrs (_: h: {
            inherit (h) system tags channel pubkey;
            closureHash = null; # CI fills this in from h.configuration.config.system.build.toplevel
          })
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
        disruptionBudgets =
          map (b: {
            hosts = resolveSelector b.selector cfg.hosts;
            maxInFlight = b.maxInFlight;
            maxInFlightPct = b.maxInFlightPct;
          })
          cfg.disruptionBudgets;
      };
    in
      builtins.seq emittedWarnings resolved;

  # Stamp CI-provided signing metadata onto a resolved fleet value.
  # `signatureAlgorithm` is optional — omit it when signing with ed25519
  # (the default per CONTRACTS §I #1 for backward-compatible consumers).
  # Set it to `"ecdsa-p256"` (or any future value the contract accepts)
  # when Stream A's CI signs with a non-default algorithm, e.g. when the
  # TPM keyslot emits ECDSA P-256.
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
in {
  inherit withSignature;
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
              type = types.listOf edgeType;
              default = [];
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
          };
        }
        input
      ];
    };
  in
    evaluated.config // {resolved = resolveFleet evaluated.config;};
}
