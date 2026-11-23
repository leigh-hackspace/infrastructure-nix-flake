# DATA — plain attrset read by `import` from anywhere in the tree; it is not a module and defines no options.
#
# The inference endpoints this network can put a chat agent in front of: which one
# exists, what it is called, where its OpenAI-dialect root is, and in what order to
# try them. `common/pi-endpoints.nix` imports this file for `name` and `baseUrl`
# rather than repeating them (AGENTS.md rule 2: one fact, one place), and keeps its
# own `compat`/`thinkingLevelMap` blocks because those are pi-specific and belong
# with pi. `machines/services1/services/slack-agent.nix` renders `agentOrder` into
# the file `slack-agent --endpoints-file` is handed.
#
# ## What must never appear in this file
#
# **A model id.** Every row is a URL; the id, the context window and the modalities
# are what `GET {baseUrl}/models` says at run time. A hand-written copy of those
# facts existed here once under pi's name, every class of it had already drifted, and
# it is deleted over it — `docs/pi-agent-models.md` and rule 31. `slack-agent probe`
# is the command that prints what it actually discovered on each box.
#
# **A credential.** Every `baseUrl` here is a front-door `*.int` name whose row
# carries `strataBearer = true`, so nginx injects the engine key on the LAN and no
# client holds one (`common/nginx/front-door.nix`, `common/front-door.nix`). That has
# been true of every row since 2026-11-21, when services3's llama.cpp (the last row
# gated only by the LAN ACL) was replaced by a Strata engine and took the header with
# it; the LAN ACL still gates every `*.int` name as well.
#
# ## `agentOrder`, and why the order is a decision
#
# First answer wins, so this is the quality ordering as much as the resilience one.
# Measured 2026-11-16 with `just slack-agent-probe` and `just slack-agent-ask` and
# re-measured 2026-11-21 when services3 changed engine: services2 answers the
# tool-calling benchmark correctly; aibox is a bigger general model with twice the
# context and, since 2026-11-23, the box whose RAM is no longer shared with the fleet's
# builds (machines/aibox/STRATA.md is the record of what that sharing cost the engine), and
# services3 now serves **the same Coder pack as services2** on an 11 GB
# Pascal card — same answers, ~18 tok/s decode against services2's ~45, and a first
# token that can take minutes, so it is the third arm and not the first. `zen3` is
# excluded from the agent because it is off-network and unreachable from several of
# these boxes by design.
let
  # The failover order for chat.
  agentOrder = ["strata-services2" "strata" "strata-services3"];

  endpoints = {
    "strata-services2" = {
      name = "Strata (services2, RTX 3060)";
      shortName = "services2 (RTX 3060)";
      baseUrl = "https://strata.int.leighhack.org/v1";
      note = ''
        The CUDA engine, and the one the user asked to put the bot in front of.
        `machines/services2/strata.nix`; door row `strata.int.leighhack.org`
        (`strataBearer`, so the LAN gets the key injected).'';
    };

    "strata" = {
      name = "Strata (aibox)";
      shortName = "aibox (iGPU)";
      baseUrl = "https://strata2.int.leighhack.org/v1";
      note = ''
        aibox's Radeon iGPU engine, `machines/aibox/strata.nix`, door row
        `strata2.int.leighhack.org`. Bigger general model, 2× the context —
        the fallback that changes the register of the answer, not just its
        availability.'';
    };

    "strata-services3" = {
      name = "Strata (services3, GTX 1080 Ti — Pascal, slowest)";
      shortName = "services3 (GTX 1080 Ti)";
      baseUrl = "https://strata3.int.leighhack.org/v1";
      note = ''
        `machines/services3/strata.nix`, door row `strata3.int.leighhack.org`
        (`strataBearer`, like the other two). The same Coder IQ1_M pack as
        `strata-services2` on a Pascal card, in place of that box's llama.cpp since
        2026-11-21 — so it is the same answers, slower, and the row that was
        `llama-services3` (a Vulkan llama-server that no longer runs, at a name that
        now 502s) is retired rather than repointed.'';
    };

    "strata-zen3" = {
      name = "Strata (zen3, R9700 — off-network, faster, bigger context)";
      shortName = "zen3 (off-network)";
      baseUrl = "https://llm.ai.chrisdell.info/v1";
      note = ''
        Not this flake's box; reachable over the WAN only, which several of these
        machines have no route to (the WAN backup gives no inbound, and services3's
        uplink is a USB dongle). Declared so pi can still be pointed at it from a
        box that can, and excluded from `agentOrder` so a hackspace bot never waits
        on a link that is not there.'';
      agent = false;
    };
  };
in {
  inherit agentOrder endpoints;

  # The `--endpoints` spec `slack-agent` reads, in `agentOrder`, as
  # `url|shortName,url|shortName`.  Built here so the order and the labels cannot
  # disagree with the table, and so the deployed config is one store path someone
  # can `cat`.
  toAgentSpec = builtins.concatStringsSep "," (
    map (id: "${endpoints.${id}.baseUrl}|${endpoints.${id}.shortName}")
    (builtins.filter (id: endpoints.${id}.agent or true) agentOrder)
  );
}
