# pi-agent-sync

Writes `~/.pi/agent/models.json` from the endpoints this flake declares, **asking each
endpoint what it is running** instead of being told. The subsystem — why the endpoint list
is Nix and the model list is a probe — is `docs/pi-agent-models.md`; this file is the
code-level trap list.

```
pi-agent-sync --config /etc/pi-agent/endpoints.json --out ~/.pi/agent/models.json \
              --status ~/.pi/agent/pi-agent-status.json [--settings ~/.pi/agent/settings.json]
pi-agent-sync --report ~/.pi/agent/pi-agent-status.json
```

The NixOS module (`common/pi-agent.nix`) runs the first form on a timer; `just
pi-agent-status` runs the second. `--dry-run` prints the document with credentials redacted.

## Layout

| file | job |
| --- | --- |
| `src/config.rs` | read `/etc/pi-agent/endpoints.json`. Absent field → default; malformed field → **refuse the whole file, listing every finding** |
| `src/probe.rs` | the two GETs, the two dialects, and what each engine said. Never fails: a dead endpoint is a `Report` with an `error` |
| `src/render.rs` | the merge with the file that is already there, the fallback ladders, and the redaction |
| `src/main.rs` | arguments, one line per endpoint, the status document, `--report` |
| `tests/fixtures/` | the four live response bodies, captured 2026-11-14, plus the front door's 502 page |

## Traps

* **`/v1/models` has two dialects, both live here.** Strata answers with `data[]` carrying
  `meta.n_ctx`; llama.cpp answers with `data[]` *and* an Ollama-compatibility `models[]`
  with `capabilities`. Read both and you double-count; read the wrong one and you get a
  model with no id. `data[]` first, `models[]` only if it produced nothing.
* **A `200` with no models is not a usable answer.** An engine that is still loading answers
  it, and treating it as success deletes every model from every box. `Report::usable()` is
  that rule, and `models_from` returning an empty vec is indistinguishable from a parse
  failure — which is deliberate: both mean "keep what you had".
* **`params.reasoning_format` is not a capability.** On services3 it reads `"none"` while the
  model's own chat template ends with `<think>`. It is a request-*parsing* knob. The
  capability signal is `<think>` in `chat_template`.
* **`video` is dropped, not mapped to `image`.** pi's `input` is `text`|`image`. services3's
  Ternary reports `["text","image","video"]`; claiming the image half from a video modality
  would be a lie about a capability.
* **Every fallback is sticky, and the stickiness is recorded.** Context window, input
  modalities and `reasoning` fall back to *the previous run's value* before they fall back to
  a declaration, because a transient `/props` failure must not silently turn thinking off or
  shrink a window. The source string (`ctxSource`, `reasoningSource`, `propsSaysThinking`)
  goes into the status file — that is the only way to tell "this engine has no context to
  report" from "the number on screen is a week old".
* **A kept-stale provider takes its shape from the config and only its `models` from the
  file.** Otherwise "endpoint temporarily off" means "the credential in its old entry lives
  here forever" — which is precisely what the pre-flake file was doing with `strata_api_key`.
  `a_dead_endpoint_keeps_its_models_but_not_its_credential`.
* **Nothing is printed unredacted.** `--dry-run` runs `render::redact`, because the document
  can contain a provider this program only *preserved*. AGENTS.md rule 13 does not pause for
  a `println!`.
* **No HTTP client crate.** TLS to the front door from `std` is not a reviewable thing, and
  `common-rs/web` is server plumbing. `curl` comes from the unit's `path`; `--curl` names it
  absolutely. Same shape as `crates/ups-watchdog` (which shells out to `ssh`/`systemctl`/`upsc`
  for the same reason).
* **The JSON emitter lives in `common-rs/json`, not here.** Merging means writing back a
  document this program did not fully author, so `emit` had to be a general round trip;
  `emit_round_trips_a_document_it_did_not_write` is the property, and integer-vs-`262144.0`
  is why `number()` exists.
* **`maxTokens` is policy, not a measurement.** `n_ctx` is the *whole* window; the divisor
  (`8`, from Nix) says how much of it to promise back. It reproduces the two hand-written
  numbers this replaces, which is the only sense in which it was "measured".
* **Reruns must not churn.** The output is byte-stable (keys sorted, models sorted by id) and
  the file is only written when it actually changed — a config that shuffles on a timer makes
  every diff meaningless. `rerunning_on_unchanged…` holds that.
* **A skipped run is a loud run.** If the agent directory is absent the program prints `SKIP:`
  with the reason and exits 0, writing no status file. The unit deliberately does **not** use
  `ConditionPathIsDirectory=`: a skip that leaves no trace is how "the config was never
  written on the newest box" stays hidden until someone needs it.
* **Replace-in-place vs replace-the-path is decided by who owns the descriptor.** Here a
  rename is right (pi opens `models.json`, reads, closes); AGENTS.md rule 29 is the case where
  it was catastrophic. Do not copy either decision across.
* **`cargo test` runs tests on threads of one process.** The first version of the test helpers
  keyed temp files on `std::process::id()` and two tests raced on one path, which surfaced as
  "unexpected end of input" on a perfectly valid fixture. Unique per call.

## Tests

`cargo test -p pi-agent-sync` (21, no network: the live bodies are fixtures). `just test` and
`just clippy` cover this crate with the rest of the workspace.
