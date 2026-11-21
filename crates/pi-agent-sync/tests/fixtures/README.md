# Fixtures: what the fleet's inference endpoints actually answer

Captured with `curl` from the live engines on **2026-11-14**, through the front door
(`https://strata2.int.leighhack.org/v1/models`, `https://llama.int.leighhack.org/…`), and
read by `src/probe.rs`'s `the_captured_live_bodies_produce_the_measured_answers`. They exist
so the two `/v1/models` dialects are a test rather than a recollection, and so an engine
update that renames `meta.n_ctx` or reshapes the list fails here instead of quietly rewriting
four boxes' `~/.pi/agent/models.json` with nothing in it.

| file | endpoint | what it is |
| --- | --- | --- |
| `strata2-v1-models.json` | aibox's Strata | the dialect with `data[].meta.n_ctx` **and** `status.value` |
| `strata2-props.json` | aibox's Strata | `default_generation_settings.n_ctx`, `modalities.vision: false`, the thinking block |
| `llama-services3-v1-models.json` | services3's llama.cpp (**gone**) | `data[]` **plus** the Ollama-compatibility `models[]` — the double-list trap. Kept after the engine was replaced by Strata on 2026-11-21: llama.cpp is still a dialect this parser must survive, and this file is now the only copy of that endpoint's answer |
| `llama-services3-props.json` | services3's llama.cpp (**gone**) | `modalities: {vision: true, video: true}`, `model_ftype` — same note |
| `front-door-502-body.txt` | services2 (engine stopped) | the nginx page a client gets when the upstream is off; an HTML body a JSON parser must survive |

**One edit was made, and it is inside the file:** `chat_template` on the two `*-props.json`
is ~40 KB of Jinja, so the fixtures keep a one-line marker plus the last 260 characters —
the part that emits `<think>`, which is the only thing this program reads from it. Everything
else is byte-for-byte the response. Re-capture with:

```bash
curl -sS https://strata2.int.leighhack.org/v1/models  > tests/fixtures/strata2-v1-models.json
curl -sS https://strata2.int.leighhack.org/props      > tests/fixtures/strata2-props.json

# The two llama.cpp bodies cannot be re-captured: the endpoint they came from is gone
# (services3 replaced llama.cpp with Strata on 2026-11-21, and
# `llama.int.leighhack.org` was retired rather than repointed). They are the network's
# only copy of that dialect, so treat them as permanent fixtures, not as captures:
# re-capturing the llama.cpp half needs a box that runs llama-server again — aibox's
# displaced `llama-server.service` is the nearest one (`docs/follow-ups.md`).
```

The Strata pair above is the live capture; if services3's own engine is ever wanted as a
third sample, it answers the same dialect as aibox's at
`https://strata3.int.leighhack.org/v1/models` (`docs/pi-agent-models.md`, "The two
dialects").

…then trim `chat_template` again before committing, and check the numbers in
`docs/pi-agent-models.md`'s table while you are there — those are the same measurements.
The bodies carry no credentials: both requests are unauthenticated GETs against the door,
which injects the engine's key itself.
