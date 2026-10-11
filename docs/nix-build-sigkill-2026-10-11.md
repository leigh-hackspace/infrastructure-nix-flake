# A nix build SIGKILLed the admin's whole session — 2026-10-11

Post-mortem. On services1, for roughly an hour, **any** nix build — `nixos-rebuild`
`switch`/`boot`/`build`/`dry-activate`, or a bare `nix build` — killed every process owned
by `leigh-admin` (uid 1234) within seconds: the shell it was typed in, tmux, the Zed remote
server, `user@1234.service`, and `network-status.service`. It did it to a human at their own
terminal, not just to agents, and it did it even for `dry-activate`, which changes nothing.

The box was never broken. The system kept serving (nginx, sshd, podman, the dashboards) and
no bad generation was ever activated. What was broken was the **ability to deploy**: the
only way to apply a fix was to run a build, and every build killed the session running it.

## Root cause

`strace` on `nix-daemon`, for signal syscalls, caught the kill directly:

```
1750194 clone(...)                      = 1750544
1750544 kill(-1, SIGKILL)               = 0          # every process this uid may signal
1750194 --- SIGCHLD ... si_pid=1750544, si_uid=1234
```

and, with uid-setting included:

```
1770847 setuid(1234)                    = 0
1770847 kill(-1, SIGKILL)               = 0
```

`kill(-1, SIGKILL)` is nix's build-user cleanup: nix runs it after a build to sweep leftover
processes belonging to the uid the build ran as. It is written to target a throwaway
`nixbld` uid. On this box there was no throwaway uid, and it resolved to **1234 — the
invoking user** — so the sweep took out the admin's entire process set.

That is the whole bug. Everything else in this document is either why nix had no build uid
or why the cleanup fired at all.

### Why nix had no build user

Current nixpkgs ships these together in `nix-daemon.nix`:

```nix
nix.settings = {
  sandbox = true;
  auto-allocate-uids = true;
  # No such group would exist within the sandbox, so chowning to it would fail
  build-users-group = "";
};
```

The empty `build-users-group` is deliberate: it is paired with `sandbox = true` plus the
`auto-allocate-uids` *experimental feature*, which allocates uids inside the sandbox. This
repo does neither — `experimental-features` is only `nix-command flakes`, and
services1/services2 override `sandbox = "relaxed"` (needed for the llama.cpp build) — so
nix had no uid of its own to build or clean up as.

### Why the cleanup fired

The cleanup only runs when a build is **interrupted or failed**. The daemon's own journal
puts it immediately after the interruption, every time:

```
accepted connection from pid 1772385, user leigh-admin
unexpected Nix daemon error: error: interrupted by the user
```

The first switch of the day completed cleanly and killed nothing. Every kill after it
followed an interrupted build. The new source of interruptions was the **agent harness's
per-call time limit**: a build that outran the limit had its client killed, which is an
interrupted build, which triggered the sweep.

### Why the early mitigations failed

`systemd-run` **waits for a oneshot unit to finish** before returning unless you pass
`--no-block`. So the "run the build in the background and poll a log" wrapper still blocked
the tool call for the whole build, and the call still got killed — which still produced an
interrupted build and still fired the sweep. Several attempts were spent concluding "the
scope didn't help" when in fact the scope never detached.

## What was ruled out (each looked convincing in the journal)

| Suspect | Why not |
|---|---|
| The activation script | `dry-activate` changes nothing and still killed; `$system/activate` contains no `kill` |
| OOM / `systemd-oomd` | Memory and swap were idle; oomd logged nothing |
| `auto-rollback.timer` | It had self-deactivated hours earlier |
| `linger` | Set and still killed: the user *manager* is killed directly, not via a session teardown |
| Rolling back a generation | `build-users-group` is absent from `nix.conf` in **every** generation (`system-{652..657}`), so an older generation behaves identically |

The last one matters: rolling back was proposed as the recovery, and it would have cost the
running containers and the NAS-mount boot sequence for no change in behaviour.

## How it was escaped

The sweep kills a **uid**. So: build as a uid that owns nothing, from a unit that cannot be
interrupted, then activate the already-built closure (activation does no build).

1. A `deploy` account with its own copy of the repo and the two private flake inputs.
2. The build, as `deploy`, in a root-launched unit with `--no-block` (so the tool call
   returns instantly and nothing interrupts it).
3. The activation, from the printed store path, with `switch-to-configuration switch`
   followed by `nixos-confirm` **in the same unit**.
4. Verified: `/run/current-system` = `yp5r292ffsrnjx0vrfp31q5vil1cq0q1-…`,
   `system-good` = the same generation, and `build-users-group = nixbld` live in
   `/etc/nix/nix.conf`.

The exact commands are in `AGENTS.md` ("Deploying when every build kills the session"),
because that is guidance; this file is the story.

**The escape is not a cure.** Later the same day, sweeps were observed after a *successful*
build, and after a build authored by the `deploy` account rather than uid 1234. So the cleanup
targets uid 1234 regardless of who asked, and does not need a failure to fire. The `deploy`
account lets a build **finish** (nothing interrupts it) and keeps it out of an interactive
session's path, but a sweep can still take out any interactive session — or any
`User=leigh-admin` service — on that machine. Until the `sandbox = "relaxed"` thread is
resolved, treat **every** nix build on services1/services2 as able to kill `leigh-admin`'s
processes.

**One gap in the escape:** `switch-to-configuration switch` activates the running system but
does **not** update `/nix/var/nix/profiles/system`, which is what the bootloader boots. Both
machines came up running the fixed generation while still *booting* the pre-fix one, so a
reboot would have silently reverted the fix (on services1 that means losing
`build-users-group` and being sweep-prone again). Re-pointed with `nix-env --profile
/nix/var/nix/profiles/system --set /run/current-system && /run/current-system/bin/switch-to-configuration boot`;
the updated recipe in `AGENTS.md` includes that step.

## Honest gaps

- **`build-users-group = "nixbld"` did not demonstrably stop the sweep.** It is committed as
  correct hygiene (nix should not build as an interactive admin), but a build started after
  it was live still resolved the uid to 1234. The mechanism that picks 1234 is not fully
  closed; the box was unblocked by keeping the build away from that uid entirely.
- **"nixpkgs changed the default two weeks ago" is inference.** Only the current nixpkgs is
  in the store, so it could not be diffed against the previous pin. The `build-users-group =
  ""` + `auto-allocate-uids` pairing is directly read from the module.
- The `deploy` account was created **imperatively** (`useradd`) and its `flake.nix` points
  at copied inputs. It is a deliberate escape hatch, not managed config, and a future switch
  will not recreate it.

## Why aibox is unaffected (verified 2026-10-11)

aibox has the same nix (2.34.8) and the same empty `build-users-group`, but a **successful**
local build there produced **zero** SIGKILLs:

```
kills before: 0
BUILD4=0        # nix build --impure --expr 'with import <nixpkgs> {}; runCommand ...'
kills: 0
```

(Earlier probes on aibox failed with `executing '...bash': No such file or directory` — the
sandbox needs the builder to come from the store, so a bare `/run/current-system/sw/bin/bash`
is not in the build closure. Those failures are not evidence either way; the `runCommand`
build is, because it actually ran and succeeded.)

Two differences, either of which is sufficient:

1. **`sandbox = true`, not `relaxed`.** Only services1 and services2 override the NixOS
default, to `relaxed`, for the llama.cpp build. The sweep appears only on the machines with
`relaxed` — which is the strongest remaining clue about *why* nix resolves the cleanup uid to
the invoking user there. This is the thread to pull for the real fix.
2. **Nothing on aibox runs as `leigh-admin`.** `ps -eo user` shows zero `leigh-admin`
processes and no unit sets `User=leigh-admin`, so even if the sweep did fire it could only
kill an interactive shell — never a service. services1 lost `network-status` precisely
because that unit runs as the admin.

aibox imports `common/base.nix`, so its next deploy will also pick up
`build-users-group = "nixbld"` (harmless; not the fix).

## So it cannot happen again

Ordered by how much they would have shortened this.

1. **Never build as the interactive admin.** Builds go through a dedicated uid (the `deploy`
   account) or a root-launched unit. This alone would have prevented every session death.
2. **Always `--no-block`** on `systemd-run`, and poll a log file. Without it the wrapper is
   not a wrapper.
3. **Separate build from activate.** `switch-to-configuration switch` on a built closure
   cannot be interrupted into a bad state, and does not build.
4. **Keep agent calls short.** Anything that may compile for minutes belongs in a unit whose
   lifetime is not tied to the call.
5. **`network-status.service` should not run as `User=leigh-admin`.** It is what turned a
   uid-wide kill into a service outage (it restarted seven times). Blocked on the hop key
   it needs: a dedicated user would need that key in sops, or a dedicated router key.
6. **Finish the real fix.** Either make build users actually take effect (the
   `sandbox = "relaxed"` interaction is the unexamined part), or enable
   `auto-allocate-uids`, so nix has a throwaway uid and the empty `build-users-group`
   default is harmless — which is what nixpkgs assumes.
7. **`nix-gc` was failing the whole time** with an option string `nix-collect-garbage`
   doesn't accept (`--max-age`); fixed in the same pass.

## If it happens again

Symptoms: a build/switch kills the terminal mid-way; `journalctl` shows
`mm_reap: child terminated by signal 9` then `user@1234.service: ... status=9/KILL`, and
nix-daemon shows `interrupted by the user` just before.

1. Do **not** reboot or roll back — nothing is broken and all generations behave the same.
2. Do **not** run the switch as root. The sweep is `setuid(uid); kill(-1, SIGKILL)`; with
   uid 0 that reaches every process on the machine rather than one user's.
3. Use the build-as-`deploy` / activate-from-closure recipe in `AGENTS.md`.
