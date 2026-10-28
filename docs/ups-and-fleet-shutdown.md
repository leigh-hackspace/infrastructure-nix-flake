# The UPS on `gw`, the notices it sends, and the power-off it drives

Written 2026-10-28, the day a **Back-UPS XS 1400U** appeared on the router's USB
port. What is deployed lives in `machines/gw/ups.nix` (NUT, the outage timer, the
notices, the target list) and `crates/ups-watchdog/` (the notice delivery and the
ordered power-off). The UPS is on the **router**, not on a service box, and that
one fact is why this is a policy module and a Rust program instead of
`power.ups.enable = true`: a router that dies hard takes the DNS, the leases, the
default route and the NFS path to the NAS away from boxes that are still drawing
current.

Nothing here has been through a real outage. §7 is the list of what that means.

---

## 1. What is attached, and why the kernel is not the answer

Read off `gw` on 2026-10-28:

```
usb 2-1.2   idVendor=051d idProduct=0002   American Power Conversion
Back-UPS XS 1400U  FW:926.T2 .I USB FW:T2   serial 4B1836P34201
hid-generic 0003:051D:0002.0005: hiddev97,hidraw4
```

`lsusb` sees it and the kernel has claimed it, which is as far as Linux goes:
this model is **HID-only** and there is **no in-kernel driver for APC's HID UPS
protocol**. `/sys/class/power_supply` is empty and always will be, and `upower`
has nothing to show. That is not a bug to fix — it is why NUT (`power.ups`, whose
module in this nixpkgs is `nixos/modules/services/monitoring/ups.nix`) has to own
`/dev/hidraw4`: `usbhid-ups` is the maintained implementation of APC's HIDP
dialect. Anything that "just reads the battery" without NUT means implementing
that protocol against a device that will not be unplugged for testing.

## 2. Who decides what

| Question | Owner | Why that owner |
| --- | --- | --- |
| is the UPS discharging? | NUT: `upsdrv` (`usbhid-ups`) → `upsd` (loopback :3493) → `upsmon` | protocol, and the notification stream |
| for how long before acting? | `upssched`, in the generated `/etc/nut/upssched.conf` | `AT ONBATT * START-TIMER … 300` + `AT ONLINE * CANCEL-TIMER …` is exactly the semantics wanted, in the one component that is restarted by nothing else |
| **tell everyone, and say how long** | `ups-notice.service` → `ups-watchdog broadcast` | see §3 |
| does the box still have mains *now*? | `ups-watchdog`'s re-check (advisory; `plan::decide`) | a lost `ONLINE` must not power off a building with mains; a broken `upsc` must not be a battery-to-zero |
| whom do we tell, in what order, over what? | `ups-watchdog run`, from `shutdownTargets` in `machines/gw/ups.nix` | NUT has never heard of the rest of the network |
| does the UPS drop its load? | NUT's `ups-killpower.service` at shutdown, armed by POWERDOWNFLAG | §5 |

**There is one countdown in this design and it is not in Rust.** A second timer
would be a second timer to disagree with, which is the same mistake `AGENTS.md`
rule 26 is about for tables. `upsmon`'s own `SHUTDOWNCMD` is pointed at the same
systemd unit the timer is, so *`upsmon` deciding, the timer expiring and a human
starting the unit* all arrive at one sequence: two ways to power off a router is
the two-DHCP-servers mistake wearing a different hat. The unit is
`RemainAfterExit`, so `upsmon`'s habit of re-running `SHUTDOWNCMD` every five
seconds is a no-op rather than a second pass over the fleet.

## 3. The notices, and why there are three of them

A power cut at 02:40 is unattended, but a power cut is also the only time anyone
at the hackspace wants to be told *before* something happens to their box. Three
messages, each sent to **every server over ssh `wall`, and to this box's own
terminals** (`--local-wall`), through one unit — `ups-notice.service` — whose body
is the file `ups-trigger` writes (`/run/ups-watchdog/notice`; line 1 is the
headline, the rest is the body):

| When | Headline | What it promises |
| --- | --- | --- |
| the first `ONBATT` | `POWER OUTAGE DETECTED` | the countdown ("servers power off in 300s unless mains return"), and that **nothing has been switched off yet** |
| `ONLINE` | `MAINS RESTORED` | the timer was cancelled, and the one case it cannot undo (a shutdown already under way) |
| `COMMBAD`/`TIMEDOUT` | `UPS UNREACHABLE` | the countdown was **cancelled** because the UPS cannot be read, and that this box may be on battery with no clock on it |
| the start of `ups-fleet-shutdown.service` | `FLEET SHUTDOWN BEGINNING` | what is about to happen, that it is this router doing it, and the window in which it can still be stopped |

Three decisions in that table are worth keeping when you edit it:

* **The outage notice names the number.** "On battery" is a status; "servers
  power off in 300 s unless mains return" is something a person can act on.
  `AGENTS.md`'s "a UI that refuses an edit must say so" applies to a terminal bell
  too, and `plan.rs` has a test that fails if the countdown stops being in the
  message.
* **The shutdown notice is sent *before* the first hop, and then the program
  waits** (`--notice-delay-seconds`, 30; `--late-notice-delay-seconds`, 5 once
  `LOWBATT`/`upsmon` have decided). That wait is the only intervention window in
  the sequence, so the message says what to type: `systemctl stop
  ups-fleet-shutdown` on `gw`. A wall that arrives after the poweroff command has
  been sent is a confession, not a warning.
* **"Cannot see the UPS" cancels the countdown rather than starting one.** Driven
  from `upssched` (`AT COMMBAD * CANCEL-TIMER`), not from Rust, so the cancellation
  and the countdown are configured two lines apart. A countdown to powering off a
  network, run off a UPS nobody is reading, is a worse failure than the hard-off it
  was preventing.

Delivery details that are decisions rather than incidental:

* **The text travels on the child's stdin, never inside the remote command
  line.** `wall` reads a stream; interpolating a message into a string a remote
  shell parses is a quoting bug that only appears during an outage.
  `--wall-command` is fixed text from the unit (`sudo -n wall -n`, because
  `wall` writes terminals it does not own and `common/base.nix` gives
  `leigh-admin` passwordless sudo).
* **Every recipient is tried; a missed one is not fatal.** A notice is not a
  permission slip. `broadcast` mode exits non-zero so `systemctl is-failed
  ups-notice` answers "was everybody told", and the journal names who was not —
  but `run` continues, because a box that cannot be told is still going to be told
  that it went down.
* **The recipients of the notice and the targets of the poweroff are one generated
  list** (`sharedArgs` in `machines/gw/ups.nix`), so a server cannot be powered
  off without being able to be warned, or warned without being in the sequence.

## 4. The sequence, end to end

```
mains lost → usbhid-ups sets OB → upsmon notifies ONBATT
  ├─ upssched: START-TIMER apc-outage 300
  └─ ups-trigger apc apc-observing
        └─ ups-notice.service   → wall every server + gw: "POWER OUTAGE DETECTED … 300s"
   …mains back within 300s…
        └─ AT ONLINE CANCEL-TIMER → ups-trigger apc apc-restored → "MAINS RESTORED", done
   …still on battery after 300s…
        └─ timer expires → ups-trigger apc apc-outage
              └─ ups-fleet-shutdown.service (RemainAfterExit)
                   1. re-check ups.status → OL? abort, exit 21, nothing powered off
                   2. broadcast "FLEET SHUTDOWN BEGINNING" (all servers + local wall)
                   3. wait 30s   ← the intervention window
                   4. ssh -i agent-hop-key leigh-admin@<each host>: sudo -n systemctl poweroff --no-block
                      services1 → services2 → aibox, 30s wall-clock each
                   5. wait 10s (their teardown, not ours)
                   6. write /run/killpower, then `systemctl poweroff` here
                        └─ shutdown → ups-killpower.service → `upsdrvctl shutdown`
```

`LOWBATT` and `upsmon`'s `SHUTDOWNCMD` enter at step 1 with a reason that says
"already late": no abort on `OL`, a 5 s window instead of 30 s, and no further
waiting. A human starting the unit arrives the same way, which is deliberate — a
person typing `systemctl start ups-fleet-shutdown` means it.

**gw is never in the ssh list**, and the program refuses it by *address*: the hop
key is authorized on every box including this one, so a hop to 10.3.1.1 from gw
succeeds (`AGENTS.md` rule 27), and "the router powers itself off first, over ssh,
while three boxes still need telling" is a failure that produces no error.
`selfAddresses` is `common/hosts.nix`'s `router` plus every VLAN's gateway
address, and `just ups-drill` prints what would be run.

The order is `services1` (postgres, the containers, the NAS's biggest NFS client),
then `services2`, then `aibox`, then the router. The **NAS is deliberately not in
the list** (§7), and `services1`'s mounts are automounts (`common/nas.nix`), so
services1 going down first is the ordering that does not hang anything.

## 5. Getting the network back without anyone touching a button

**Measured on first deploy (2026-10-28): `driver.flag.allow_killpower: 0`.** NUT 2.8
refuses to cut the UPS output unless the driver is explicitly given that flag, and
`machines/gw/ups.nix` does not give it — so today `ups-killpower.service` would run
`upsdrvctl shutdown` and the driver would decline the part that matters.  The
consequence is concrete: after an outage that outlasts the battery, `gw` powers off,
the outlets stay live, and **the network comes back when a person walks up to it.**
That is a decision rather than an oversight — upstream added the flag because
killpower is the one command in this path that can leave a building offline past its
own recovery — but it does mean the rest of this section describes a mechanism that
is not currently switched on.

What the mechanism is for, if the flag is ever enabled deliberately: the sequence
ends by writing NUT's POWERDOWNFLAG (`/run/killpower`, one path shared by
`upsmon.conf`, `ups-killpower.service` and `--killpower-flag`), and during shutdown
NUT runs `upsdrvctl shutdown`, which tells the UPS to drop its output and **return
it when mains come back**. The box then powers on by itself if the BIOS does what
"Restore on AC Power Loss → Power On" says it does, and the network rebuilds itself
in the order `docs/network-upgrade-runbook.md` describes (NAS, then services1, then
the rest). With the flag off neither half happens: no AC cycle, no power-on event.

Two consequences worth keeping straight either way:

* A short outage never reaches step 6 at all, so the network is not "restored" by
  anything here — it simply never went down.
* Whether this firmware implements `shutdown.return` at all is still unmeasured, and
  it sits *behind* the flag: measuring it and deciding on the flag are one exercise
  (§7.1).

## 6. Proving it without cutting the cable

| Prove | Command | What it does **not** prove |
| --- | --- | --- |
| NUT can read the UPS | `just ups-status` (exit 0/10/20) or `sudo upsc apc@localhost` | that the battery holds charge, or that `OB` ever arrives |
| the notice reaches every server | `just ups-notice-test` | anything about the shutdown path — it runs `broadcast`, not `run` |
| the shutdown list and argv | `just ups-drill` | that ssh works from `gw` (dry-run skips the hops *and* the sleeps) |
| the policy as deployed | `just ups-events` | that NUT accepted `upssched.conf` (read `journalctl -u upsmon`) |
| the whole chain | `just ups-drill-notify` (sends a real `ONBATT` through `upssched`, then `ONLINE` to cancel it) | this is as close to a live drill as gets without unplugging: it starts the real 300 s timer and the real notice to every server, so it **must** be followed by the cancel — the recipe does both, back to back |
| is a timer armed right now | `pgrep -a upssched` | that the timer is the one you think; `upssched` self-daemonises and **exits when its queue empties**, so "no process" is the healthy idle state and a running one means something is counting down |

**`upssched`'s contract, measured (NUT 2.8.4) rather than read:** it calls `CMDSCRIPT`
with **one** argument — the `EXECUTE` name, or the *timer* name on expiry — and sets
`$UPSNAME` only for the `EXECUTE` path, **empty when a timer fires**. The first
draft of `machines/gw/ups.nix` passed `<upsname> <event>` because the man page reads
that way, so every event fell through to "unhandled", logged a warning and exited 0:
upsmon, upssched and systemd all recorded a success and nothing worked. That is why
the event names come from one table, why an unmatched event is now `err` and
nonzero, and why the throwaway-config proof below is the recipe to re-run if a
future NUT changes it:

```bash
# Nothing here touches the fleet: a private CMDSCRIPT, pipe and lock, a 5 s timer.
mkdir -p /tmp/prove && printf '%s\n' '#!/run/current-system/sw/bin/bash' \
  '{ echo "argc=$# arg1=[$1] arg2=[$2] UPSNAME=[$UPSNAME]"; } >> /tmp/prove/log' > /tmp/prove/cmd.sh
chmod 755 /tmp/prove/cmd.sh && printf '%s\n' 'CMDSCRIPT /tmp/prove/cmd.sh' 'PIPEFN /tmp/prove/pipe' \
  'LOCKFN /tmp/prove/lock' 'AT ONBATT * START-TIMER prove-timer 5' 'AT ONBATT * EXECUTE prove-immediate' > /tmp/prove/upssched.conf
sudo env NUT_CONFPATH=/tmp/prove NUT_STATEPATH=/tmp/prove UPSNAME=proveups NOTIFYTYPE=ONBATT upssched
sleep 8; cat /tmp/prove/log; sudo pkill -x upssched; rm -rf /tmp/prove
```

`NUT_CONFPATH` is not optional in any manual `upssched` call: it is how the NixOS
units tell NUT where its config is, and without it `upssched` looks inside the
package's own `etc/` and fails with `Can't open …/etc/upssched.conf` — which is what
the first "drill" on this box actually reported, and it looked like a dead design
rather than a missing environment variable.

Two things the drill cannot tell you, and both bite: ssh from `gw` to each target
with the hop key (a host-key rotation means `StrictHostKeyChecking=yes` refuses
the hop **on purpose** — regenerate `common/ssh-known-hosts.nix` with
`just known-hosts` after reinstalling a box), and whether `wall` is readable to
whoever is logged in (`wall` is root's tool; `sudo -n wall -n` is the remote verb
for a reason).

## 7. Deployed 2026-10-28: what was proven, and what is still a guess

Proven by activating it and reading the box:

* **`usbhid-ups` reads this UPS, and `upsc` needs no credentials.** The driver came
  up as `APC HID 0.100` with the right model and serial; `ups.status` read `OL` a
  few seconds after `WAIT`; `upsd` is listening on `127.0.0.1:3493` and `::1` only
  (`ss -ltnp`), as the assertions in `machines/gw/ups.nix` insist. The re-check
  being *advisory* is still the design — "`upsc` failed" is not "the mains are
  fine".
* **Nothing is armed by a deploy.** `ups-fleet-shutdown` and `ups-notice` are
  `inactive`, and the drill unit proves the argv without a hop (`--dry-run` skips
  the hops *and* the sleeps).
* **Three bugs the tests did not catch**, all found by running the deployed thing:
  the drill unit exited 21 because `--dry-run` still obeyed the mains re-check (so
  the check a person uses to trust the sequence could never run — now `plan::proceed`,
  tested); a notice with no remote targets printed the *shutdown's* "the rest of the
  network runs on to a dead switch" sentence; and `Documentation=` wanted a URL, not
  a repo path, so systemd logged `Invalid URL` on every start.
* **Three more, in the shell half**, found by sending a real `ONBATT` (§6): the
  dispatcher read its event from `$2` because the man page reads as two arguments
  and `upssched` passes one, so **every event fell through to "unhandled"** while
  upsmon, upssched and systemd each logged success; `printf '%s\\n'` inside an
  indented Nix string writes a literal backslash-n, so the reason file would have
  read as "unknown" (a `LOWBATT` losing its "already late" handling) and the
  broadcast text would have ended in `\n`; and `DATADIR` is not an `upssched`
  directive at all, so the generated config printed `invalid directive` and
  `Encountered 1 config errors` on the power-cut path. The event names are now one
  table with an `assertions` entry comparing the generated conf against the
  generated dispatcher, and an unmatched event is `err` + nonzero rather than a
  warning + 0.
* **`upssched` self-daemonises and exits when its timer queue empties**, so `pgrep
  upssched` is the way to ask "is anything counting down?" and the absence of a
  process is healthy idle, not a missing service. There is no `upssched.service`
  in this repo and none is wanted: `NOTIFYCMD` starts it on demand. Its own
  bookkeeping (`Timer daemon started`, `Cancelling timer: …`) goes to syslog
  facility `daemon`, which this box does not collect — so the *journal* is not
  evidence that a timer was armed or cancelled, and `just ups-events` asks the
  process table instead.
* **The notice path works, over ssh, twice in one boot.** A real `ONBATT` produced
  `POWER OUTAGE DETECTED` on gw's terminals and on services1, services2 and aibox
  (`gw: notified`, `services1: notified`, …, unit exit 0), and the following
  `ONLINE` produced `MAINS RESTORED` the same way — which also proves
  `ups-notice.service` is not `RemainAfterExit` and so fires on every event rather
  than once per boot. What it does not prove is that a human was there to read one, and it
  has one known gap: the dispatcher uses `systemctl start`, so an event arriving
  *while* a notice is being delivered is a no-op (the job is already queued) and that
  message is lost. Events are seconds to minutes apart in practice and a notice takes
  under a second per recipient; `restart` would be worse, because it kills the hops
  already in flight.
* `systemctl mask` **does not work as an emergency brake on NixOS**: the unit file
  is a symlink into the store and `mask` refuses it (`already exists and is a
  symlink`), so the safety net for a drill is `--dry-run`/`--no-poweroff` in the
  drill unit, and for a live sequence it is `systemctl stop ups-fleet-shutdown` —
  which is what the wall message says, and why the wall message says that.
* **Three eval errors the router's own gate cannot see**, because `just check-eval`
  evaluates no machine config: two `types.str` options (`power.ups.schedulerRules`,
  `power.ups.users.<x>.passwordFile`) handed derivations, and one integer
  interpolated into a string. `nixos-rebuild build --flake .#gw` is the eval gate on
  this box, so it is not optional — see `docs/nix-dev-workflow.md`'s ordering of
  build/switch.

Still not verified, and none of it should be trusted until it is:

1. **Whether the UPS can be told to drop its load at all.** `upsc` reports
   `driver.flag.allow_killpower: 0`, so NUT 2.8 declines to send the killpower
   command regardless of firmware support (§5): **today, after an outage that
   outlasts the battery, the network stays down until someone walks to `gw`.**
   Enabling the flag is a deliberate decision — it means the sequence really does
   cut the power — and measuring `shutdown.return` sits behind it. **Do not** probe
   with instant commands on a UPS carrying the router; the test is one controlled
   discharge with a sacrificial box on it, watching `journalctl -u ups-killpower`.
2. **The BIOS.** "Restore on AC Power Loss → Power On" is unreadable from the
   running system, moot while the flag is off, and half of the recovery story once
   it is on.
3. **The runtime arithmetic is a manufacturer's number, not a measurement.**
   `battery.runtime: 860` at `battery.charge: 100` — about fourteen minutes — with
   `battery.runtime.low: 120` and `battery.charge.low: 10`, so `LOWBATT` arrives
   with roughly two minutes of runway. The 300 s grace is about half the claimed
   runtime, which is defensible; but `battery.mfr.date` is **2018-09-08** and
   `battery.date` is the nonsense `2001-09-25`, so this is an eight-year-old pack at
   a load we have never measured. Re-measure under discharge, and probably replace
   the pack, before treating 860 s as real.
4. **The NAS (`10.3.1.6`) and Proxmox (`monster1`) are not in the list and are not
   known to be on this UPS.** TrueNAS has its own shutdown verb, and monster1 hosts
   VMs this flake does not own; `docs/monster-powercut-runbook.md` is the runbook for
   that box being down, not a plan. Until it is settled, this feature powers off the
   *flake machines* and the router can still go while the NAS is serving NFS.
5. **Nobody has watched a real `ONBATT`** — closed, in part: `just ups-drill-notify`
   has now done it (§6), and the notice, the timer, the cancel and the dispatcher
   all behaved. Still open: a *real* `OB`/`LOWBATT` from the driver rather than a
   notification typed by hand, and a human confirming they saw the wall.
6. **The shutdown half has still never run.** The drill unit prints the ordered argv
   and exits 0; no box has ever been powered off by this code, and the 30 s notice
   window, the 20 s per-hop limit and the 10 s settle are judgement against that
   measured 860 s of claimed runtime. The one command that would prove it for real
   is a controlled discharge with a sacrificial machine on the UPS (§7.1).

## 8. Read also

* `machines/gw/ups.nix` — the deployed policy, and the comments on each choice.
* `crates/ups-watchdog/README.md` — the CLI's trap list and exit codes.
* `docs/nix-dev-workflow.md` — building on the router: you don't; `just switch`
  here evaluates on `gw` and compiles on aibox.
* `docs/router-and-dns.md`, `machines/gw/README.md` — what the router must be seen
  to do, which is what an outage takes away.
* `docs/monster-powercut-runbook.md` — the other power-cut runbook, for the box
  that is not flake-managed.
* `common/nas.nix` — why services1 survives the NAS going away slowly, and not at
  all.
