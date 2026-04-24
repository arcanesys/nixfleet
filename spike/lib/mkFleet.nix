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

    errs = hostChannelErrors ++ channelPolicyErrors ++ edgeErrors;
  in
    if errs == []
    then true
    else throw ("nixfleet invariant violations:\n  - " + lib.concatStringsSep "\n  - " errs);

  # --- Resolved projection (RFC-0001 §4.1) ---
  resolveFleet = cfg:
    assert checkInvariants cfg; {
      schemaVersion = 1;
      hosts =
        lib.mapAttrs (_: h: {
          inherit (h) system tags channel;
          closureHash = null; # CI fills this in from h.configuration.config.system.build.toplevel
        })
        cfg.hosts;
      channels =
        lib.mapAttrs (_: c: {
          inherit (c) rolloutPolicy reconcileIntervalMinutes compliance;
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
in {
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
          };
        }
        input
      ];
    };
  in
    evaluated.config // {resolved = resolveFleet evaluated.config;};
}
