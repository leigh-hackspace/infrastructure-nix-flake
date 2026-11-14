# MODULE — NixOS module; in a machine's module list or a directory's imports, never read as data.
#
# `programs.pi-agent`: declare the network's inference endpoints once, in
# `common/pi-endpoints.nix`, and let every box write its own `~/.pi/agent/models.json`
# from them — with the model list and the context windows **discovered from the running
# engines** rather than asserted here.
#
# The program that does it is `crates/pi-agent-sync`; `docs/pi-agent-models.md` owns the
# mechanism, the measurements and the failure modes. What this file owns is the shape:
#
#   common/pi-endpoints.nix ──(pkgs.writeText)──▶  <store>/pi-agent-endpoints.json  (0444, no secrets)
#          │                       │ a path that IS the file - writeTextFile's path is a DIRECTORY,
#          │                       │ and handing that to --config is the activation failure recorded
#          │                       │ in docs/pi-agent-models.md
#          │                       ▼
#          └── options.programs.pi-agent.endpoints   --config (the store path; nothing is copied to /etc)
#                                            pi-agent-<user>.service  ──▶  $HOME/.pi/agent/models.json
#                                            + pi-agent-<user>.timer        $HOME/.pi/agent/pi-agent-status.json
#
# ## Why a timer and not a build-time file
#
# The two numbers that matter most about an endpoint — which models it currently serves
# and how large a context it serves them with — are facts about a *process that may be
# stopped right now*. A derivation cannot ask it (and `nix` must not go to the network
# at build time, let alone at evaluation: AGENTS.md rule 22 and the whole of
# `docs/nix-dev-workflow.md` "Evaluation must not build"). So the endpoint list is
# declarative and the model list is discovered at run time, which is also what makes a
# box with no route to an endpoint (services3 behind a USB dongle) degrade to a status
# line instead of a broken config.
#
# ## Why a system unit with `User=` and not a user unit
#
# A `systemd.user.*` unit needs a lingering user manager, which needs someone to have
# ever logged in as that user — and `just deploy` reaches these boxes over ssh with a
# key, not a login session, so on a freshly installed box the timer would simply never
# exist. `loginctl enable-linger` is one command, but it is a command, and the fault
# (config silently absent on the newest box) is the one this module exists to remove.
# A root-started oneshot with `User=`/`Group=` and an absolute `HOME` writes the same
# file, is manageable with plain `systemctl start` from the deploy path, and needs no
# session. Note what that trades away: it is the *flake* deciding to write inside a
# person's dotfiles, which is why `users` is an option and why the merge rule in
# `crates/pi-agent-sync`'s `render.rs` (only the providers named by the generated config
# are touched; everything else in the file is copied through) is the guarantee that
# makes it acceptable.
{
  config,
  lib,
  pkgs,
  CRANE,
  ...
}: let
  cfg = config.programs.pi-agent;

  # Every field the reader (`crates/pi-agent-sync/src/config.rs`) understands. Absent
  # means "the program's default", which is why most rows of
  # `common/pi-endpoints.nix` are five lines.
  endpointOptions = {
    name,
    config,
    ...
  }: {
    options = {
      name = lib.mkOption {
        type = lib.types.str;
        default = name;
        description = "Provider display name; also suffixed onto every discovered model.";
      };
      baseUrl = lib.mkOption {
        type = lib.types.str;
        example = "https://strata2.int.leighhack.org/v1";
        description = ''
          The OpenAI-compatible base URL, **as a client dials it** — usually a front-door
          name rather than the engine's address, because that is where the credential is
          injected. Use an address-free name: `common/hosts.nix` exists so the two ends
          of a proxy cannot disagree, but this row is read by a laptop too.
        '';
      };
      api = lib.mkOption {
        type = lib.types.str;
        default = "openai-completions";
        description = "Which API dialect pi speaks to this endpoint.";
      };
      apiKey = lib.mkOption {
        type = lib.types.str;
        default = "pi-agent-sync";
        description = ''
          A **placeholder**, written into a world-readable file and then into
          `models.json`. Its only job is to be non-empty, so pi lists the model.

          Putting a real key here puts it in the Nix store, on every box that
          substitutes this system, forever, by name — see `common/sops.nix` and
          `config.sopsSecretText` for what that costs. If an endpoint genuinely needs a
          client-side secret, it belongs in `auth.json` via `pi /login` or in an
          interpolated `"$ENV_VAR"`, and this option's description should say which.
        '';
      };
      probeRoot = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "https://strata2.int.leighhack.org";
        description = ''
          Where `/props` lives. Defaults to the origin of `baseUrl`, which is right for
          every engine here; set it when the OpenAI surface is mounted under a prefix
          the origin does not share.
        '';
      };
      reasoning = lib.mkOption {
        type = lib.types.enum ["auto" "true" "false"];
        default = "auto";
        description = ''
          Whether the models here think. `auto` asks the engine — its chat template
          renders a `<think>` block — which is measured on both engines here rather than
          assumed; `true`/`false` is the escape hatch for one that cannot be asked.
        '';
      };
      input = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = ["text"];
        description = "Declared input modalities, used only when neither the engine nor the previous run says.";
      };
      contextWindow = lib.mkOption {
        type = lib.types.nullOr lib.types.int;
        default = null;
        description = ''
          Declared context window, used only when the engine reports none and the
          previous file has none — i.e. a last resort for a server that will not say.
          Setting it on an engine that *does* say is how this module got written, so
          prefer leaving it null and reading `just pi-agent-status`.
        '';
      };
      maxTokens = lib.mkOption {
        type = lib.types.nullOr lib.types.int;
        default = null;
        description = "Output limit, overriding the divisor rule below entirely.";
      };
      maxTokensDivisor = lib.mkOption {
        type = lib.types.int;
        default = 8;
        description = ''
          `n_ctx` is prompt **and** completion, so the output limit is a slice of it.
          One eighth reproduces both numbers the hand-written file carried
          (131072 → 16384, 262144 → 32768). It is a policy, not a measurement: nothing
          an engine reports says how much of the window you may ask back.
        '';
      };
      maxTokensMin = lib.mkOption {
        type = lib.types.int;
        default = 4096;
        description = "Floor on that slice, so a small-context engine is not left with a useless one.";
      };
      maxTokensMax = lib.mkOption {
        type = lib.types.int;
        default = 32768;
        description = "Ceiling, so a 393216-token window does not promise a completion nobody waits for.";
      };
      compat = lib.mkOption {
        type = with lib.types; nullOr (attrsOf (oneOf [bool str int]));
        default = null;
        apply = v:
          if v == {}
          then null
          else v;
        description = ''
          Provider-level `compat`, emitted verbatim. Only **verified** differences from
          the standard request/response behaviour belong here (pi's own guidance, and the
          reason `llama-services3` has none); a model-level `compat` overrides these per
          field, so one copy covers whatever the endpoint loads next.
        '';
      };
      thinkingLevelMap = lib.mkOption {
        type = with lib.types; nullOr (attrsOf (nullOr str));
        default = null;
        description = ''
          pi thinking level → the value this engine's template expects. A model field in
          pi's schema (unlike `compat`), so it is written onto every model.
        '';
      };
      headers = lib.mkOption {
        type = with lib.types; nullOr (attrsOf str);
        default = null;
        description = ''
          Extra request headers. Same interpolation rules as `apiKey` (`"$NAME"`), so a
          header may be a secret without this file containing one.
        '';
      };
      cost = lib.mkOption {
        type = with lib.types; nullOr (attrsOf raw);
        default = null;
        apply = v:
          if v == {}
          then null
          else v;
        description = "Self-hosted, so zero by construction; set it if an endpoint ever starts charging.";
      };
      note = lib.mkOption {
        type = lib.types.str;
        default = "";
        description = ''
          Prose for whoever ends up on the box. Not sent to pi; it lands in
          `pi-agent-status.json`, so the answer to "why is this endpoint here / why does
          it have no compat block" travels with the endpoint instead of living only in
          the flake.
        '';
      };
    };
  };

  # Rendered for the reader: drop the defaults nobody set, and key each row by its
  # attribute name (`id` is not an option, because an id that can disagree with its own
  # attribute name is one more thing to get wrong).
  endpointRow = id: e:
    lib.filterAttrs (n: v: v != null && v != "" && v != [] && (n != "note" || e.note != "")) {
      inherit id;
      name = e.name;
      baseUrl = e.baseUrl;
      api = e.api;
      apiKey = e.apiKey;
      probeRoot = e.probeRoot;
      reasoning = e.reasoning;
      input = e.input;
      contextWindow = e.contextWindow;
      maxTokens = e.maxTokens;
      maxTokensDivisor = e.maxTokensDivisor;
      maxTokensMin = e.maxTokensMin;
      maxTokensMax = e.maxTokensMax;
      compat = e.compat;
      thinkingLevelMap = e.thinkingLevelMap;
      headers = e.headers;
      cost = e.cost;
      note = e.note;
    };

  # `writeText`, not `writeTextFile`: the latter's store path is a **directory** with the
  # file inside it, and handing that path to `--config` was this module's first deploy
  # failure — `read /nix/store/…-pi-agent-endpoints.json: Is a directory (os error 21)`, the
  # tool exits 1, and because a freshly started unit failing *is* an activation failure,
  # `nixos-rebuild switch` exited 4 on a box whose configuration had otherwise applied
  # completely (measured 2026-11-14, services1). `writeText` yields a path that *is* the file.
  endpointsJson = pkgs.writeText "pi-agent-endpoints.json" (builtins.toJSON {
    # `endpoints` is a **list**, not an attrset: the order is the order the tool probes
    # and prints in, and a list keeps the journal stable across boxes instead of
    # alphabetical-by-luck. `builtins.attrValues` on an attrset is already sorted by
    # name, which is what the tool re-sorts models by for the same reason.
    endpoints = map (id: endpointRow id cfg.endpoints.${id}) (lib.attrNames cfg.endpoints);
  });

  unitsFor = user: let
    u = config.users.users.${user};
    agentDir = "${u.home}/${cfg.agentDir}";
    args =
      [
        "--config"
        "${endpointsJson}"
        "--out"
        "${agentDir}/models.json"
        "--status"
        "${agentDir}/${cfg.statusFile}"
        "--settings"
        "${agentDir}/settings.json"
        "--timeout"
        (toString cfg.probeTimeout)
      ]
      ++ lib.optionals cfg.createAgentDir ["--create-missing"];
  in {
    service = {
      description = "Discover pi's model list from the flake's inference endpoints (for ${user})";

      # Not `after wait-for-nas` — this wants no mount — but it does want a route and
      # a resolver, because its whole job is asking remote processes questions. Without
      # this the boot-time run races dnsmasq on the very box that runs it (gw) and the
      # first answer of the day is "every endpoint is unreachable".
      after = ["network-online.target"];
      wants = ["network-online.target"];
      # No `wantedBy`: the timer of the same name is what starts this, and adding a
      # second pull would mean two runs at boot. `OnBootSec` + `Persistent` below are
      # the boot run.

      # Every endpoint, twice the timeout each, plus a margin: a unit that hangs here
      # would otherwise hold no lock but does hold its own timer's slot, and the point
      # of the timer is that a dead engine costs a log line and not the hour.
      serviceConfig = {
        Type = "oneshot";
        User = user;
        Group = u.group;
        # The tool takes absolute paths, so this is for curl's own taste and for
        # anything it shells out to. Measured on services1 with `env -i`: nixpkgs'
        # curl needs no NIX_SSL_CERT_FILE to verify the front door's certificate, so
        # the unit sets nothing else.
        Environment = "HOME=${u.home}";
        TimeoutStartSec = 30 + cfg.probeTimeout * 2 * builtins.length (builtins.attrNames cfg.endpoints);
        # A failed probe is a *result* here (the tool exits 0 and records it), so
        # anything non-zero is the config or the filesystem — and the answer is one loud
        # failed unit, not a retry loop. `Restart` was set until the first deploy, where
        # one bad argument left the unit restarting every minute and sitting in
        # `activating` indefinitely: that keeps `systemctl is-system-running` off
        # `operational` on an otherwise healthy box (the false-incident shape
        # `docs/follow-ups.md` already objects to), and it re-tries a mistake that cannot
        # fix itself. The timer asks again in `refreshInterval`, which is the retry.
        Restart = "no";
        # `getExe'` with the program named rather than `getExe`: the crane-built
        # crates in this repo set no `meta.mainProgram`, and nixpkgs now warns about
        # guessing it — and a warning is a finding (AGENTS.md rule 9).
        ExecStart = "${lib.getExe' cfg.package "pi-agent-sync"} ${lib.escapeShellArgs args}";
      };

      # The one store path the unit needs beyond the binary. `path` puts it on PATH,
      # which is what the tool's default `--curl curl` asks for.
      path = [pkgs.curl];
    };

    timer = {
      description = "Re-discover pi's model list (for ${user})";
      wantedBy = ["timers.target"];
      timerConfig = {
        OnBootSec = "2min";
        OnUnitActiveSec = cfg.refreshInterval;
        # A box that was off when the engines changed catches up on boot rather than
        # serving a list as old as its last boot.
        Persistent = true;
      };
    };
  };
in {
  options.programs.pi-agent = {
    enable = lib.mkEnableOption ''
      keeping `~/.pi/agent/models.json` in sync with the endpoints declared in Nix,
      with the models and context windows discovered from the live engines
    '';

    package = lib.mkOption {
      type = lib.types.package;
      default = CRANE.crate {
        pname = "pi-agent-sync";
        version = "0.1.0";
        member = "pi-agent-sync";
        sharedCrates = ["json" "service"];
      };
      defaultText = lib.literalExpression "CRANE.crate { pname = \"pi-agent-sync\"; ... }";
      description = "The discovery program (`crates/pi-agent-sync`).";
    };

    endpoints = lib.mkOption {
      type = with lib.types; attrsOf (submodule endpointOptions);
      default = import ./pi-endpoints.nix;
      defaultText = lib.literalExpression "import ./common/pi-endpoints.nix";
      description = ''
        The endpoints every box may dial. Override per machine to add one
        (`programs.pi-agent.endpoints."mine" = { baseUrl = …; }`) or to drop one
        (`programs.pi-agent.endpoints."strata-zen3".baseUrl` is not how you drop it —
        that leaves a half-row; the way is to not name it, e.g. an explicit
        `endpoints = lib.attrByPath … `, or `enable = false` on an endpoint-less box).
      '';
    };

    users = lib.mkOption {
      type = with lib.types; listOf str;
      default = ["leigh-admin"];
      description =
        ""
        + "Whose `~/.pi/agent/models.json` is managed. One unit and one timer each."
        # Resolved through `users.users` rather than by assuming `/home/<name>`, so a
        # person whose home this configuration moves moves here too. A name with no
        # `users.users` entry is an eval error naming that attribute — deliberately not
        # a pre-check: `lib.hasAttr name config.users.users` forces the *conditions* of
        # every user definition, and on services1 that chain runs through
        # postgresql/outline and a sops secret, i.e. it turns an eval into a build
        # (AGENTS.md rule 22 — "Evaluation must not build anything"), which is exactly
        # what this module must not do on the router. Measured 2026-11-14: the check
        # died with `path '…-sops-secret-pg_pass.drv' is not valid` from nothing but the
        # hasAttr, with the unit's own `${u.home}` read perfectly legal because it asks
        # about one key.
        + " A user this flake does not declare (an SSS/authentik POSIX account,"
        + " docs/auth-posix-logins.md) has no home to write into: add them to"
        + " `users.users`, or do not name them here.";
    };

    agentDir = lib.mkOption {
      type = lib.types.str;
      default = ".pi/agent";
      description = "pi's agent directory, relative to the user's home (see `PI_CODING_AGENT_DIR`).";
    };

    statusFile = lib.mkOption {
      type = lib.types.str;
      default = "pi-agent-status.json";
      description = ''
        Where the last run records what it probed, what each engine answered and where
        each number came from. Next to `models.json` rather than in `/run`: the point of
        it is that the person sitting at the box can read it, and it is the file
        `just pi-agent-status` prints.
      '';
    };

    refreshInterval = lib.mkOption {
      type = lib.types.str;
      default = "15min";
      example = "hourly";
      description = "How often to ask the engines again (`OnUnitActiveSec`).";
    };

    probeTimeout = lib.mkOption {
      type = lib.types.int;
      default = 20;
      description = "Seconds per HTTP request (`curl -m`). Two requests per endpoint.";
    };

    createAgentDir = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        Create the agent directory if it is not there. Default false, because a
        directory under `~/.pi` on a box where pi was never installed is state this
        module invented: the tool then says `SKIP: … does not exist — pi is not
        installed for this user here` and exits 0. Set true only if the module also
        owns the install.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.users != [];
        message = "programs.pi-agent.enable needs at least one user; with none there is nothing to write and nothing to fail.";
      }
    ];

    environment.systemPackages = [cfg.package];

    systemd.services = lib.listToAttrs (map (user: lib.nameValuePair "pi-agent-${user}" (unitsFor user).service) cfg.users);
    systemd.timers = lib.listToAttrs (map (user: lib.nameValuePair "pi-agent-${user}" (unitsFor user).timer) cfg.users);
  };
}
