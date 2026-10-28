# `ups-watchdog` — the fleet half of a power cut on `gw`

One binary, four modes, one job NUT cannot do: **`gw` is the router, so it must
not be the last machine to shut down — and nobody may find out about the outage
from the fact that their box went away.**

The policy (what is attached, the grace period, who is powered off in what order,
what each notice says) is `machines/gw/ups.nix`; the reasoning, the battery maths
and the list of what has never been tested on a live battery is
[`docs/ups-and-fleet-shutdown.md`](../../docs/ups-and-fleet-shutdown.md). This
file is the trap list for the CLI.

```
ups-watchdog <run|plan|status|broadcast> [flags]     # flags: --help
```

| Mode | Used by | Does |
| --- | --- | --- |
| `run` | `ups-fleet-shutdown.service`, started by `upssched`'s CMDSCRIPT, by `upsmon`'s `SHUTDOWNCMD`, or by a person | re-check the UPS, broadcast the shutdown notice, wait, ssh-poweroff each target, settle, arm NUT's killpower, power off this box |
| `broadcast` | `ups-notice.service`, started for the three notices that are not a shutdown (`on battery`, `mains restored`, `UPS unreachable`) | compose one message and `wall` it to every target and to this box's terminals |
| `plan` | a human, before a deploy | print the order and the exact argv; run nothing, touch nothing. `just ups-drill` is the same review through the deployed unit |
| `status` | `just ups-status` | one `upsc` read, plus the parsed form |

Exit codes are part of the interface, because the only record a oneshot leaves in
a building that has just lost power is `systemctl show -p ExecMainStatus`:
`run` 0 done / **21 aborted because the UPS reports mains** / 2 bad flags;
`status` 0 discharging / 10 mains fine / 20 cannot tell;
`broadcast` 0 everyone reached / **30 somebody was missed**.

## Files, by job

| File | Owns |
| --- | --- |
| `cli.rs` | argv and its defaults — the flag list `machines/gw/ups.nix` passes, and the refusals that belong at parse time (no mode, no identity file, a `broadcast` with no recipient, a `run` that would power off without arming the killpower flag). Knows nothing about power or ssh |
| `main.rs` | the four modes and the step order of `run` |
| `plan.rs` | the deciding, with no side effects: `ups.status` parsing, `decide()` (status × reason → act/abort), `proceed()` (same decision × is-this-a-drill), the target list and its refusals, the ssh argv, the broadcast text. **No sleeps, no `Command::new`, no clock** — that is what makes it the place to argue about behaviour |
| `exec.rs` | running programs under a deadline, and the one `upsc` read |
| `notice.rs` | composing a notice and delivering it over `wall` |

## Traps

* **`--dry-run` does not obey the abort arm; it reports it.** `plan::proceed` is
  `decide()` × "is this a drill", and the split exists because the drill unit is
  started while the mains are fine: a dry run that honoured "mains are back, abort"
  exited 21 every time and showed nobody the plan. The 37 tests that passed before
  the deploy did not catch this, because the bug was in which arm a *mode* obeys —
  so the behaviour is now `plan::tests::a_drill_reports_an_abort_instead_of_obeying_it`
  and `just ups-drill` is the unit test that cannot lie.
* **A self-address list is a list of addresses, not of names.** `gw` is refused from
  the ssh targets by *address* (`--self-address`, built from `common/hosts.nix` and
  every VLAN gateway this box answers on) and by *name* (`gw`, `gw.*`) — because the
  hop key is authorised on every box including this one, a target that resolves to
  localhost succeeds and powers this box off mid-sequence. See rule 27's self-hop
  note in `AGENTS.md` before changing either half.

* **There is no timer in here, on purpose.** "Discharging for N seconds, cancelled
  by `ONLINE`" is `upssched`'s idiom and it is configured in `machines/gw/ups.nix`.
  A second implementation would be a second countdown to disagree with.
* **The re-check is advisory and asymmetric.** `OL` during the grace timer → abort
  (a lost `ONLINE` must not take the building down); `OL` when the reason is
  `LOWBATT`/`upsmon` → act anyway; **no reading at all → act on the timer's
  authority**, because `upssched` already cancels the timer on `COMMBAD`/`TIMEDOUT`
  and a broken `upsc` must not be the reason a router rides its battery to zero.
  `plan::decide` is the function; it is tested in both directions.
* **`gw` is refused by *address*, not by name.** `common/hosts.nix` calls the
  router `router` and its hostname is `gw`, and every VLAN gateway address is also
  this box — so `--self-address` is a list, and a target answering any of them is
  an error. The reason is `AGENTS.md` rule 27: the hop key is authorized on every
  box *including this one*, so a self-hop exits 0 and looks like success.
* **A bad row does not stop the sequence.** A refused/typo'd `--host` is printed as
  a finding and the remaining targets are still told; an *empty* target list is
  printed loudly and this box still powers itself off. Partial coverage beats
  dying mid-write, and the journal is the record.
* **ssh's 255 means two different things and both are handled deliberately.** In
  `run` it is counted *uncertain* (a box going down under the hop closes the
  connection too); in `broadcast` it is a miss (nothing was supposed to close it).
  Do not "unify" those.
* **Every wait is bounded in the program, not by systemd.** `TimeoutStartSec`
  kills the *unit*, and the unit's last step is the local poweroff: a hung hop
  that eats the timeout costs the box its clean shutdown. Hence
  `exec::run_bounded` and no `Command::status()` anywhere.
* **The notice text travels on stdin.** `wall` reads a stream, and a message
  interpolated into a remote command line is a quoting bug that only appears
  during an outage. `--wall-command` is fixed text from the unit.
* **The message that gets you out of this is the wait, not the wall.** `run`
  broadcasts and *then* sleeps `--notice-delay-seconds` (30; 5 when the reason is
  already late), because the only intervention that exists is
  `systemctl stop ups-fleet-shutdown` typed on `gw` after someone reads it.
* **Nothing writes the UPS's load.** The only power command it receives is NUT's
  own `upsdrvctl shutdown` at shutdown, armed by the POWERDOWNFLAG this program
  writes. No `upscmd`, no instant commands — `AGENTS.md` rule 16's shape of
  reasoning applies to anything with a power switch on the end of it.
* **No dependencies** (`Cargo.toml` says why): every program this runs is a store
  path the unit puts on `PATH`, and a crates.io graph is one more thing standing
  between "the power failed" and "the fleet is powered off".

## Testing

`just test` runs the workspace (`cargo test --workspace` from `crates/` on a box
that is allowed to build — **not `gw`**, rule 22). The pure half has the tests that
matter for behaviour: the status grammar including `--`/empty as *unknown*, the
act/abort matrix, the self-hop refusal, the broadcast text naming the countdown,
and the wall-clock limits. There is no test that requires a UPS, and writing one
that does is how this file gets a real outage before its first deploy.

For the live path: `just ups-status` → `just ups-notice-test` → `just ups-drill` →
`just ups-events`, in §6 of the doc; the closest thing to a real drill is
`sudo upssched apc ONBATT` (the NOTIFYCMD contract, `<upsname> <event>`), which
starts the actual timer and the actual notice and must be followed by the matching
`sudo upssched apc ONLINE`.
