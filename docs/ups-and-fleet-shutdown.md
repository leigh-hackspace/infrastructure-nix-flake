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

Powering `gw` off with the UPS still energising its outlets leaves a router that
is *off* and a building with no DNS until someone walks to it. So step 6 writes
NUT's POWERDOWNFLAG (`/run/killpower`, one path shared by `upsmon.conf`,
`ups-killpower.service` and `--killpower-flag`), and during shutdown NUT runs
`upsdrvctl shutdown`, which tells the UPS to drop its output and **return it when
mains come back**. The box then powers on by itself if the BIOS does what
"Restore on AC Power Loss → Power On" says it does, and the network rebuilds
itself in the order `docs/network-upgrade-runbook.md` describes (NAS, then
services1, then the rest).

Two consequences people are surprised by, both stated in
`machines/gw/ups.nix`'s header:

* If `shutdown.return` is not supported by this firmware, the outlets drop and
  **stay** dropped until mains return *and* the UPS's own return delay elapses —
  or, in the worst case, until someone presses the front button. That is
  unverified (§7) and it is the single most important thing to measure.
* A short outage never reaches step 6 at all, so the network is not "restored" by
  anything here — it simply never went down.

## 6. Proving it without cutting the cable

| Prove | Command | What it does **not** prove |
| --- | --- | --- |
| NUT can read the UPS | `just ups-status` (exit 0/10/20) or `sudo upsc apc@localhost` | that the battery holds charge, or that `OB` ever arrives |
| the notice reaches every server | `just ups-notice-test` | anything about the shutdown path — it runs `broadcast`, not `run` |
| the shutdown list and argv | `just ups-drill` | that ssh works from `gw` (dry-run skips the hops *and* the sleeps) |
| the policy as deployed | `just ups-events` | that NUT accepted `upssched.conf` (read `journalctl -u upsmon`) |
| the whole chain | `sudo upssched apc ONBATT` (the NOTIFYCMD contract: `<upsname> <event>`) | — this is as close to a live drill as gets without unplugging; it starts the real 300 s timer and the real notice, so follow it with `sudo upssched apc ONLINE` |

Two things the drill cannot tell you, and both bite: ssh from `gw` to each target
with the hop key (a host-key rotation means `StrictHostKeyChecking=yes` refuses
the hop **on purpose** — regenerate `common/ssh-known-hosts.nix` with
`just known-hosts` after reinstalling a box), and whether `wall` is readable to
whoever is logged in (`wall` is root's tool; `sudo -n wall -n` is the remote verb
for a reason).

## 7. What is not verified, and what must not be trusted yet

1. **`upsdrvctl shutdown` on this firmware.** Whether the XS 1400U implements
   `shutdown.return`/`load.off` at all is unmeasured — and the module deliberately
   does **not** send instant commands. Measure with `upscmd -l apc@localhost` and
   the driver's own log line during a controlled discharge. Until then §5's
   recovery story is a hypothesis.
2. **The BIOS.** If "Restore on AC Power Loss" is not Power On, this box does not
   come back by itself even when the UPS does the right thing. Unreadable from the
   running system.
3. **`upsc`'s anonymous read on NUT 2.8.** `upsd` ships no `ACCEPT` line (the
   nixpkgs module writes `LISTEN`s only), and the re-check is advisory *because*
   that has not been proven here: if it turns out to need credentials, the failure
   mode is "always acts on the timer's authority", which is the behaviour the
   original ask described.
4. **Runtime arithmetic.** The XS 1400U is 1400 VA / 700 W; the router is tens of
   watts, which is a *guess about the load*, not a measurement — and the real
   ceiling is battery age (serial `4B1836P34201`, a 926.x firmware unit of unknown
   service history). `upsc apc@localhost battery.runtime` after `just ups-status`
   is the number to write into the grace period, and the grace period should be
   `runtime − (the sequence, ~2 min, plus margin)`, not a round number.
5. **The NAS (`10.3.1.6`) and Proxmox (`monster1`) are not in the list and are not
   known to be on this UPS.** TrueNAS has its own shutdown verb, and monster1
   hosts VMs this flake does not own; both need their own answer and an item in
   `docs/follow-ups.md`. Until then this feature powers off the *flake machines*.
6. **Nobody has watched a real `ONBATT`.** Every second value here — 300 s of
   grace, 30 s of notice window, 20 s per-hop limit — is engineering judgement,
   not measurement.

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
