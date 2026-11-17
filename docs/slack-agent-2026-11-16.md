# Slack agent: replacing Vulcan with a tool-calling chat bot (design, 2026-11-16)

**Status: a plan, nothing deployed.** No Slack message has been sent while writing this and
none is needed to test any phase — see [Testing without Slack](#testing-without-slack).

Vulcan is Leigh Hackspace's Slack bot, answering `temps`, `status`, `events`, `printers`,
`members`, `humidity` by regex, posting the door access log into `#doors` and a Monday
calendar digest into `#general`. It runs as an unpinned container on `apps1`, the one
off-flake box this network still leans on. The goal is a bot that answers
*"is it cold in the hackspace"* and *"is the hackspace open"* in free text, with the tools
an agent needs published from our own systems, and the whole thing declared here.

This document is the survey, the decisions, and the build order. It records what was
**measured on 2026-11-16** as opposed to what is assumed, and it is deliberately blunt about
the four places this can fail quietly.

---

## 1. What we are replacing, measured

`ssh -i ~/.ssh/agent-hop-key root@10.3.1.30` → `systemctl cat vulcan-bot.service`:

- Image `ghcr.io/leigh-hackspace/vulcan-bot:latest`, pulled on **every** start
  (`ExecStartPre=docker pull`). Unpinned, so the box can move underneath a reboot with no
  commit and no receipt — the same fault class as `apps1`'s `:stable` Home Assistant pin
  (`docs/homeassistant-frigate-via-device-2026-11-02.md`).
- `CMD=[python3 -m vulcan_bot]`, `ENTRYPOINT=[]`, `-v /srv/vulcanbot:/data`,
  `--env-file=/etc/docker/environment_files/vulcanbot` (three keys: `SM_SLACK_BOT_TOKEN`,
  `SM_SLACK_APP_TOKEN`, `SM_SQLITE_FILE`).
- `/srv/vulcanbot/vulcanbot.db` is 12 KiB — the `slack-machine` framework's own storage,
  holding nothing the hackspace cares about. **There is no state to migrate.**
- Slack app: `api_app_id` `A05ARUTUESH`, team `T0A9Y2E83`, **Socket Mode** (outbound wss,
  so the bot has always had *no inbound listener* — that property must survive, it is why a
  bot lives happily behind `gw`).

Source: `github.com/leigh-hackspace/vulcan-bot` — **265 lines of Python**, four plugins:

| Plugin | What it does | Source it reads |
| --- | --- | --- |
| `HackspaceSpaceAPIPlugin` | `status`, `temp`/`temps`/`temperature`, `humidity`, `members`, `printers` | `GET <api>/space.json`, plucked by key |
| `HackspaceEventsPlugin` | `events` (7-day window) + `@schedule(day_of_week=0, hour=9)` digest into `#general` | `GET <api>/events?calendar=public\|members` |
| `HackspaceAccessLogPlugin` | subscribes MQTT `access/log` forever | `mqtt.int.leighhack.org:1883` |
| `HackspaceAccessLogPosterPlugin` | `"{device_id} - {user} - {result}"` into `#doors` | the plugin above |

**The most important fact in this section is the one that is not a command.** The live
journal shows the app receiving `Event - person detected on camera main_space` messages
uploaded by a **bot** (`bot_id` `B05HF8YAHUN`) in a channel Vulcan watches. So:

> Slack delivers this app's events including **its own and other bots'** messages.

Any replacement that answers "all messages in channels it can see" will answer its own
output, and the output of Frigate's notifier, and then answer that. **Filtering on
`bot_id`/`subtype` is not a nicety, it is the loop-prevention mechanism**, and the fact only
appears in traffic.

### What is worth keeping, and it is not the chat

Two of those four plugins are **not conversational and must never depend on an LLM**:
the `#doors` access log and the Monday digest. If the inference box is down — and
`common/pi-endpoints.nix` already records that services2 "is offline more often than the
others" because its RAM *is* the engine — the door log must keep posting. They are ported as
deterministic code in phase 3, before the LLM is anywhere near them.

---

## 2. The requirement, as testable sentences

1. `@bot is it cold in the hackspace` → an answer that reads a real number, names the room,
   and is right about what "cold" means here.
2. `@bot is the hackspace open` → the same truth as `state.open` in `space.json`, now.
3. `@bot who came in last night` → the access log. `@bot what's the workshop doing` →
   printers + Frigate + temperature.
4. Unknown question → says it does not know, rather than inventing a temperature. This is a
   real hazard at IQ1_M quantisation with a coder model, not a hypothetical.
5. `temps` still works (it is muscle memory), and costs one tool call.
6. Inference down → door log and digest keep running; chat says the engine is down.
7. Nothing the bot can *do* can write to a switch, a door, or Home Assistant (phase 1–4).

---

## 3. What we would put behind tools, all measured from services1 on 2026-11-16

| Source | Probe | Result | Notes for the tool design |
| --- | --- | --- | --- |
| Space API `api.int.leighhack.org/space.json` | `curl` | **200** | The whole building in one call: `state.open/lastchange`, 7 `temperature`, 7 `humidity`, `total_member_count`, `ext_3d_printers` (2), `membership_plans`. **This one endpoint is most of Vulcan.** |
| `…/events?calendar=public` | `curl` | **200** | Real calendar (CTF on the 16th, `rrule` weekly/4wk) |
| Home Assistant `10.3.1.30:8123/api/states` | `curl` | **401** | Routing fine, needs a long-lived access token. Everything *else* in the building lives here (lights, locks, presence, power, per-area history) |
| Frigate `frigate.int…/api/stats`, `/api/events` | `curl` | **200** | Keyless from the LAN; events carry `camera`/`label`/`zones`/`start_time` |
| Prometheus `prometheus.int…/api/v1/query` | `curl` | **200** | 771 metric names, **none of them a room temperature** — moonraker printers, `node_hwmon_temp`, `smartctl`, UPS. Fleet telemetry, not building telemetry. Do not let the model "find the temperature" here. |
| MQTT broker (services1 podman, `podman-mqtt.service`) | broker journal | **live** | `New client connected from 10.3.1.30…` — Vulcan's dial works since `mqtt.int` was moved to `answeredBy via = "services1"` (2026-11-03, `docs/mqtt-int-legacy-answer-2026-11-02.md`). Anonymous LAN broker by design. |
| `gw` `10.3.1.1:8091/api/leases` | `curl` | **200** | `{"total":30,"expired":0,…}` — "how many people are here" |
| printers `10.3.14.62:7125` | `curl` | **rc=7** | Off-flake VLAN, not reachable from services1 right now. Printers come from `space.json` + Prometheus instead. |
| Slack `api.slack.com`, `wss-primary.slack.com` | `curl` | 302 / **200** | Outbound is fine from services1; Socket Mode needs no firewall change |
| **services2 engine** `strata.int.leighhack.org/v1/models` | `curl` | **200, no key** | `qwen3.8-flash-next-coder-iq1m`, `n_ctx: 131072` |
| same, `/v1/chat/completions` with one tool | `curl` | **`finish_reason: tool_calls`** | Correct `tool_calls` array. **The door injects `strata_api_key`, so a LAN client holds no credential** (`common/nginx/front-door.nix` `strataBearer`). Measured: prefill **243 tok/s**, decode **31.9 tok/s**, `reasoning_content` returned as a **separate field** |
| aibox `strata2.int…/v1/models` | `curl` | 200 | `qwen3.8-flash-next-iq2xs`, `n_ctx: 262144` — the *non-coder* general model |
| services3 `llama.int…/v1/models` | `curl` | 200 | `Ternary-Bonsai-2-9B-Q2_0`, `capabilities: [completion, multimodal]` |

Two things fall out of this table and change the design:

- **The LLM costs are asymmetric and decode is the wall.** ~32 tok/s means a 200-token answer
  is 6 s, and every tool round **re-prefills** the whole transcript. Budget ~20–30 s of wall
  clock for a two-tool answer, and design the Slack UX around it (react immediately, post
  once) rather than around a fast model.
- **services2's engine is a *coder* model at a 1-bit quant.** It demonstrably calls tools. It
  has not been measured answering *"is it cold in the hackspace?"* in a chatty register. The
  offline eval in phase 2 is the gate on that, and the fallback ordering is a *quality*
  decision as much as a resilience one.

---

## 4. Libraries and tools that already exist (researched 2026-11-16)

Checked **against this flake's pinned nixpkgs** (`2074cc672349`, and the system registry's
`flake:nixpkgs` is the same rev, so `nix eval nixpkgs#…` answers the question asked):

| Candidate | At our pin? | Verdict |
| --- | --- | --- |
| `python3Packages.mcp` | **2.3.0** | The official Python MCP SDK — server *and* client, Streamable HTTP included |
| `python3Packages.fastmcp-slim` | **3.4.7** | FastMCP 3, the high-level way to write an MCP server/client in Python |
| `python3Packages.openai-agents` | **0.22.0** | **The OpenAI Agents SDK is already packaged here.** Any OpenAI-compatible `base_url`, MCP clients built in |
| `python3Packages.slack-bolt` | **1.30.0** | The official Slack framework — Socket Mode, event routing, `app_mention` middleware |
| `python3Packages.aiomqtt` | 2.5.1 | What Vulcan already uses for `access/log` |
| `open-webui` | 0.13.3 | Rejected below |
| `litellm` | 1.108.0 | A gateway, not an agent; no chat loop to reuse |
| `n8n` | 2.39.6 | Wrong shape: workflow runner, not a tool-calling agent |
| `onyx` | 2.0.12 | A RAG product with a Slack connector; "what's the temp" is not a retrieval task |
| `goose` | 3.30.0 | **Not the AI agent.** `nix eval nixpkgs#goose.meta.homepage` → `pressly.github.io/goose`, i.e. Pressly's **SQL migration tool**. `block/goose` is not in nixpkgs |
| `pydantic-ai`, `llm`, `langgraph`, `smolagents`, `letta` | absent | Would be vendored |

Rust side (crates.io, reached by crane's `fetchcargo` like every other dep here):

- **`rmcp`** — the official Rust MCP SDK, `client` + `server` features, Streamable HTTP
  (JSON *or* SSE responses) and pluggable transports. Both halves of what we need.
- **`slack-morphism`** 2.x — async Slack client, **Socket Mode since 0.10.x**, Web API and
  Block Kit types. Saves hand-rolling the wss handshake + ack protocol.

Upstream facts worth knowing before choosing a ready-made harness:

- **Home Assistant ships an MCP *server* integration** (`mcp_server`, at `/api/mcp` and
  per-tool-set paths like `/api/mcp/assist`, introduced 2025.2). But: measured **404 on our
  HA**, i.e. not configured; enabling it is a UI config flow in an off-flake box; it
  authenticates with **OAuth 2.0 + PKCE and a browser login** (with the docs' own warning
  that proxy/hostname mismatches break `issuer` discovery); the main URL wants an **admin**
  account; and it exposes write-capable device control. Wrong contract for a headless bot.
- **OpenAI Agents SDK's** MCP support covers stdio/Streamable HTTP (SSE deprecated) — the
  Python path is genuinely well-served.
- **Slack has an MCP server of its own**, and the OSS one was moved to the *archived*
  MCP-servers repo (no security fixes). Irrelevant either way: **Slack is our transport, not
  a tool.** We do not want an agent "reading Slack" — we want it reading the building.

### Options rejected, with the reason rather than the conclusion

- **Open WebUI.** Slack integration + MCP tools exist, and it is packaged. But it drags a
  multi-user web app, its own DB, and its tools are PEP-723 pip-embedded scripts stored in
  that DB — the opposite of declared config — and its own docs say bot/scheduled runs
  execute tools with **nobody there to click Allow**. For one bot in one workspace that is a
  worse security posture than 400 lines we own.
- **A coding-agent harness as the brain (Goose, or pi itself).** Both are tempting because
  both speak MCP and pi is already installed on all five boxes with the engines already
  declared in `common/pi-endpoints.nix`. Both are still wrong: their *value* is file and
  shell tools, which is precisely what untrusted Slack text must never reach; you would ship
  the binary and then spend the safety effort switching it off. `block/goose` additionally
  has two reported OpenAI-compat defects that are exactly our situation — tools sent without
  `tool_choice` (#12590, fixed in #12716) and `role:"tool"` messages sent without
  `tool_call_id` (#13344) — and note the org name is as the search index returned it, not
  re-verified against a canonical repo.
- **HA's built-in MCP** as the primary tool source — reasons above; revisit only if HA-side
  tooling grows and someone wants HA to be the thing that is asked.

### The one real choice

Everything above narrows to **write the loop, own the tools** — a small Rust service and a
small Rust MCP server, both from our own crate tree — or take the packaged Python stack.

| | **A. Rust** (`slack-agent` + `hackspace-mcp`, `rmcp` + `slack-morphism`) | **B. Python** (`slack-bolt` + `openai-agents` + `mcp`, all from nixpkgs) |
| --- | --- | --- |
| Rule 1 ("Rust for anything new") | follows | breaks it |
| Vendoring | crane/Cargo.lock, same as the 8 crates today | none — every dep is already in the pinned package set |
| Lines to write | ~1,200–1,800 (agent loop, socket glue, ~8 tools, session store) | ~400–600 |
| Deployable artefact | one static binary, no interpreter at runtime | `buildPythonApplication` + interpreter closure |
| Sits with the existing house style | yes — `crates/status-dashboard` is the template, `common-rs/{json,service,web}` already exist to avoid the drift rule 2 was written for | a second language in `crates/` and a new build shape in `common/crane.nix` |
| Where it hurts | the MCP client and Slack ack protocol are ours to get right | MCP-in-Python is smoother; the cost is a fleet that now has two toolchains and one more thing nobody updates |

**Recommendation: A (Rust), with the honest caveat that B is about a third of the code and
needs nothing vendored.** Rule 1 is not a purity rule — it exists because shared code had
already drifted between three callers, and a second bot in a
second language is exactly how that starts. If we take B, the argument that earns it is "the
agent-loop library is the risky part and we would rather not own it", said out loud and
written into this doc, not by drifting.

---

## 5. Architecture

```
Slack (leighhack) ──outbound wss, Socket Mode, no inbound port──┐
                                                                │
services1 ──┬─ slack-agent.service        (new crate, DynamicUser)
            │    ├─ agent loop ── POST https://strata.int.leighhack.org/v1/chat/completions
            │    │      (fallback strata2.int → llama.int)      ↑ door injects strata_api_key
            │    ├─ MCP client ── http://127.0.0.1:8191/mcp ─┐  │  no credential in the unit
            │    └─ deterministic jobs: MQTT access/log → #doors, Monday digest  (no LLM)
            │                                                │
            └─ hackspace-mcp.service     (new crate, loopback)┘
                 space · events · home-assistant · frigate · prometheus · access · leases
                 (each group behind a read-only code path; HA token from /run/secrets)
```

Three components, deliberately:

1. **`slack-agent`** — the harness. Slack Socket Mode in, tool-calling loop, session store,
   reply out. Knows *nothing* about the hackspace: its tool list comes from `tools/list` at
   startup. Owns the two deterministic jobs, which share its Slack client but not its model.
2. **`hackspace-mcp`** — the tools. One binary, one Streamable-HTTP MCP server bound to
   loopback, one tool group per source, **read-only by construction**. It is the only thing
   that holds an HA token, and it is the thing to reuse next: `network-info`, `switch-gui`,
   Grafana, anything a future agent wants.
3. **The wiring** — `machines/services1/services/slack-agent.nix`, the two `CRANE.crate`
   packages, the sops secret, the endpoint table, the verbs.

Why our own MCP server rather than pointing the agent at five HTTP APIs directly: the model
has ~131k of context and 32 tok/s, and a raw `/api/states` is ~200 entities of JSON. **The
tool's job is to make the answer small** — `ha_states(domain="sensor", search="temp")`
returning seven rows is what makes "is it cold" cheap enough to be worth asking. That
narrowing is worth a component even if the transport were irrelevant; MCP makes it reusable
instead of a private struct in one binary.

---

## 6. The agent loop contract

The details that decide whether this feels smart. All measured, none assumed.

- **Endpoint, never a model id.** `GET /v1/models` answers the model id today (services2:
  `qwen3.8-flash-next-coder-iq1m`, aibox `qwen3.8-flash-next-iq2xs`, services3
  `Ternary-Bonsai-2-9B-Q2_0`). The bot asks on startup and on each failover. A hand-written
  model id is rule 31's exact failure — `machines/aibox/pi-models.json` is deleted over
  precisely this.
- **Drop `reasoning_content`, always.** The engine returns it beside `content` (54 tokens on
  the probe). It is not shown to users and it is **not** echoed back on the next turn:
  replaying it inflates every re-prefill for zero benefit and some engines reject it.
- **Small, flat tool schemas.** `supportsStrictMode = false` on every engine in
  `common/pi-endpoints.nix`, so the schema is advice. ≤ 8 tools, no nested objects,
  descriptions that state units and ranges ("°C, building has been 12–24 °C all year")
  because that is the only channel we have to stop a hallucinated 71 °C.
- **Execute every tool call in the array.** Confirmed the engine returns a `tool_calls`
  array; run them concurrently with a per-call `timeout` and always send a result back —
  a tool call left unanswered wedges OpenAI-dialect loops (Goose's #13344 is that bug).
- **Round and token budget.** Hard cap: 4 tool rounds, 6,000 tool-result tokens/turn,
  last N turns of thread history (Slack thread = session), and a summarised stub past that.
  `n_ctx` comes from the endpoint's own `meta.n_ctx` (131072 / 262144), never a constant.
- **Failover = health + quality.** Try endpoints in declared order, pick the first whose
  `/v1/models` answers; say in the reply footer which engine answered while we are tuning.
  Ordering is a phase-2 output of the eval, not a preference.
- **One slot per engine, queued.** 32 tok/s is one conversation at a time. Two people ask at
  once → the second waits, and the react-first UX makes a wait legible instead of broken.
- **Never answer a bot.** Drop anything with `bot_id`/`authorizations[].is_bot`, or a
  `subtype` we did not ask for. This is the loop guard from §1, and it is the one thing that
  can take the channel down by itself.
- **`app_mention` outside the allow-list, plain text inside it** — and an explicit
  allow-list of channels in Nix so the default is "not everywhere".
- **Say "I don't know".** The system prompt states: every number must come from a tool call
  made in this turn; if no tool answered the question, say what you can see and what you
  cannot. Coder model at IQ1_M will otherwise fill silence.

---

## 7. Security model

- **No inbound listener.** Socket Mode is kept precisely for this: no vhost, no `gw` row, no
  port, nothing for the front-door guards to know about (rules 14/24 do not fire). The MCP
  server binds `127.0.0.1` and only `slack-agent` talks to it.
- **Read-only, and that is a phase gate, not a vibe.** No HA service calls, no switch writes,
  no door actuation in v1. A `slackAgent.writes = false` option that makes *eval* refuse to
  expose a write tool. Slack text is untrusted input from anyone who can type in the
  workspace, so prompt injection has to be answered with "there is nothing to inject *into*"
  rather than with a smarter prompt. Rule 16 (nothing writes NVRAM) and the
  `X-WEBAUTH-USER` lesson in `AGENTS.md` are the same argument: the boundary is capability,
  not wording.
- **Credentials.** `xoxb-` + `xapp-` become **runtime** sops secrets delivered by
  `EnvironmentFile` — never `config.sopsSecretText`, no `infra.buildTimeSecrets` entry, no
  store path, no `argv` (rule 13; the shape `switch_gui_env_file` and
  `docs/pi-web-search.md`'s `"!cat /run/secrets/…"` argument both already set). The HA token
  likewise, scoped to `hackspace-mcp.service`. **The LLM needs no secret at all** —
  `strataBearer` on `common/front-door.nix` injects the key at the door for a LAN client,
  which is exactly why the pre-flake `models.json` holding the real key was a finding.
- **`DynamicUser`**, no `nixbld` (rule 7), `after`/`requires` on nothing exotic; it does not
  need the NAS so it does not claim `wait-for-nas` (rule 12 is a *needs* rule, not a default).
- **Data the bot now can read**: temperatures, open state, member *count*, recent door events
  (who badge-in), lease counts, camera event counts. The door log is the one item that is
  about a person, and `#doors` already receives it today, so the bot adds a *retrieval path*
  to data already posted in-channel, not a new collection. Worth an explicit yes from the
  committee, recorded here rather than decided by whoever ships it.

---

## 8. Nix wiring

Files, in the order a reviewer would read them:

- `common/llm-endpoints.nix` — **new DATA table**: `name`, `baseUrl`, `order`, `note`, one
  row per engine. `common/pi-endpoints.nix` imports and extends it with pi's `compat` /
  `thinkingLevelMap` blocks, so the network has one copy of "which engines exist" (rule 2).
  A model id appearing in this file is a bug.
- `crates/hackspace-mcp/` — new workspace member (`crates/Cargo.toml` `members`), `rmcp`
  server, tools per §5, `/v1/models`-free by design, plus `--list-tools` / `--call` for a
  human.
- `crates/slack-agent/` — new member: `main.rs` (subcommands `serve`/`ask`/`jobs`), `agent.rs`
  (loop), `engine.rs` (endpoint discovery + failover), `slack.rs` (Socket Mode + posting),
  `jobs.rs` (access log, digest), `session.rs`. Rule 26: no file over 1000 lines, and the
  split above is by job, which is the rule's own test.
- `machines/services1/services/slack-agent.nix` — the unit: two `CRANE.crate` packages,
  `INFRA.mkNeverGiveUp` (rule "never give up" — Slack socket death must self-heal, this is a
  long-lived wss and `switch-gui`'s dead-session bug is the cautionary tale),
  `DynamicUser = true`, `EnvironmentFile = <sops secret>`,
  `ExecStart = slack-agent serve --config …` with every path a store path or a `/run/secrets`
  reference. Rule 10 applies the day it is written: **build and read back the rendered unit**
  rather than trusting the flags typed here.
- `machines/services1/sops.nix` — declare `slack_agent_env_file` (keys
  `SLACK_BOT_TOKEN`, `SLACK_APP_TOKEN`, `HA_TOKEN`), `0440`, `group = secrets`, member of the
  two service accounts.
- `machines/services1/configuration.nix` — the import.
- `justfile` — the verbs in §9.
- `docs/services.md` — a section per service; the AGENTS.md row already points at this doc.

**No `common/front-door.nix` row, no `common/int-dns.nix` row** — and that absence is a
decision worth keeping, because the next person will reach for both. If the MCP server ever
wants to be reachable off-box, it takes a door row and an ACL and this section gets rewritten
with rules 14/24 in it.

---

## 9. Testing without Slack

The constraint that shapes the build order: **nothing gets posted to `#general` to see
whether it works.** So every phase is provable from a terminal first, and the Slack leg is
the *last* thing switched on, in a channel of one.

| Verb | Proves | Needs Slack? |
| --- | --- | --- |
| `slack-agent-mcp-tools` | `tools/list` renders, and each schema's description says units | no |
| `slack-agent-mcp-call <tool> [json]` | one tool against the live source, raw JSON | no |
| `slack-agent-ask "is it cold in the hackspace"` | **the whole loop** — endpoint discovery, tool call, second round, answer — printed to stdout, with timings and which engine answered | no |
| `slack-agent-eval` | the fixture: ~12 canonical questions, expected tool per question, and a plausibility band per expected number; run against **all three engines**, hand-scored | no |
| `slack-agent-jobs-digest --dry-run`, `… access-log --once` | the two ported jobs, print instead of post | no |
| `slack-agent-serve --dry-run` | real Socket Mode, real events, **posts nothing**, logs what it would have said | reads, never writes |

`slack-agent-eval` is the phase-2 gate on §3's coder-model question and it is cheap: the same
fixture later becomes the regression suite for an engine bump, which in this fleet is a
`nix flake update strata` moving *both* engines at once
(`machines/services2/STRATA.md`, `machines/aibox/STRATA.md`).

Only after a green `serve --dry-run` in a quiet channel does the posting flip, and the
`--dry-run` flag's existence is what lets the cutover be rehearsed without an incident.
(`--no-auth`/`--dev-user` rule 17 is the precedent for "a test hook is a loopback-gated
flag", and here the hook is "do not write to the one external system".)

---

## 10. Cutover, and the one way it goes loudly wrong

**Two bots, one app token, is a double-answer, not a graceful overlap.** Slack delivers the
same `app_mention` to both socket connections; both reply; every message in `#general` gets
two answers. There is no safe parallel run. Order:

1. `slack-agent-jobs-*` verified on services1 (`#doors` receiving from the *new* code while
   Vulcan still posts chat — different plugins, no conflict, so the jobs **are** the safe
   overlap and should be cut over first).
2. Offline eval green; `serve --dry-run` green.
3. `systemctl stop vulcan-bot && systemctl disable vulcan-bot` on `apps1` (ssh there as
   `root@10.3.1.30` with **`-o IdentitiesOnly=yes -i ~/.ssh/agent-hop-key`**: `apps1` is not in
   `common/hosts.nix`, so rule 32's `programs.ssh.hosts` block deliberately does not cover it,
   and without that flag a plain `ssh -i` can die at `Too many authentication failures` —
   measured 2026-11-16, and it reads as "apps1 is broken", which it is not), then the off-flake
   parking rule from `docs/monster-proxmox.md`: the unit file to
   `/var/lib/systemd/disabled-units/` with a `# DISABLED — 2026-11-1x — replaced by
   slack-agent on services1 — restore with:` header, and the reason in that file, not here.
   Do **not** delete `/srv/vulcanbot` or the container until the new bot has survived a week.
4. Start `slack-agent.service` with the channel allow-list = one test channel, confirm with
   the humans actually in the workspace, then widen.
5. Keep the old container image (`docker image ls`, not `:latest` re-pulled) as the rollback,
   and note in the doc that rollback = re-enable on `apps1`, which needs the tokens still
   present in `apps1`'s env-file — which is an argument for *not* scrubbing them.

Because `apps1` is off-flake, none of step 3 is declarative: `just check` cannot see it, and
nothing will ever tell us if someone re-enables it. That asymmetry — a flake that owns the
new thing and can only *ask* about the old one — is the real reason to finish this migration
rather than run both forever.

---

## 11. Build order, with the thing that must be true to finish each

| Phase | Deliverable | Gate |
| --- | --- | --- |
| 0 | This doc | measured, links exist |
| 1 | `hackspace-mcp` + Nix package + unit | every tool answers against the live source from `--call`, on services1 |
| 2 | `slack-agent` **without Slack**: `ask` + `eval` | "is it cold in the hackspace" and "is the hackspace open" correct on ≥ 11/12, no invented numbers, on all three engines; engine ordering chosen from the measurement |
| 3 | The two deterministic jobs ported | `#doors` fed by the new code, digest fires on a timer, no LLM in either path |
| 4 | Slack Socket Mode + reply, `--dry-run` then live in one channel | a day of real traffic, zero self-answers, P95 turn latency recorded |
| 5 | `apps1` parked | **done 2026-11-16**, but out of order — see §14: the two deterministic jobs of phase 3 were not ported first, so they are dark |
| 6 | Observability: `/metrics` on loopback (turn latency, tool calls, tokens, engine used, session drops), a health timer alerting via the existing `slack_url` | "the bot is quiet" distinguishable from "the bot is fine and nobody asked" — rule 29's lesson, applied to a bot |

## 12. Open questions, before any of this is deployed

1. **The HA long-lived access token** cannot be minted from Nix: it is a browser login to
   `ha.int.leighhack.org` as an admin. Who owns that account, and is a read-only service
   account acceptable to authentik's posix mapping (`docs/auth-posix-logins.md` — uids are
   `2000 + pk`, so a new account is fine but must not be a human-shaped one)?
2. **The two Slack tokens** sit in `apps1:/etc/docker/environment_files/vulcanbot`. They
   must be copied into `secrets/secrets.yaml` by a human who will not see them in a
   transcript here (rule 13 — I have deliberately not read that file's values, only its key
   names). Reuse the existing app, or mint a fresh one and rename the display? Reusing means
   zero Slack-side config work and keeps `#doors`' incoming-webhook history; minting means a
   clean scope list without `files:write`/`pins:write`/`usergroups:read`, which Vulcan does
   not need. Leaning: **new app, minimal scopes** (`app_mentions:read`,
   `channels:history`, `groups:history`, `im:history`, `chat:write`, `reactions:write`),
   because the app manifest in the vulcan repo shows 25 scopes for a bot that reads and
   posts.
3. **Persona and voice** — keep the Vulcan name, and who writes the system prompt? A hackspace
   bot that invents a temperature is worse than one that says nothing.
4. **DM policy** — answer DMs from anyone in the workspace, or `#general`-only?
5. **Writes, ever?** "Turn the heaters on" is the obvious next request and it is the moment
   this doc's §7 has to be re-litigated, not quietly extended.
6. **Which box.** services1 for the reasons in §5. `apps1` would keep the bot off the flake
   again (the thing we are fixing); `aibox` is the build machine and the engine host, and
   putting the agent next to its own engine buys nothing while costing the "one box dies,
   nothing works" merge.

---

## 13. Built — phases 1, 2 and 4(dry-run), measured 2026-11-16 evening

The instruction was "proceed with the prototype: Rust, Nix, existing tokens, local LLM,
no leak risk, use your judgement". What follows is what that produced, and the five things
it cost, because each one is a trap that will catch the next person.

**Running on services1 as of this commit:**

| Unit | Bind | What it is |
| --- | --- | --- |
| `hackspace-mcp.service` | `127.0.0.1:8191`, `POST /mcp`, `GET /health`, `GET /metrics` | seven read-only tools over the space API, HA REST, Frigate, Prometheus, the router's lease table |
| `slack-agent.service` | `127.0.0.1:8192` health; Slack is an **outbound** Socket Mode websocket | the harness: engine discovery, tool loop, Slack. **`--dry-run`**, so it reads and answers into the journal and posts nothing |

Both are `DynamicUser`, `ProtectSystem=strict`, `EnvironmentFile=/run/secrets/slack_agent_env`
(keys `SLACK_BOT_TOKEN`, `SLACK_APP_TOKEN`, `HACKSPACE_MCP_HA_TOKEN` — values never read into
a transcript, and no `--token` flag exists on either binary for the same reason).
No inbound port, no vhost, no DNS row: §5's shape held exactly.

**It is live in the workspace as of 2026-11-16** (`infra.slackAgent.dryRun = false;` in
`machines/services1/configuration.nix`; the option exists and still defaults to `true` so that
any future first deploy of this service is the safe one, and flipping it back is the kill
switch — one line, one switch, no undeploy).

The answer policy is what was asked for: **it answers in the channel the request was made
from, without being mentioned, like Vulcan did.** `--channels "*"`. That is safe only because
unmentioned messages must also pass `triage::answers` (`src/triage.rs`), which is the thing
Vulcan got free from a keyword table and an LLM does not: a question or a hackspace topic is
answered, `does anyone have a spare cable?` / `thanks!` / `see you monday` are not. Its header
owns the reasoning and the suite pins both directions — the first version of that list counted
bare weekday names as topics, and the suite caught that it would have interrupted `#general`
over "see you monday".

**Vulcan's command vocabulary is now ours.** `triage::answers` used to defer a message that was
*nothing but* `temps`/`status`/`events`/`printers`/`humidity`/`members` to the old bot, because
Slack duplicates every event to both consumers of one app token and two replies is the one
visible failure. That deferral is now behind `--defer-vulcan-words`, **off by default**, and the
words themselves moved into the gate's `whole_words` list — matched as whole words, because
`temp` lives inside "attempt" and `state` inside "restate", and a gate that fires on a substring
interrupts a conversation about something else (the suite asserts both). The flag exists for
exactly one reason: if `vulcan-bot` is ever connected again, turn it on or those six words are
answered twice.

### §12, answered by the instruction rather than by waiting

| Question | Answer, and how it was proven |
| --- | --- |
| Rust or Python | **Rust.** It is in the workspace, `CRANE.crate` packages it, and there was no second language to learn on a box this load-bearing. |
| HA token | **No minting needed**: `services1:/run/secrets/env_file` already carried `DE_HOME_ASSISTANT_ACCESS_TOKEN` (the door-entry system's), and it reads. It is copied into `slack_agent_env` as `HACKSPACE_MCP_HA_TOKEN` — same value, separate name, so a later rotation is a decision rather than an accident. **It is not a read-only token** — HA long-lived tokens carry the user's whole authority — so §7's "no write calls in the code" is the actual boundary, and that is worth revisiting when HA lets us mint a scoped one. |
| Slack app | **Reused.** Both tokens came out of `apps1:/etc/docker/environment_files/vulcanbot` into sops without being displayed. Cost of reuse: the app still carries ~25 scopes including `files:write` it never uses; the display name is still Vulcan's. |
| Persona | The prompt in `crates/slack-agent/src/prompt.rs`: short, factual, British-ish, **grounded** — every number, state or time must come from a tool call this turn, and "I can't see that; I can see X" is a correct answer. |
| DMs | **On.** A DM to a hackspace bot asking "is anyone in?" is not a secret. |
| Writes | **No.** There is no HA service-call path in the code at all, and the prototype proved the answer reads well: asked to "turn the heaters on in the workshop", it said it cannot act, and reported what it *can* see (workshop 12.8 °C, and which entities exist). |
| Which box | services1, per §5. |

### Engine facts, measured rather than assumed

`slack-agent probe` against `/v1/models` and a fixed completion, on this box:

| Endpoint | Model the engine itself reported | `n_ctx` | decode |
| --- | --- | --- | --- |
| `strata.int.leighhack.org/v1` (services2, RTX 3060) | `qwen3.8-flash-next-coder-iq1m` | 131072 | ~32 tok/s |
| `strata2.int.leighhack.org/v1` (aibox iGPU) | `qwen3.8-flash-next-iq2xs` | 262144 | ~14–20 tok/s |
| `llama.int.leighhack.org/v1` (services3, 1080 Ti) | `Ternary-Bonsai-2-9B-Q2_0` | 262144 | ~39 tok/s |

No model id is written in any `.nix` (rule 31): `common/llm-endpoints.nix` is *names and
URLs*, and `pi-endpoints.nix` now imports it, so the two consumers cannot disagree about
where an engine lives. `common/llm-endpoints.nix` deliberately declares the off-network
`zen3` row and **excludes it from `agentOrder`** — an agent must not pick an endpoint that
is up to chance.

Two engine behaviours the harness has to respect: the engine emits
`reasoning_content` **as a separate field** (dropped, never shown, never replayed into a
later turn), and an exhausted reasoning budget can return `finish_reason=length` with
empty `content` — the loop retries that once with a doubled answer budget instead of
answering a user with a blank.

### What a real turn costs

```
$ just slack-agent-ask "what's on at the hackspace this week, and is it cold?"
[2 rounds · services2 (RTX 3060) · 17002 ms · 2951 in / 461 out tokens · 2 tool call(s)]
   space_events {"days":7} → 616 chars
   space_status  {}        → 489 chars
```

17 seconds for two tools. §4's warning that this is a *slow* UX was right and the harness
behaves accordingly: `eyes` reaction while working, one reply, no streaming.

### The five things it cost

1. **`CRANE.crate`'s `sharedCrates` list is transitive, and only the Nix build checks it.**
   `slack-agent` never names `common-http` — but `common-mcp`'s client calls
   `common_http::post_json`, and crane replaces every `common-rs/` crate a member does not
   list with a **manifest-only stub**. So `just test` and `cargo build` were green for a day
   and `nix build` said `cannot find function post_json in crate common_http`. The list is
   now commented in `machines/services1/services/slack-agent.nix`. Worth a follow-up: derive
   it from the members' `[dependencies]` path tables instead of maintaining it by hand.
2. **Adding a key to `secrets/secrets.yaml` stops `nix flake check --no-build` from being
   eval-only.** Rule 9 says re-read the baseline rather than inherit it, so I stashed the
   work and ran the check at clean HEAD: it *passed*. With my secret added it failed with
   `path '…-sops-secret-pg_pass.drv' is not valid` — every `config.sopsSecretText` name is
   its own derivation keyed on the *whole file*, so one new key invalidates all of them, and
   `services.outline.databaseUrl` `builtins.readFile`s one, so evaluation now wants to
   *build* it. This is the eval-must-not-build flaw (AGENTS "Evaluation must not build
   anything"), not a defect in the new module. It clears as soon as one build happens — the
   deploy built them and the check passes again — but it means **the first `just check` after
   any secret edit needs a build**, on every box that shares that file.
   The same mechanism is why this deploy restarted outline, grafana and everything else that
   embeds a build-time secret: expected, and worth knowing before you do it during a
   meetup.
3. **A stray dev process is a failed deploy.** `hackspace-mcp.service` could not bind 8191
   because a `hackspace-mcp` I had started by hand *under sudo* (to read the HA token from
   `env_file`) still owned the port. Activation died at status 4 with the new generation
   already current, i.e. **the rollback countdown running**. Two sub-lessons: `pkill -f
   target/debug/hackspace-mcp` exited 143 because the pattern matched the shell running it,
   and `kill <pid>` then said *Operation not permitted* because that process was root's.
   Before any switch that owns a port: `sudo ss -lntp | grep <port>`, and kill by PID with
   the same privileges that started it.
4. **`just` at this pin has no shell-quoting builtin** (`quote`, `shell_quote` both absent
   in 1.59), and a justfile recipe interpolates raw. Prose through two shell levels therefore
   loses its apostrophes — `what's on tonight?` arrives mangled — and JSON arguments lose
   their double quotes (`'{"search":"cam8"}'` arrived as `{search:cam8}`, measured). Fix:
   the question goes to `ask` on **stdin** (`ask -`, fed by a quoted heredoc, indented so
   just does not end the recipe) and JSON goes through an environment variable.
5. **A JSON-RPC envelope built with `format!` is invalid JSON** — `--self-test` failed on
   the very first run because an unquoted tool name is not JSON. Build envelopes with the
   writer, always.

### Where this leaves the plan in §11

Phases **1**, **2** and **4-partial** are done: every tool answers from the live source
through the deployed unit, and `ask` runs the whole loop against all three engines.
Phase **4** needs a human: watch `journalctl -u slack-agent -f`, DM the bot something, and
decide whether the answer in the journal is one you would let stand in public — then flip
`infra.slackAgent.dryRun` and try it in one channel. Phases **3** (the `#doors` MQTT feed
and the Monday digest — deterministic, no LLM, and still Vulcan's today), **5** (park
`vulcan-bot.service` on `apps1`, per §10's order) and **6** (alerting on "quiet vs fine")
are not started. `just slack-agent-eval` and `crates/slack-agent/questions.txt` are the
scoring harness for phase 2's ≥ 11/12 gate, which needs a human to score it.

### The overlap, measured: Slack duplicates events to both consumers

Vulcan and `slack-agent` are **two Socket Mode consumers of the same app token**, both
connected. **Answer (measured 2026-11-16 12:44:09): Slack delivers the event to *both*.**
Someone typed an @mention `temps` in `C0868M1SEJY` (in a thread); Vulcan logged the
`event_callback` and replied, and `slack-agent` logged the same trigger and ran the whole loop:

```
· services2 (RTX 3060): 2357 in / 35 out tokens, 1602 ms, finish=tool_calls
· space_status {} → 490 chars
· services2 (RTX 3060): 2614 in / 150 out tokens, 5938 ms, finish=stop
answered via services2 in 7570 ms (2 rounds, 1 tool call(s))
DRY-RUN would post to C0868M1SEJY thread 1794833049.446659:
```

The workspace saw **one** reply only because `post()` short-circuits on `--dry-run` before it
builds a request — one choke point, `slack.rs:176`. So the moment `dryRun = false` with Vulcan
still running, every `@Vulcan …` gets **two** answers. That closes the "does §10's order matter"
question: stopping `vulcan-bot.service` belongs in the same change as flipping the flag, not in
the one after it.

(Also measured, and pleasant: `temps` — a command Vulcan handles with a hardcoded list — took the
new path 7.6 s end to end and produced room temps, the coldest/warmest judgement, the 2.2 °C
spread, and the fact that the space is closed so nothing is heating it.)

The older hypothesis, kept for the record: Slack documents multiple
connections per app, and 

`slack_agent_seconds_since_slack_frame` is the metric that makes "the bot is quiet"
distinguishable from "the bot is broken" (rule 29's lesson, pre-positioned); what is still
missing is the thing that *reads* it and pages — phase 6.

## 14. Phase 5, done out of order — what it cost (2026-11-16)

`vulcan-bot.service` on `apps1` is **stopped, disabled, file-removed, and masked**:

```
enabled: masked      active: inactive      container: vulcan-bot Exited (0)
```

The unit's own text is parked at `/var/lib/systemd/disabled-units/vulcan-bot.service` with the
off-flake banner this repo's convention asks for — reason, date, what it stops, and `restore
with:` including the `--defer-vulcan-words` note above. Masked *and* removed rather than merely
`disable`d because the ask was "stay disabled on reboot": `disable` is a symlink someone can
re-create, `mask` is a `/dev/null` symlink that makes a start attempt fail loudly. Nothing else
could have resurrected it — checked first, not assumed: the container's own
`HostConfig.RestartPolicy` is `no` (so the docker daemon will not start it at boot whatever
systemd does), no timer or cron names it, and `/etc/systemd/system/vulcan-bot.service` was the
only file in the tree that did. The container and `/srv/vulcanbot` are kept: they are the
rollback for the week, and the doc's own §10 said so.

**What it cost, and what became of it.** The old bot had two jobs that were never conversational
and were not ported at the moment it was stopped:

| Job | Where it lived | State now |
| --- | --- | --- |
| MQTT `access/log` → the `#doors` access feed | `plugins/hackspace.py::access_poster` | **dropped by decision** (2026-11-16: "doors not needed") — nobody posts who came in, and nothing else depended on it |
| Monday event digest → `#general` | `plugins/hackspace.py::scheduled_events_notify` | **ported and improved**: `slack-agent digest` on `slack-agent-digest.timer`, `Mon 09:00`, `Persistent=true` |

### The digest, and the fast path it came out of

The port is **deterministic** — no model in either the timer or the answer — because "what's on"
is the question a hackspace bot gets most and the calendar already holds the answer:

- **`slack-agent digest`** runs on the timer and posts to `--to general` (resolved by name via
  `conversations.list`, refused out loud if no such channel exists). Better than the old one in
  the three ways the old one was wrong: the list is **sorted by start** (Vulcan emitted the API's
  own order, so Monday's social could sit under Thursday), each event carries **the time it
  published** rather than a UTC conversion or an `arrow.humanize()` string like "in 3 hours" that
  is only true at the instant it was written (`clock::wall`, and the suite pins a `+01:00` event
  to `18:30`), and the list is **grouped under the day it happens on** (`Today (Mon 16 Nov)`,
  `Tomorrow (…)`, `Sat 21 Nov`) with the API's `calendar.public_events` reduced to `[public]`.
- **`triage::events_window`** is the same renderer on the live path: a message that is *asking*
  what is on (`events`, `what events are on?`, `whats on tonight`, `is there anything on
  tomorrow`) goes straight to the tool and answers in the time of one HTTP GET instead of ~17 s
  and ~3 000 tokens. It replaced Vulcan's `@respond_to(r"^events")`, which answered anything
  *beginning* with those five letters; the replacement is a word list plus a shape test, and the
  suite pins `"events can be tricky sometimes"` as **not** a question. If the fast path errors at
  runtime it falls through to the model rather than going silent.
- The renderer lives in `hackspace-mcp`, so the digest, the fast path and the model all quote one
  implementation — three callers, one list, no drift.

Preview it without waiting for Monday: `just slack-agent-digest` (posts nothing; prints the exact
text and the resolved channel id).

Doing phase 5 before phase 3 was the wrong order — §10's own list put the jobs first, precisely
so the feed would not skip a beat — and the reason it happened anyway is worth writing down:
the phase table made the cutover look like a chat-bot concern ("`apps1` parked"), when what it
actually gates is two unglamorous scheduled things. **To do, now, in this order:** port the
`#doors` feed (an MQTT subscribe to `access/log` on `mqtt.int`, a fixed format, no LLM, in
`slack-agent` next to the Socket Mode loop), then the digest (a timer + the same Slack post),
then close §11's phase 3 box. Until then, the door feed is an open finding in
`docs/follow-ups.md`, and `docs/monster-proxmox.md` ("What runs inside `apps1`") records the
parking so nobody reads `masked` as a fault.

## 15. Does the model think, and should we let it? (measured 2026-11-17)

**It thinks on every call, on its own, and we were never asking.** `qwen3.8-flash-next-coder-iq1m`
is a Qwen3 and its chat template has a thinking mode that is on unless something turns it
off; `engine.rs` sent nothing about it. The monologue arrives as `reasoning_content`, a sibling
of `content`, which §13 already noted we drop from the transcript — dropping it hides it from
the next round's prefill, which is worth doing, but it does not un-pay for the tokens.

Asked the sheep riddle ("all but 9 die — answer with just the number") against the live engine:

| request | prompt tok | completion tok | wall | reasoning |
| --- | --- | --- | --- | --- |
| default | 77 | 64 — **hit the `max_tokens` cap** | 2.86 s | 217 chars |
| `chat_template_kwargs.enable_thinking=false` | 37 | **2** | **0.90 s** | none |
| `enable_thinking=false`, top level | 77 | 64 | 2.63 s | 211 chars |
| `chat_template_kwargs.thinking=false` | 77 | 64 | 1.85 s | 217 chars |
| `reasoning_effort=low` | 65 | 22 | 2.07 s | 53 chars |

Every row answered "9". Two mechanics matter more than the numbers:

* **only the nested spelling does anything.** Top-level `enable_thinking` and
  `chat_template_kwargs.thinking` are both accepted with a 200 and ignored. A setting that is
  accepted-and-ignored is worse than one that errors, because it survives review.
* **reasoning is billed against `max_tokens`.** A thinking model can spend the whole budget on
  monologue and answer with `content: null`, which is the exact case `agent.rs` retries —
  and `journalctl -u slack-agent-listen` since 2026-11-16 shows that path has run **twice**.
  One retry is a doubled turn, in front of someone standing in a room.

**So the obvious fix was tried and lost.** Turning thinking off, measured over the whole
14-question fixture (`crates/slack-agent/questions.txt`, one run per question per mode,
sequential so the GPU never served two of my own requests):

| | total | mean | median | worst | rounds | completion tok |
| --- | --- | --- | --- | --- | --- | --- |
| thinking **on** | 270.8 s | 19.3 s | 15.9 s | 59.4 s | 41 | 6694 |
| thinking **off** | **359.6 s** | 25.7 s | 18.3 s | 71.8 s | 44 | **8972** |

Off was **33 % slower**, with more rounds and more tokens, because without a scratchpad the
model calls more tools: "how hot is aibox" went 43.1 s → 71.8 s (1061 → 1856 tokens) and
"turn the heaters on in the workshop" went 20.6 s → 39.7 s. A few single-tool questions did
win — "how many devices are on the network" 9.6 s → 5.8 s, "are the cameras working" 19.2 s →
13.4 s — and the tool *set* changed on 7 of 14 questions, none of them wrong enough to call a
regression. n=1 per cell at temperature 0.2, so read the trend and not any single row.

The conclusion is the boring one and it is now the default in `engine::Thinking::On`: **the
thinking is not the cost centre, the extra tool chatter is**, and taking away the model's
notes to save 40 tokens a round spends far more than that on wandering. The switch stays
(`--thinking on|low|off`, `SLACK_AGENT_THINKING`, reported in
`slack-agent-listen`'s `/status.json` as `"thinking":"on"`) so this is a dial and not a
rumour — `low` is the option worth revisiting first if a voice answer ever needs a third of a
minute again.

What the run actually taught us to fix instead is in `docs/follow-ups.md`: the questions
everyone asks have **one obvious tool and no judgement in them** ("what time is it", "is it
open", "how warm is the workshop", "how many devices"), and §13 already answers "what's on"
that way — a deterministic path that formats a tool result and posts it, ~1 s instead of
17. The same shape covers the rest, and it is where voice latency goes, not in the thinking.

## 16. The fast path, measured (2026-11-17)

§15 said the cost centre was extra tool chatter, not the thinking. This is the fix that
followed from that sentence: questions with **one obvious tool and no judgement in them**
get that tool call and a template, and never meet a model. The shape is not new — "what's
on" has been answered this way since phase 1 — what changed is that it is now **one door
for every harness** (`Agent::quick_answer`), reached by a recognizer (`triage::quick`),
instead of one special case in the Slack loop.

Measured on the same 14-question fixture §15 used, so the columns are comparable:

| question | before (thinking on) | after |
| --- | --- | --- |
| is it cold in the hackspace | 12.7 s | **< 1 s** |
| what's the temperature in the workshop | 4.9 s | **< 1 s** |
| which room is the coldest and how cold is it | 5.2 s | **< 1 s** |
| is the hackspace open | 6.7 s | **< 1 s** |
| how many devices are on the network | 9.6 s | **< 1 s** |
| the other nine | 203 s | 203 s (untouched, on purpose) |
| **fixture total** | **270.8 s** | **~227 s**, five of them free |

Through the real voice path, end to end including piper and a WAV on disk:

```
"what time is it"       3.4 s   agent 0.0 s · 0 rounds · fast path
"is the space open"     2.5 s   agent 0.0 s · 0 rounds · fast path
"what's on tonight"     2.9 s   agent 0.0 s · 0 rounds · fast path
"how warm is the workshop" (before, through the model)  6.9-18 s
```

**Piper is now the floor.** When the answer costs 40 ms and the whole turn costs 2.5-3.4 s,
the remaining 2.4 s is synthesis: piper is spawned per turn and loads its voice every time.
That, not the model, is what the next second comes out of — a warm synthesizer and audio
that streams to the browser as it is made.

Three findings from wiring it, all of them design rather than decoration:

* **A refusal must be an error, not an answer.** `space_status` with an unknown room returns
  `Err`, and `quick_answer` turns any error into a fall-through to the model. Tested live:
  "how warm is the cnc" → the tool says *there is no CNC sensor* → the model takes it and
  says so, correctly. Recognition is allowed to guess (`Room("cnc")` is a reasonable guess);
  the guess is not the last word. Had the refusal been an `Ok("I cannot see that room…")`,
  the fast path would have posted it as the answer, and the fast path is the code path
  nobody reads when something is 25× faster than it used to be.
* **An old server silently ignores a new argument.** While testing against the *running*
  MCP server, "how hot is aibox" hit the fast path and answered with the entire building's
  status: the deployed tool had no `focus` parameter, so it returned `Ok(everything)`. Same
  failure class as the `enable_thinking` spelling above — **accepted, ignored, 200 OK** —
  which makes it a pattern worth remembering rather than an anecdote: *ship the tool before
  the caller that trusts it to say no.*
* **A wake word in the transcript defeats the recognizer.** `"how warm is the workshop"`
  matches; `"hey jarvis how warm is the workshop"` does not, because the recognizer counts
  two named words and backs off to the model (measured: 4 rounds, 19.1 s). The camera
  satellite already strips its wake phrase (`wake::strip`) before asking; **whatever strips
  the wake word in the browser and server-wake paths must do it before `quick_answer` sees
  the text**, or the fast path works in the lab and not in the room. Filed in
  `docs/follow-ups.md`.

The recognizer errs toward silence by design: past tense, a noun that is not the building
("is the door left open"), two rooms at once, anything over 200 characters — all `None`,
all the model. Its filler list deliberately contains **no room names**: the space API owns
that table, and `AGENTS.md` rule 31 is a whole paragraph about what a second copy of a list
becomes.
