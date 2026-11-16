# DATA — plain attrset read by `import` from anywhere in the tree; it is not a module and defines no options.
#
# Every inference endpoint the fleet's `pi` installs may dial, and what `pi-agent-sync`
# is allowed to assume about each one. Rendered by `common/pi-agent.nix` into a
# `pkgs.writeText` store path, which the unit hands straight to `--config` (there is no
# `/etc/pi-agent`), read by `crates/pi-agent-sync`, which turns it into
# `~/.pi/agent/models.json` on every box. `docs/pi-agent-models.md` owns the mechanism;
# this file is the table.
#
# ## What belongs here, and what must never be written here
#
# **Not here:** a model id, a context window, or an output limit. Those are properties
# of a *running engine* and this file's whole purpose is to stop asserting them — every
# hand-written copy that preceded it had already gone stale (aibox reports `n_ctx`
# 262144 against the 131072 its own `pi-models.json` claimed; zen3 reports 393216
# against 262144; and zen3's model has since changed quant, so its old id answers
# nothing at all). Measured 2026-11-14, receipts in `docs/pi-agent-models.md`.
#
# **Never here:** a credential. `apiKey` is a placeholder whose only job is to make pi
# list the model, because nothing that authenticates to these endpoints is held by a
# client: the two Strata engines are behind `strataBearer` on `common/front-door.nix`,
# which injects `strata_api_key` at the door, and services3's llama.cpp is gated by the
# LAN ACL. This is not tidiness — the pre-flake `models.json` dialled `10.3.1.21:8080`
# and `10.3.1.32:8080` directly and so **carried the real `strata_api_key`**, in
# `~/.pi/agent/models.json`, on four boxes (found 2026-11-14; rotation is the open item
# in `docs/follow-ups.md`). Every row here is therefore a front-door name or an
# out-of-band host, and the file it renders is 0444 on purpose.
#
# **Here, and only here:** what no probe can tell you — which API dialect to speak,
# which compat claims are *verified* rather than advertised, what the thinking levels
# map onto, and how much of a context window to hand an output limit.
#
# Which endpoints exist, what they are called and where they are is *not* stated
# here: it comes from ./llm-endpoints.nix, so the fleet's two consumers of the
# inference boxes (pi, and the Slack agent's harness) cannot disagree about an
# address. What is stated here is only what no probe can tell you about a running
# engine and only pi cares about.
let
  LlmEndpoints = import ./llm-endpoints.nix;
  endpoint = id: LlmEndpoints.endpoints.${id};
in {
  # The provider id is the key, and it is load-bearing in two places: pi's
  # `settings.json` `defaultProvider` names it, and `pi-agent-sync` replaces a provider
  # *by id*, so renaming one is a user-visible change and not a cosmetic one.
  "strata" = {
    name = (endpoint "strata").name;
    baseUrl = (endpoint "strata").baseUrl;
    note = ''
      aibox's Radeon iGPU, the full 125B IQ2_XS: `machines/aibox/strata.nix`, and the
      door row `strata2.int.leighhack.org` in `common/front-door.nix` (`strataBearer =
      true`, which is what makes `apiKey` a placeholder). Two names because there are
      two engines; `strata` is this one and `strata` in `common/int-dns.nix`'s terms is
      services2's.'';
    compat = {
      supportsStore = false;
      supportsDeveloperRole = false;
      supportsReasoningEffort = false;
      supportsUsageInStreaming = true;
      supportsStrictMode = false;
      maxTokensField = "max_tokens";
      thinkingFormat = "qwen-chat-template";
    };
    thinkingLevelMap = {
      off = "off";
      minimal = null;
      low = "low";
      medium = "medium";
      high = "high";
      xhigh = null;
      max = null;
    };
  };

  "strata-services2" = {
    name = (endpoint "strata-services2").name;
    baseUrl = (endpoint "strata-services2").baseUrl;
    note = ''
      services2's CUDA engine, the Coder IQ1_M: `machines/services2/strata.nix` and its
      door row. This endpoint is offline more often than the others — the box's RAM is
      the engine's and it is stopped to build things — which is the case
      `pi-agent-sync`'s keep-the-previous-model-list rule exists for.'';
    compat = {
      supportsStore = false;
      supportsDeveloperRole = false;
      supportsReasoningEffort = false;
      supportsUsageInStreaming = true;
      supportsStrictMode = false;
      maxTokensField = "max_tokens";
      thinkingFormat = "qwen-chat-template";
    };
    thinkingLevelMap = {
      off = "off";
      minimal = null;
      low = "low";
      medium = "medium";
      high = "high";
      xhigh = null;
      max = null;
    };
  };

  "llama-services3" = {
    name = (endpoint "llama-services3").name;
    baseUrl = (endpoint "llama-services3").baseUrl;
    note = ''
      services3's Vulkan llama-server (Ternary-Bonsai-2-9B-Q2_0 + vision,
      `machines/services3/llama.nix`). Its door row deliberately carries **no**
      `strataBearer` — that field injects the *Strata* key, and pointing an unrelated
      engine at the same credential to save adding a field is how secrets acquire an
      audience (`common/front-door.nix`) — so `apiKey` here is a placeholder and the
      gate is the LAN ACL.

      No `compat` block, on purpose: pi auto-detects those from the URL, and the
      guidance is not to enable them because an endpoint advertises OpenAI
      compatibility. Nobody has measured this one's tool-calling or usage-in-streaming
      behaviour from pi. `just pi-agent-status` proves it is at least reachable and
      what it reports as its context.'';
  };

  "strata-zen3" = {
    name = (endpoint "strata-zen3").name;
    baseUrl = (endpoint "strata-zen3").baseUrl;
    note = ''
      Not this flake's box: a Strata engine on `zen3-nixos`, reachable over the WAN.
      It is in this table because the point is that one declaration covers every
      machine, and because it has been the fallback that makes a big-context session
      possible at all (measured in `machines/aibox/STRATA.md`: ~51 tok/s decode,
      ~1184 tok/s prefill against aibox's). Expect it to be unreachable from a box that
      has no route (the WAN backup gives no inbound and services3's uplink is a USB
      dongle), which is a `keeping the previous model list` line in the status, not an
      error. '';
    compat = {
      supportsStore = false;
      supportsDeveloperRole = false;
      supportsReasoningEffort = false;
      supportsUsageInStreaming = true;
      supportsStrictMode = false;
      maxTokensField = "max_tokens";
      thinkingFormat = "qwen-chat-template";
    };
    thinkingLevelMap = {
      off = "off";
      minimal = null;
      low = "low";
      medium = "medium";
      high = "high";
      xhigh = null;
      max = null;
    };
  };
}
