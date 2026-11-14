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
| `llama-services3-v1-models.json` | services3's llama.cpp | `data[]` **plus** the Ollama-compatibility `models[]` — the double-list trap |
| `llama-services3-props.json` | services3's llama.cpp | `modalities: {vision: true, video: true}`, `model_ftype` |
| `front-door-502-body.txt` | services2 (engine stopped) | the nginx page a client gets when the upstream is off; an HTML body a JSON parser must survive |

**One edit was made, and it is inside the file:** `chat_template` on the two `*-props.json`
is ~40 KB of Jinja, so the fixtures keep a one-line marker plus the last 260 characters —
the part that emits `<think>`, which is the only thing this program reads from it. Everything
else is byte-for-byte the response. Re-capture with:

```bash
curl -sS https://strata2.int.leighhack.org/v1/models  > tests/fixtures/strata2-v1-models.json
curl -sS https://strata2.int.leighhack.org/props      > tests/fixtures/strata2-props.json
curl -sS https://llama.int.leighhack.org/v1/models    > tests/fixtures/llama-services3-v1-models.json
curl -sS https://llama.int.leighhack.org/props        > tests/fixtures/llama-services3-props.json
```

…then trim `chat_template` again before committing, and check the numbers in
`docs/pi-agent-models.md`'s table while you are there — those are the same measurements.
The bodies carry no credentials: both requests are unauthenticated GETs against the door,
which injects the engine's key itself.
