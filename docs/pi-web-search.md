# pi's web search: one Brave key, one pinned third-party package, nothing in the store

**Written 2026-11-16.** The ask was "add Brave search to pi, through the flake, so all
machines benefit". What was built is four moving parts and one deletion.

| You are about to… | Read |
| --- | --- |
| turn it on/off, move the key, or bump the package | §The four parts, then `common/pi-search.nix` |
| wonder why a search did not happen | `just pi-search-status`, then §What can go wrong |
| wonder whether this third-party code should be running here at all | §What we are trusting, and the open review in `docs/follow-ups.md` |
| change how `settings.json` / `web-search.json` get written | `crates/pi-agent-sync/README.md` (the merge rules and their tests) |

## The four parts

```
secrets/secrets.yaml ──sops──▶ /run/secrets/brave_api_key   0440 root:brave-search-readers
                                          ▲                          ▲
                                          │ `!cat` at request time   │ members =
                                          │                          │ programs.pi-agent.users
common/pi-search.nix ──▶ programs.pi-agent.extraConfigSections
                          { piPackages = ["npm:pi-web-access@0.48.0"];
                            webSearch  = { provider, braveApiKey, workflow, allowBrowserCookies }; }
                                          │
                                          ▼  (rendered by common/pi-agent.nix into
                                             <store>/pi-agent-config.json, 0444, no secret)
                          pi-agent-<user>.service  (the existing timer, unchanged shape)
                                          │  merges, per file, never clobbers
                          ┌───────────────┴────────────────────────┐
                          ▼                                        ▼
        ~/.pi/agent/settings.json                    ~/.pi/agent/web-search.json
        only `packages` is touched                   only the four keys above are touched
                          │                                        │
                          └──────────▶ pi installs the pinned package on next start,
                                       and the tools `web_search`, `fetch_content`,
                                       `get_search_content`, `source_check` appear
```

* **`common/pi-search.nix`** (MODULE) — the secret, the group, the package pin, the four
  settings. Imported by `common/pi-agent.nix`, not by the machines: it needs that
  module's `users`/`agentDir` to say *whose* config to write, and one import per machine
  is one fewer thing a new box can forget.
* **`programs.pi-agent.search.brave.enable`** defaults to `programs.pi-agent.enable`, so
  it is on on **all five** deployable machines without a line in any machine directory
  (rule 31's shape: declare it once, let every box get it).
* **`crates/pi-agent-sync`** grew the two merges (§Three files, three merge rules).
* **`crates/brave-search`** — deleted on the day it was started. See §Why not our own
  client.

## Where the key is, and where it is not

`brave_api_key` is a sops secret rendered to `/run/secrets/brave_api_key` at `0440
root:brave-search-readers`. The group's members are exactly `programs.pi-agent.users`,
today `leigh-admin` on every box.

It is **not** in the Nix store (nothing here goes through `config.sopsSecretText`),
**not** in `argv`, **not** in an environment variable, and **not** in `web-search.json`.
What `web-search.json` holds instead is a *source*:

```json
"braveApiKey": "!/run/current-system/sw/bin/cat /run/secrets/brave_api_key"
```

The package runs that command per request — five-second timeout, 16 KiB cap, a
minimised environment, and its errors redact the resolved value — and uses stdout. So
the boundary around the credential is the file's group membership, which is the same
decision `machines/gw/conntrack-reader.nix` and `machines/gw/lease-reader.nix` make for
root-owned files a service must read (AGENTS.md rule 28), and it survives everything a
value does not: it is not in the closure, not in `ps`, not in the session transcript,
and rotating it is `sops` plus a switch with no rebuild of anything that embedded it.

Why not the environment? Because the only declarative ways to put a value in a login
environment on this nixpkgs are `environment.variables`/`sessionVariables`, and both
write the **value** into `/etc/profile.d/` and into the system closure — a credential
world-readable by every account on five machines and remembered by the store forever.
That is the exact failure `docs/pi-agent-models.md` §Credentials documents for the old
`models.json`; writing it down here so the next person does not reach for it.

## Three files, three merge rules

`pi-agent-sync` already owned one file in `~/.pi/agent`. It now merges three, under one
rule — **only what it can prove it wrote may be removed** — with provenance kept in
`pi-agent-status.json` because none of the three files is ours to mark up:

| file | what may change | removal allowed when |
| --- | --- | --- |
| `models.json` | a provider whose id the config names | (replaced wholesale — the config names it) |
| `settings.json` | the `packages` list | `managedPackages` from the last run names the entry |
| `web-search.json` | the keys the config names | `managedWebSearchKeys` from the last run names the key |

`settings.json` also holds `defaultProvider`, `defaultModel` and `defaultThinkingLevel`,
which are a person's and are copied through; `packages` is a plain list, so replacing it
would silently uninstall someone's extension. `web-search.json` is written **by the
extension** too (`/websearch`, `/curator` persist into it), so the same care applies, and
it is *copied* rather than symlinked onto the store: a write through a symlink onto a
read-only store path is `EROFS`, which would turn a key press into an unexplained
failure. When a write does happen, the emitter sorts object keys — so a hand-written
file gets reordered once, at the moment something of ours actually changes; a pass with
nothing to say leaves the bytes (and the mtime) exactly as they were.

Both files are merged on the existing 15-minute timer, so **a change here is live in a
quarter of an hour without anyone touching the box** — and `just pi-agent-sync` is the
"now" half.

## What is deliberately not configured

The package's defaults are the safe ones and pinning them all would be a second copy of
its documentation, with the exception of three that are worth naming because each is a
capability we do not want arriving in an upgrade:

* `provider: "brave"` — its default is `auto`, whose fallback chain **starts with Exa's
  hosted endpoint**, so an unconfigured install sends the query somewhere else before it
  reaches Brave. Pinning also makes "which provider answered" a fact in the flake rather
  than a fact about which keys happen to exist on a box.
* `workflow: "none"` — raw results. The alternatives open a curator window and make a
  second model call to write a summary; neither belongs in an agent session on a server.
* `allowBrowserCookies: false` — its `true` reads a Chromium profile's cookie database.
  Off today, pinned so an upstream flip of the default is a merge conflict rather than a
  behaviour change.

Left at the package's defaults on purpose, and worth knowing about: the **SSRF guard**
stays fully armed (it blocks private and reserved address ranges), which is what stops
`fetch_content` being a route from an agent session to the internal boxes on this
network. `fetchRouting.allowRemoteHostedProviders` and `ssrf.allowRanges` stay unset —
the second one is exactly the knob that would open the LAN to `fetch_content`, so
`extraSettings` is where it would go, and it should not.

## What we are trusting

`npm:pi-web-access` is third-party code that runs inside every pi session on five
machines: 0.48.0, 211 npm packages installed under `~/.pi/agent/npm`, an author's repo
at `github.com/nicobailon/pi-web-access`, ~150 TypeScript modules, its own `bin`, MCP
server, HTML/DOM parsing, PDF extraction and an SSRF guard. It is pinned to an exact
version because a pi package that is not pinned is code that changes underneath a
session with no commit in front of it (pi's own docs say to review third-party package
source before installing; that review is **open** in `docs/follow-ups.md`, and today's
evidence is that the Brave path does what it says, not that the whole package does only
that).

Two consequences worth stating out loud:

* The **first `pi` start after a deploy** runs npm against the registry to install it
  (~8 s here). If a box cannot reach the registry pi warns, skips that package's
  resources, and keeps running — a session with no web search, not a broken session.
* The pin survives as a string in one option. Bumping it is: read the upstream
  CHANGELOG, change `packageSource`, `just pi-agent-sync`, `just pi-search-status`, and
  note the version in the follow-ups review row.

## What can go wrong, and what it looks like

| situation | what you see |
| --- | --- |
| key file unreadable by this account | the credential command fails; the tool reports a provider error. `just pi-search-status` says so in the first block, with `id -nG` and the fix (the box has not switched, or the user is not in `programs.pi-agent.users`) |
| Brave refuses the key | 401 at request time. `just pi-search-status` prints `401` and points at the follow-up, never the key |
| no route (services3's dongle, an offline box) | provider error; `just pi-search-status` prints "no answer". The endpoint is public HTTPS on the front door's egress path like any other outbound call — no DNS record, no vhost, no firewall row |
| `settings.json` is not valid JSON | `note: settings.json is not readable JSON (…); leaving it untouched` in the run's output, the status file and `just pi-agent-status`. Nothing is written, by design |
| someone hand-adds the same package by hand | nothing: the entry is already right, no write happens. If the flake later drops the pin, the *hand-written* copy stays — the two are indistinguishable and keeping theirs is the non-surprise |
| the package has not installed yet | first `pi` start after the switch runs npm; until then the tools do not exist. `just pi-search-status` shows the declaration and `pi list` shows the install (here: 211 packages, ~8 s) |
| **the box switched, the grant is in `/etc/group`, and pi still cannot read the key** | the running session predates the switch: a process keeps the supplementary groups it was started with, and `pi` lives inside a long-lived tmux server. Measured on services1 on 2026-11-16: `id -nG` in the agent's session had no `brave-search-readers` while `su - leigh-admin -c 'id -nG'` did, and `/run/secrets/brave_api_key` was readable only in the latter. Fix: restart the tmux server (or log out and in). `just pi-search-status` says exactly this when it is the case, rather than "permission denied" |
| the first timer run after a switch did nothing new | possible: that run can execute the **previous** generation's `ExecStart` (observed 2026-11-16 — the activation run was logged against `…-pi-agent-endpoints.json`, the old name, and wrote nothing to the two new files, while `/run/current-system` and the loaded unit were both already the new ones). It is a start-ordering artefact, not a stuck config: `systemctl show -p ExecStart --value pi-agent-<user>` is the truth, and `just pi-agent-sync` is the re-run |
| curl says `option --config: had unsupported trailing garbage` | a boolean in a curl config file is the **bare option name**: `silent`, not `silent = true` (measured 2026-11-16, curl 8.23). It is the syntax every `--config -` invocation here uses |
| a literal credential is ever put in `extraSettings` | **evaluation refuses it**, naming the key: that object is rendered into a world-readable store path, so the check is rule 13 as an eval error rather than a comment |

## Why not our own client

The first implementation of this was `crates/brave-search`: a Rust CLI (zero external
crates, key handed to curl on **stdin** so it never enters `argv`, `--doctor` mode,
~1,000 lines plus a vendored skill telling the agent to run it). It worked; it was
deleted the same day for three reasons, written here so nobody rebuilds it:

1. **The package solves the credential problem better.** A `!command` source in a
   config the package already reads beats a binary we maintain that reads a file, and
   both end at the same group permission.
2. **A skill telling a model to shell out to our CLI is a worse interface than a tool
   the agent is shown.** `web_search`/`fetch_content` arrive with their schemas, bounds
   and stored-result retrieval; ours would have had a `~40 KB` JSON body and a man page.
3. **Two ways to search would be a drift, not a fallback.** The rule that owns this
   whole area (rule 2, and its sibling rule 31) is that one fact has one owner.

What we kept from it is the stdin-instead-of-argv argument, which is why
`just pi-search-status` hands curl a config file rather than a `-H` header.

## Verifying it, on a box

```
just pi-search-status      # key perms, live Brave 200, the two files, pi's own view
just pi-agent-sync         # apply the merges now instead of on the timer
just pi-agent-status       # what the last run saw, incl. managedPackages / managedWebSearchKeys
```

A live end-to-end that involves no model, from 2026-11-16, is also the fastest way to
tell "the credential is broken" from "the agent did not pick the tool" — the package
ships a standalone MCP server that reads the same config:

```bash
cd ~/.pi/agent/npm && (printf '%s\n' \
 '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"probe","version":"0"}}}' \
 '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
 '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"web_search","arguments":{"query":"nixos","numResults":2}}}'; sleep 25) \
  | timeout 60 node node_modules/pi-web-access/dist/mcp-cli.js
```

It answered `**Provider:** brave` with two sources. Note the `node` requirement: that
path uses the system/node pi was started with, and is a diagnostic, not the supported
surface — inside pi the tools are already registered.
