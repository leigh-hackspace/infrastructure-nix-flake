# pi-agent-sync

Writes `~/.pi/agent/models.json` from the endpoints this flake declares, **asking each
endpoint what it is running** instead of being told, and merges the flake's pi-package
declaration and web-search keys into the two other files in that directory. The
subsystem — why the endpoint list is Nix and the model list is a probe — is
`docs/pi-agent-models.md`; why `web-search.json` is in here at all is
`docs/pi-web-search.md`; this file is the code-level trap list.

```
pi-agent-sync --config /nix/store/…-pi-agent-config.json --out ~/.pi/agent/models.json \
              --status ~/.pi/agent/pi-agent-status.json \
              [--settings ~/.pi/agent/settings.json] [--web-search ~/.pi/agent/web-search.json]
pi-agent-sync --report ~/.pi/agent/pi-agent-status.json
```

The NixOS module (`common/pi-agent.nix`) runs the first form on a timer; `just
pi-agent-status` runs the second. `--dry-run` prints the document with credentials redacted
and writes nothing at all.

## The three files, and what may be changed in each

| file | this program's rule | provenance |
| --- | --- | --- |
| `models.json` | a provider id named by the generated config is replaced; everything else is copied through | the config names it |
| `settings.json` | only `packages`: declared entries are added, nothing is removed unless a previous run wrote it | `managedPackages` in the status file |
| `web-search.json` | only the keys the config names; same removal rule | `managedWebSearchKeys` in the status file |

## Layout

| file | job |
| --- | --- |
| `src/config.rs` | read the generated document (a `pkgs.writeText` store path the unit is handed directly — there is no `/etc/pi-agent`): endpoints, `piPackages`, `webSearch`. Absent field → default; malformed field → **refuse the whole file, listing every finding**. The two new sections are optional so an old config still loads on a new binary mid-upgrade |
| `src/probe.rs` | the two GETs, the two dialects, and what each engine said. Never fails: a dead endpoint is a `Report` with an `error` |
| `src/render.rs` | the merge with the file that is already there (three files, three merge rules, one shared rule: *touch only what you can prove you wrote*), the fallback ladders, and the redaction |
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

* **A file this program cannot parse is a file it does not touch.** `models.json` is the
  exception by necessity (its own providers are the point) and even there it says so;
  `settings.json` and `web-search.json` are refused outright on a parse error, a
  non-array `packages`, or a package entry that is not a string. `a_file_this_tool_cannot_read_is_left_alone`.
* **Removal needs provenance, and the provenance is in the status file.** "The flake no
  longer declares this package" and "a person added this package by hand" are the same
  bytes in `settings.json`, and getting them the wrong way round is silently
  uninstalling someone's extension on their next start. So an entry is removable only
  if `managedPackages`/`managedWebSearchKeys` from the last run names it — and with no
  status file (a reinstall) nothing is removable at all, which is the safe direction.
* **"No logical change" beats "byte-different".** A hand-formatted `settings.json` whose
  packages are already right is **not** rewritten: the emitter sorts keys and re-indents,
  so writing it to say the same thing produces a diff with no reason in it — and
  `web-search.json` is a file the *extension* also writes, so those diffs land in someone
  else's edit. Hence the two early returns in `merge_packages`/`merge_keys`, and
  `a_file_that_already_says_the_right_thing_is_not_rewritten_however_it_is_printed`.
* **`web-search.json` is copied, never symlinked**, even though nothing in it is secret.
  The extension persists a person's `/websearch` choices into it, and a write through a
  symlink onto a read-only store path is `EROFS` — a refusal that only appears when
  someone presses a key.

## Tests

`cargo test -p pi-agent-sync` (30, no network: the live bodies are fixtures). `just test` and
`just clippy` cover this crate with the rest of the workspace.
