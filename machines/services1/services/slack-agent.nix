# The hackspace Slack bot, as two units on this box.
#
# Replaces `vulcan-bot.service`, which was an unpinned container on `apps1`
# (off-flake, pulled `:latest` on every start, four regex plugins). The design and
# the measurements behind every choice below are in
# `docs/slack-agent-2026-11-16.md`; this file is the wiring, and the comments here
# are the ones that only a *deployed* unit can answer.
#
# ## Two units, one job each
#
# * `hackspace-mcp.service` — the tools. One static binary, one MCP server on
#   `127.0.0.1:8191`, holding the Home Assistant token, the only thing in the pair
#   that knows the building exists. Read-only: every handler issues `GET` and
#   nothing else, which is the answer to "the agent's input is Slack text".
# * `slack-agent.service` — the harness. Slack Socket Mode, the tool-calling loop,
#   and the answer. Dials `https://strata.int.leighhack.org/v1` and its fallbacks,
#   where the front door injects the engine key, so **this unit holds no LLM
#   credential at all** (`common/nginx/front-door.nix`'s `strataBearer`, and rule 31).
#
# Split because the tool set is reusable and the agent loop is not: a second agent
# (Grafana's, the switches') is `slack-agent` pointed at a different `--mcp`, with
# this file as the template. It is also the security split — the untrusted-text side
# holds no credential that matters.
#
# ## Nothing here listens beyond loopback
#
# Socket Mode is an **outbound** WebSocket, so the service needs no vhost, no
# `common/front-door.nix` row, no `common/int-dns.nix` row and no firewall rule —
# and adding one of those four is a change to think about, not a step to take.
# `hackspace-mcp` refuses a non-loopback bind unless told to in a flag nobody passes
# here, because it is unauthenticated by design.
#
# ## Secrets: PID 1 opens the file, the service never does
#
# `slack_agent_env` is a runtime sops secret (`0440`, group `secrets`) handed to both
# units as `EnvironmentFile`. systemd reads it as root and injects the variables, so
# a `DynamicUser` process has the *values* in its environment and never the right to
# re-read the file — the same shape `switch_gui_env_file` uses, and why neither unit
# has a path to a secret in its `argv` (which is world-readable on Linux). Keys:
#
#   SLACK_BOT_TOKEN=xoxb-…      the bot token (the existing app's, unchanged)
#   SLACK_APP_TOKEN=xapp-…      the app-level token, connections:write, Socket Mode
#   HACKSPACE_MCP_HA_TOKEN=…    Home Assistant long-lived access token
#
# Reusing Vulcan's app rather than minting a new one was the instruction; the cost
# is that the app still carries the ~25 scopes its manifest requested (`app_manifest.yml`
# upstream) for a bot that now needs six, and that is recorded as a follow-up in
# `docs/follow-ups.md` rather than in a comment that will rot.
{
  pkgs,
  lib,
  config,
  CRANE,
  INFRA,
  ...
}: let
  cfg = config.infra.slackAgent;
  HOSTS = import ../../../common/hosts.nix;
  LlmEndpoints = import ../../../common/llm-endpoints.nix;
  ENV_FILE = "/run/secrets/slack_agent_env";

  # The endpoints to try, in order, as one line of text.  Rendered from
  # `common/llm-endpoints.nix`, so this unit cannot drift from what pi is told
  # about the same boxes, and no model id is written here or anywhere.
  endpointsFile = pkgs.writeText "slack-agent-endpoints" LlmEndpoints.toAgentSpec;

  hackspaceMcp = CRANE.crate {
    pname = "hackspace-mcp";
    version = "0.1.0";
    member = "hackspace-mcp";
    sharedCrates = [
      "json"
      "http"
      "mcp"
      "web"
      "service"
    ];
  };

  # Wait for the tool server's socket before an agent starts asking it for tools.
  #
  # `after = [ "hackspace-mcp.service" ]` cannot promise this: that unit is
  # `Type=simple`, so systemd calls it started the instant it forks, and it is still
  # dialling its five sources when an agent asks `tools/list` — and the agent's own rule
  # is that **no tools means no answer**, so it exits 1.  Measured on the first deploy of
  # `listen` (2026-11-17): exactly that, then up with 7 tools five seconds later, which is
  # the right behaviour for a long-lived service and the wrong one for an activation —
  # a unit that fails during activation fails the *switch* (status 4), which disarms
  # auto-rollback until a human confirms a deploy that had in fact worked, and a false
  # alarm is what trains people to stop reading the reason.
  #
  # So: wait up to 20 s, then start anyway and let the never-give-up policy have it.
  # `exit 0` at the end is deliberate — a tool server that is genuinely down must not be
  # turned into a unit that can never start.
  #
  # `/bin/sh`, **not** `/bin/bash`: this system has no `/bin/bash` at all (NixOS ships
  # `/bin/sh` -> bash, and that is the whole of `/bin`), which is the kind of thing you
  # only find out from `systemd-analyze verify`, where the answer is `Command /bin/bash is
  # not executable`.  Bash in POSIX mode still does `/dev/tcp` — the redirection is
  # compiled in, not a mode-dependent builtin — so the check works and needs no netcat in
  # the closure.  Verified against the live socket before deploying: rc 0 with
  # `hackspace-mcp` up, and it gives up after 20 s rather than blocking a start.
  mcpWait = "/bin/sh -c 'for i in $(seq 20); do timeout 1 /bin/sh -c \"echo > /dev/tcp/127.0.0.1/8191\" 2>/dev/null && exit 0; sleep 1; done; exit 0'";

  slackAgent = CRANE.crate {
    pname = "slack-agent";
    version = "0.1.0";
    member = "slack-agent";
    # `http` is not a crate `slack-agent` itself names — it is the one `common-mcp`'s
    # client calls (`common_http::post_json`), and crane stubs every `common-rs/` crate
    # a member does not list here.  So this list is *transitive*, and the only check on
    # it is the Nix build: `cargo build` in the workspace always sees the real code,
    # which is why `just test` was green for a day before `just check-build` said
    # "cannot find function post_json in crate common_http" (2026-11-16).
    # `web` is here because `listen` serves HTTP through `common-web` (see
    # src/listen.rs); `http` is not a crate `slack-agent` itself names — it is the one
    # `common-mcp`'s client calls (`common_http::post_json`), and crane stubs every
    # `common-rs/` crate a member does not list here.  So this list is *transitive*,
    # and the only check on it is the Nix build.
    sharedCrates = [
      "json"
      "http"
      "web"
      "mcp"
      "service"
    ];
  };

  # Which addresses the tools may read.  Plain HTTP, one flag each, so the unit states its
  # whole reach in eleven lines: five sources, **four on this box** and one on the router.
  #
  # Nothing here dials `apps1` any more, which is worth stating because it did until 2026-11-16:
  # Home Assistant and the Space API were both on that VM and both moved here (`machines/
  # services1/services/home-assistant.nix`, `./hackspace-api.nix`).  `common/hosts.nix` still
  # deliberately omits `apps1` — it is an off-flake Proxmox VM whose address this flake only
  # points at, pinned by MAC in `common/dns-records.nix` (`docs/monster-proxmox.md` owns the
  # box) — but with the last in-tree caller gone, an `apps1` outage now costs the Slack bot
  # nothing.  The only thing left dialling it is its own Prometheus, and the front door's
  # `jenkins`/`access-api`/`user-tweaker` rows.
  mcpFlags = [
    "--listen"
    "127.0.0.1:8191"
    "--space-url"
    "http://127.0.0.1:8081"
    "--hass-url"
    "http://127.0.0.1:8123"
    "--frigate-url"
    "http://127.0.0.1:5000"
    "--prometheus-url"
    "http://127.0.0.1:9091"
    "--leases-url"
    "http://${HOSTS.router}:8091"
    "--timeout-secs"
    "5"
  ];

  agentFlags = [
    "serve"
    "--mcp"
    "http://127.0.0.1:8191/mcp"
    "--endpoints-file"
    endpointsFile
    "--listen"
    "127.0.0.1:8192"
    "--timeout-secs"
    "120"
    "--max-silence-secs"
    "900"
    # Where the bot may answer without being @-mentioned.  One channel, a sandbox:
    # everything else in the workspace is mention-only, and DMs are answered.
    # Answer wherever the request is made, unmentioned — which is the contract the
    # old bot had, and what was asked for on 2026-11-16 ("always answer on the
    # channel the request is made from, just like the old implementation").
    #
    # `*` is not "answer everything": an unmentioned message must also pass
    # `triage::answers`, which is the part Vulcan got from a keyword table and an LLM
    # does not get for free — a question or a hackspace topic yes, "does anyone have a
    # spare cable" and "thanks!" no.  Its header owns the reasoning, and the suite
    # pins both directions, because a first version of that list asserted the opposite
    # about bare weekday names and would have interrupted `#general` over "see you
    # monday".
    #
    # The old bot's bare commands (`temps`, `status`, `events`, `printers`, `humidity`,
    # `members`) are now answered here, not deferred: `apps1:vulcan-bot.service` was
    # stopped, disabled, file-removed and masked on 2026-11-16.  If it is ever connected
    # again, add "--defer-vulcan-words" to the flags below — Slack duplicates every event
    # to both Socket Mode consumers of one app token (measured, both journals), so those
    # six words would otherwise be answered twice in the same channel.
    "--channels"
    "*"
    # Whether the engine thinks out loud first.  Measured default; see the option.
    "--thinking"
    cfg.thinking
  ];
in {
  options.infra.slackAgent = {
    thinking = lib.mkOption {
      type = lib.types.enum [
        "on"
        "low"
        "off"
      ];
      default = "on";
      description = ''
        Whether the inference engine may emit its private reasoning before answering.

        Default "on" because that is what the measurement said, not what intuition did:
        suppressing it made the whole 14-question fixture 33 % slower — 359.6 s against
        270.8 s, 44 rounds against 41, 8972 completion tokens against 6694 — because a
        model with no scratchpad answers by calling more tools instead.  The table, the
        single spelling of the kwarg the engine actually honours, and the two production
        turns that died because reasoning ate the entire token budget, are in
        `docs/slack-agent-2026-11-16.md` §15.  Set "low" (reasoning_effort) if a voice
        answer ever starts needing a third of a minute again; "off" is the option that
        already lost once.
      '';
    };
    dryRun = lib.mkOption {
      type = lib.types.bool;
      # True by default, and that is the whole point of the option: the first deploy
      # of a bot that can address a workspace should not be the deploy that addresses
      # it. Flip it with `infra.slackAgent.dryRun = false;` in
      # machines/services1/configuration.nix once somebody has watched the journal and
      # agreed these answers are fit to send in public.
      default = true;
      description = "Read Slack and run the model, but post nothing.";
    };

    ask = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = ''
          Run `slack-agent listen`: the bot's answer on a loopback socket, for anything
          that is not Slack.  On by default because `voice-assistant` is the second
          caller and a disabled socket there means a silent assistant, not an error.
        '';
      };
      listen = lib.mkOption {
        type = lib.types.str;
        default = "127.0.0.1:8194";
        description = ''
          The bind, and the **only** place it is written: `machines/services1/services/
          voice-assistant.nix` builds its `--agent-url` from this option rather than
          repeating the port, which is how the two units cannot disagree about where the
          bot answers.  Loopback only — the unit refuses any other bind, because the
          socket has no authentication and its tools hold the building's read tokens.
        '';
      };
      resetAfterSecs = lib.mkOption {
        type = lib.types.int;
        default = 900;
        description = "Silence after which an ask conversation starts again (see src/listen.rs).";
      };
      recordQuestions = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = ''
          Keep the question and answer text in this socket's `/status.json`, the same
          choice `voice-assistant` makes and for the same reason: `whisper-ws` has
          published its last 20 transcriptions since the day it was written, and a
          loopback socket is no more exposed than that.  `false` keeps the counts and
          the timings, so "it is slow" is still answerable.
        '';
      };
    };

    digest = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Post the weekly calendar digest on a timer.";
      };
      channel = lib.mkOption {
        type = lib.types.str;
        default = "general";
        description = "Channel for the weekly digest, by name (ids pass through).";
      };
      days = lib.mkOption {
        type = lib.types.int;
        default = 7;
        description = "How far ahead the digest lists.";
      };
      dryRun = lib.mkOption {
        type = lib.types.nullOr lib.types.bool;
        default = null;
        description = ''
          Post nothing and print instead.  Null follows `infra.slackAgent.dryRun`, which is
          what you want nine times out of ten: a digest is the most public thing this service
          does, so it goes quiet at the same moment the bot does.
        '';
      };
      onCalendar = lib.mkOption {
        type = lib.types.str;
        default = "Mon 09:00";
        description = "systemd calendar spec for the digest.";
      };
    };
  };

  config = {
    # The weekly digest — `vulcan-bot`'s `@schedule(day_of_week=0, hour=9)` job, ported
    # as a *deterministic* one: `slack-agent digest` asks `space_events` directly, so no
    # model ever has to remember to sort a list or leave a title alone.  It is a oneshot
    # on a timer, not a long-lived service.
    #
    # `Restart = on-failure` and NOT `always`: `mkNeverGiveUp` would re-post an already
    # posted digest forever, which is the one thing a scheduled job must not do.  The
    # burst is finite for the same reason — a calendar that stays down should leave a red
    # unit, not an infinite retry loop firing every minute for a week.
    systemd.services.slack-agent-digest = {
      description = "Hackspace weekly event digest (Slack, deterministic)";
      after = ["network-online.target" "hackspace-mcp.service"];
      wants = ["hackspace-mcp.service"];
      # AGENTS: these two live on the unit, not in `serviceConfig` — and here the
      # placement is not just style, it is what bounds the retry of a job that posts to a
      # public channel.
      startLimitIntervalSec = 3600;
      startLimitBurst = 5;
      serviceConfig = {
        Type = "oneshot";
        DynamicUser = true;
        EnvironmentFile = ENV_FILE;
        ExecStart =
          "${slackAgent}/bin/slack-agent digest"
          + " --to ${cfg.digest.channel} --days ${toString cfg.digest.days}"
          # `null` means "follow the bot", and it has to be tested for by name:
          # `cfg.digest.dryRun or cfg.dryRun` is still `null`, because `or` guards a missing
          # attribute and not a null value (measured: `optionalString` then refused the null
          # and the whole system failed to build).
          + lib.optionalString
          (
            if cfg.digest.dryRun == null
            then cfg.dryRun
            else cfg.digest.dryRun
          )
          " --dry-run";
        # Same posture as the two long-lived units: nothing to write, nothing to leak.
        ProtectSystem = "strict";
        ProtectHome = true;
        NoNewPrivileges = true;
        PrivateTmp = true;
        LimitCORE = 0;
      };
    };

    systemd.timers.slack-agent-digest = lib.mkIf cfg.digest.enable {
      description = "Weekly hackspace calendar digest";
      wantedBy = ["timers.target"];
      timerConfig = {
        # Persistent: a services1 that was down for maintenance on Monday morning still
        # posts the digest when it comes back, rather than losing the week.  The old bot
        # had no such notion — a missed schedule was simply missed.
        OnCalendar = cfg.digest.onCalendar;
        Persistent = true;
        AccuracySec = "1min";
      };
    };

    # The tools first: `slack-agent` refuses to start with no tools at all, which is
    # deliberate (an agent that cannot see anything must not answer from memory), and
    # `wants` + `after` rather than `requires` so restarting the tool server does not
    # drag the Slack session down with it — the agent exits, systemd dials it again,
    # and the Socket Mode session is cheap to re-open.
    systemd.services.hackspace-mcp = INFRA.mkNeverGiveUp {
      description = "Hackspace MCP tool server (loopback, read-only)";
      wantedBy = ["multi-user.target"];
      after = ["network-online.target"];
      wants = ["network-online.target"];
      serviceConfig = {
        ExecStart = "${hackspaceMcp}/bin/hackspace-mcp " + lib.concatStringsSep " " mcpFlags;
        DynamicUser = true;
        EnvironmentFile = ENV_FILE;
        # Nothing here writes, so nothing here needs a writable filesystem.
        ProtectSystem = "strict";
        ProtectHome = true;
        NoNewPrivileges = true;
        PrivateTmp = true;
        # The token is in the environment; a crash must not drop it into a core file
        # that outlives the process.
        LimitCORE = 0;
      };
    };

    systemd.services.slack-agent = INFRA.mkNeverGiveUp {
      description = "Hackspace Slack agent (Socket Mode, tools over MCP)";
      wantedBy = ["multi-user.target"];
      after = [
        "network-online.target"
        "hackspace-mcp.service"
      ];
      wants = [
        "network-online.target"
        "hackspace-mcp.service"
      ];
      serviceConfig = {
        ExecStartPre = mcpWait;
        ExecStart =
          "${slackAgent}/bin/slack-agent "
          + lib.concatStringsSep " " agentFlags
          # First boot reads and does not speak. See `infra.slackAgent.dryRun` above:
          # this is the one flag whose absence is a decision about the workspace, so it
          # is an option rather than a comment that the next deployer deletes.
          + lib.optionalString cfg.dryRun " --dry-run";
        DynamicUser = true;
        EnvironmentFile = ENV_FILE;
        ProtectSystem = "strict";
        ProtectHome = true;
        NoNewPrivileges = true;
        PrivateTmp = true;
        LimitCORE = 0;
      };
    };

    # The bot's second face: the same loop, the same tools, a socket instead of Slack.
    # A separate unit rather than a flag on `slack-agent`, because the Slack session and
    # a room waiting for an answer have different things to be bored by — Socket Mode
    # exits when Slack goes quiet for five minutes, and the ask socket must not.
    systemd.services.slack-agent-listen = INFRA.mkNeverGiveUp {
      description = "Hackspace agent answer socket (loopback, for voice and anything else)";
      wantedBy = ["multi-user.target"];
      after = [
        "network-online.target"
        "hackspace-mcp.service"
      ];
      wants = [
        "network-online.target"
        "hackspace-mcp.service"
      ];
      serviceConfig = {
        ExecStartPre = mcpWait;
        ExecStart =
          ''${slackAgent}/bin/slack-agent listen --ask-listen ${cfg.ask.listen}''
          + " --mcp http://127.0.0.1:8191/mcp --endpoints-file ${endpointsFile}"
          + " --ask-reset-after-secs ${toString cfg.ask.resetAfterSecs}"
          + " --thinking ${cfg.thinking}"
          + lib.optionalString (!cfg.ask.recordQuestions) " --no-record-questions";
        DynamicUser = true;
        # No `EnvironmentFile`: this face holds no token.  The tools it calls hold the
        # Home Assistant token, and they are a different process, which is the whole
        # point of the split in this file's header — the side that talks to a room has
        # nothing in it worth stealing from a room.
        ProtectSystem = "strict";
        ProtectHome = true;
        NoNewPrivileges = true;
        PrivateTmp = true;
        LimitCORE = 0;
      };
    };

    # Both units answer `/health` and `/metrics` on loopback, which is what makes
    # "the bot is quiet" and "the bot is deaf" distinguishable — the liveness gauge is
    # `slack_agent_seconds_since_slack_frame`, and the unit exits on its own past
    # `--max-silence-secs` so the answer to a dead socket is a restart, not a rumour.
    services.prometheus.scrapeConfigs = [
      {
        job_name = "slack-agent";
        static_configs = [
          {
            targets = [
              "127.0.0.1:8191" # hackspace-mcp: tool calls, by outcome
              "127.0.0.1:8192" # slack-agent: turns, latency, socket silence
              "127.0.0.1:8194" # slack-agent listen: asked-through-the-voice, turns, errors
            ];
          }
        ];
      }
    ];
  };
}
