# pi's inference endpoints: declared in Nix, discovered at run time

**Written 2026-11-14.** The ask was simple — stop hand-setting models on every machine. The
change is three files and a rule about where each fact is allowed to live.

| You are about to… | Read |
| --- | --- |
| add, move or retire an endpoint | §"What is declared" below, then `common/pi-endpoints.nix` |
| wonder why a model/number is wrong or missing | `just pi-agent-status` first, then §"What can go wrong, and what it looks like" |
| change how a context window or an output limit is computed | `common/pi-agent.nix`'s `maxTokens*` options and `crates/pi-agent-sync/src/render.rs` |
| wonder whether to write a pi extension instead | §"Why not an extension / a build-time file / a static file" |

## The three pieces

```
common/pi-endpoints.nix  ──(submodule defaults)──▶  /etc/pi-agent/endpoints.json   0444, no secrets
   the endpoint table                                      │
   (name, URL, dialect, verified compat,                    ▼
    output-limit policy)                        pi-agent-<user>.service  (oneshot, User=<user>)
                                                pi-agent-<user>.timer    (OnBootSec=2min,
                                                         │                OnUnitActiveSec=15min,
                                                         │                Persistent)
                                                         ▼
                                          $HOME/.pi/agent/models.json         ← pi reads this
                                          $HOME/.pi/agent/pi-agent-status.json ← what was probed
```

* `common/pi-endpoints.nix` (DATA) — every endpoint, one row each.
* `common/pi-agent.nix` (MODULE) — `programs.pi-agent.{enable,endpoints,users,refreshInterval,probeTimeout,…}`;
  renders the table, owns the unit and the timer. Enabled on **all five** machines.
* `crates/pi-agent-sync` (Rust, workspace member, zero external crates; HTTP is `curl` on the
  child path — the reason is its `Cargo.toml`) — probes, decides, merges, reports.

`just pi-agent-status` reads the last run; `just pi-agent-sync` starts the unit now. Both are
installed-binary invocations, so both are safe on `gw` (rule 22).

## The rule this implements

**A fact about a running process does not belong in a build-time file.** Which models an
endpoint serves and how large a context it serves them with are properties of a live engine,
and every number of that kind this repo carried had already gone stale — measured
2026-11-14 by asking the engines themselves:

| row | the checked-in claim | what the engine said |
| --- | --- | --- |
| `strata` (aibox) | `qwen3.8-flash-next-iq2xs`, context **131072**, max 16384 | same id, `meta.n_ctx` **262144** |
| `strata-zen3` | `qwen3.8-flash-next-iq3xxs`, context **262144** | id is **`qwen3.8-flash-next-unsloth-ud-iq4-xs-v31`**, `meta.n_ctx` **393216** |
| `strata-services2` | `qwen3.8-flash-next-coder-iq1m`, 131072 | the one number that was **right** — but only two hours later: at the first attempt the engine was `inactive` and the door 502s it, and the sync kept the previous list; at 15:09 it answered `meta.n_ctx` **131072** |

The `zen3` row had not merely drifted: its model id no longer existed, so that provider had
been offering a model the endpoint would not serve, on every box, indefinitely.

And the pairs of numbers prove the point from the other side: **the two Strata engines report
different context windows for models with the same name** — 262144 on aibox, 131072 on
services2, both measured through the door within two hours of each other on 2026-11-14. A
hand-written list has to be told that, per row, per box, forever; asking takes one GET.

**Where a model id is still written down, and why that is the right end of it:** the engines
themselves — `machines/services2/strata.nix`'s `model_name = "qwen3.8-flash-next-coder-iq1m"`,
`machines/services3/llama.nix`'s `modelName`, and aibox's equivalent — name the model they
serve, because that is the process *deciding* what it is rather than a client guessing, and it
is the id `GET /v1/models` then answers with. Nothing downstream of a socket repeats a model id
any more: `grep -rn "qwen3.8-flash-next" --include=*.nix` finds the engines' own config and
prose, not a client's list.

## What is declared, and what is discovered

| field of `models.json` | source | notes |
| --- | --- | --- |
| provider `id` | the attribute name in the table | **load-bearing**: `settings.json`'s `defaultProvider` names it, and the merge replaces providers *by id* |
| `baseUrl`, `api`, `apiKey`, `headers` | declared | `apiKey` is a **placeholder**; see §credentials |
| model `id` | `GET {baseUrl}/models` | two dialects, both live here — §dialects |
| `contextWindow` | `data[].meta.n_ctx`, else `/props` `default_generation_settings.n_ctx`, else last run, else declared | the ladder is the feature; each rung is recorded in the status file as `ctxSource` |
| `input` | `architecture.input_modalities`, else `/props` `modalities.vision`, else last run, else declared | pi's vocabulary is `text`/`image`; services3's `video` modality is **dropped**, not translated into an image claim |
| `reasoning` | the loaded chat template contains `<think>` | **not** `params.reasoning_format`: on services3 that reads `"none"` while the template ends with `<think>` — it is a request-*parsing* knob, not a capability. `reasoning = "auto" | "true" | "false"` per endpoint to override |
| `maxTokens` | policy: `n_ctx / 8`, clamped to `[4096, 32768]`, never above `n_ctx` | one eighth reproduces **both** hand-written values it replaces (131072→16384, 262144→32768). Nothing an engine reports says how much of the window you may ask back, which is why this is a policy in Nix rather than a measurement in Rust |
| `compat`, `thinkingLevelMap`, `cost` | declared, verbatim | not discoverable. `compat` is provider-level (pi merges model over provider per field) so one copy covers whatever the endpoint loads next; `thinkingLevelMap` is a *model* field in pi's schema, so it is written on each model |
| `name` | generated: `"<model id> — <provider name>"` | |

Declared fields use `pi`'s own guidance on `compat` — *describe verified differences, do not
enable them because an endpoint advertises OpenAI compatibility* — which is why
`llama-services3` has **no** `compat` block: nobody has measured its tool-calling or
usage-in-streaming behaviour from pi, and an unverified `true` there is a claim, not a config.

## Credentials: the finding, and the open item

Before this, `~/.pi/agent/models.json` on **four boxes** dialled the engines' addresses
directly (`http://10.3.1.21:8080/v1`, `http://10.3.1.32:8080/v1`) and therefore held the real
`strata_api_key` — a 12-character Bearer key, in a dotfile, on four machines, in whatever
backups and screen-scrapes and transcripts those directories reach. (`machines/aibox/STRATA.md`
says the file was pointed at the front door "on purpose"; the copy on the boxes had drifted
back to the addresses, which is exactly what a hand-maintained per-user file does.)

Now every row is a name the front door serves, `strataBearer = true` injects the key at
`gw` (`common/front-door.nix`), and the placeholder is the only thing a client holds. Two
consequences:

* **`strata_api_key` should be rotated** — it is in the store (by design, for the door) and it
  *was* in four home directories (by accident). Filed in `docs/follow-ups.md`. Rule 13 applies:
  it has never been printed here.
* A provider whose endpoint is **down** keeps its previous model list, so a stale entry could
  have kept a stale credential forever. It does not: the shape of a kept-stale provider
  (`baseUrl`, `apiKey`, `compat`, `headers`) is rebuilt from the current config and only the
  `models` array is carried over. `render.rs`'s
  `a_dead_endpoint_keeps_its_models_but_not_its_credential` is that rule as a test.
* `--dry-run` prints the document **redacted** (`render::redact`), because the document can
  contain a provider this program merely preserved.

## The two dialects

`GET {baseUrl}/models`, live on this network, in two shapes:

* **Strata** (aibox, services2): `data[]` with `id`, `status.value`, `meta.n_ctx`,
  `architecture.input_modalities`. The context window is *in the model list*.
* **llama.cpp** (services3): `data[]` with `id`, `architecture`, **and** an Ollama-compatibility
  `models[]` list with `capabilities` — so a reader that walks the first array it finds can end
  up with a model that has no id, and one that walks both can double-count. `probe.rs` prefers
  `data[]` and falls back to `models[]` only if `data[]` produced nothing.

Both engines also serve `GET /props` (not part of the OpenAI API), which is where the loaded
model's `n_ctx`, `modalities` and `chat_template` live. The four bodies are checked-in fixtures
in `crates/pi-agent-sync/tests/fixtures/`, captured from the live endpoints on 2026-11-14 —
plus the front door's 502 page, which is what a stopped engine actually returns to a client.
Only `chat_template` is trimmed (~40 KB of Jinja; the fixture keeps the tail that emits the
thinking block, and says so in the value).

## What can go wrong, and what it looks like

Every one of these is a line in the journal, in `just pi-agent-status`, or both. That is the
design: a config tool that succeeds silently is indistinguishable from one that did nothing.

| situation | behaviour |
| --- | --- |
| engine stopped / door 502s | provider keeps its previous `models` list; `keptStale` in the status; `note: <id>: keeping the previous model list (N model(s)) — GET …: HTTP 502` |
| engine answers `200` with **no models** | treated as *not usable* — a loading engine must not delete every model from every box. The status records the empty answer with both HTTP codes |
| `/props` unreachable, `/models` fine | context and modality fall back to the last run; `reasoning` likewise (a lost `/props` must not turn thinking off). Recorded as `ctxSource: "previous run"`, `propsSaysThinking: null` |
| no context window from anywhere | the field is **omitted** (so pi uses its documented 128000) and the run carries a warning naming it. Not invented |
| pi not installed for that user | `SKIP: /home/leigh-admin/.pi/agent does not exist — pi is not installed for this user here`, exit 0. Deliberately not `ConditionPathIsDirectory=`, which is the *silent* kind of skip. `createAgentDir = true` opts into creating it |
| `settings.json`'s `defaultModel` is no longer in the file | one note per run: the program does not touch people's preferences, but it can make one impossible (the engine changed quant) and that is worth a line rather than a silent fallback |
| every engine down for hours | `lastSuccessUtc` goes stale while `generatedUtc` stays fresh: "the timer is running and nothing answers" is a different diagnosis from "the timer is not running", so the status carries both |
| a hand-written provider in the file | preserved untouched (every other provider id, and every other top-level key, including `$schema`) |

The file is written by rename-and-replace with the previous mode preserved. That is safe here
and is *not* a general lesson: pi opens `models.json`, reads it and closes it, whereas the
dnsmasq lease file of rule 29 is rewritten in place on a long-lived descriptor, where replacing
the path is the broken thing. The owner of the descriptor decides which is which.

`pi` re-reads `models.json` when `/model` is opened, so a timer write is picked up without a
restart.

## Why not an extension, a build-time file, or a static file

* **A provider extension with `refreshModels`** is pi's documented answer for "the catalog
  comes from a live service", and it would discover the models. It was not chosen because the
  endpoints would then live in a per-user `.js` file — the thing being removed — and pi's own
  docs say to prefer `models.json` when the service already speaks a supported API, which both
  engines do. If this ever needs custom *auth* (an endpoint with a real client-side secret, an
  OAuth flow), the extension is the right shape and the Nix module is still the right place to
  distribute it.
* **Generating `models.json` at build time** would put a network request inside a derivation
  (or worse, an evaluation: rule 22 and "Evaluation must not build anything"), and would bake
  one box's view of the engines into every box — including the box whose engine was the one that
  happened to be awake.
* **A static file in `/etc` that pi reads** does not exist: pi reads `models.json` from the
  agent directory (`~/.pi/agent`, or `$PI_CODING_AGENT_DIR`), per user. `programs.pi-agent.agentDir`
  follows that, so a box with a relocated agent directory sets the option rather than
  hardcoding a path.

## Adding an endpoint, and checking it

```nix
# common/pi-endpoints.nix, or per machine:
programs.pi-agent.endpoints."mine" = {
  name = "Strata (bench)";
  baseUrl = "https://mine.int.leighhack.org/v1";   # a front-door name if it needs a key
  # apiKey stays a placeholder; compat/thinkingLevelMap only if measured
};
```

A `*.int` name still needs its rows like any other vhost — `common/int-dns.nix` **and**
`common/front-door.nix`, guards and all (rules 14 and 24). Then:

```
just pi-agent-sync     # run the unit now, print the result
just pi-agent-status   # what the last run saw, per endpoint, with each number's provenance
```

and, per rule 10, if you changed the unit itself: read the rendered one back from a real build
(`result/etc/systemd/system/pi-agent-<user>.service` after building that machine's toplevel)
rather than believing the module.

## Open

* `strata_api_key` rotation (`docs/follow-ups.md`).
* Nothing watches the timer. `lease-reader-watch.timer` is the precedent for a unit that fails
  when its channel dies; a `pi-agent` equivalent (last success older than an hour *and* at
  least one endpoint configured) wants a decision about who is paged by a stale model list,
  which nobody has asked for yet.
* services3's `compat` is empty because it is unmeasured, and its `input` says `image` because
  the *engine* says so — nobody has run a picture through pi on that endpoint.
* `settings.json` (`defaultProvider`/`defaultModel`/`defaultThinkingLevel`) is still per-person,
  hand-edited state. This module can now *say* what the defaults should be; it does not.
