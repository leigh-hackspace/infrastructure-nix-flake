# The ProCurve switches — access, RADIUS, and the two firmwares

Moved out of `AGENTS.md` on 2026-10-14. `AGENTS.md` keeps the rules: Telnet only,
`agent` is a **Manager** (not read-only), nothing writes NVRAM unless asked,
`Can't reach RADIUS server.` is a note rather than a failure, and `show
running-config` output is never pasted anywhere it will be kept. This file is the
device-level background and the RADIUS root cause. The CLI's traps, the parser
design and the write path live in `crates/switch-gui/README.md`; the GUI itself is
`docs/services.md`.

## The three devices

| Name | Model | Firmware | Address |
| --- | --- | --- | --- |
| `switch1` | J9280A 2510G-48 | `Y.11.44` | 10.3.1.2 |
| `switch2` | J9280A 2510G-48 | `Y.11.44` | 10.3.1.3 |
| `switch3` | J4903A 2824 | `I.10.107` | 10.3.1.4 (Workshop) |

All three are in `common/hosts.nix` under those names, which are also each
device's own `System Name` and the names the GUI's URLs use (`/switch/switch2`).
**Managed over Telnet only** — no API, and the web UI needs a Manager login.
`crates/switch-gui/` wraps them in a browser GUI.

`switch3` answers slowly: a login there can take **15–25 s** because RADIUS is
tried first and the retry ladder is ~5 s per attempt, so the login reads use a
separate timeout (`SWITCH_GUI_LOGIN_TIMEOUT`, default 45 s) rather than the 20 s
command timeout — at 20 s the read times out before the prompt arrives. Its login
also needs the `Press any key to continue` gate answered before the login name is
typed, or the first character is eaten (`agent` arrives as `gent`) and RADIUS
refuses it.

The gatus "Network Switches" pings use the **addresses** from `common/hosts.nix`:
`switch1.int` / `switch2.int` / `switch3.int` are **not** dnsmasq records. If the
names are ever wanted, add them to the router's host overrides
(`docs/router-and-dns.md`); until then do not expect them to resolve.

## Privilege: `enable` is a second login, and writes need config mode

`agent` is a **Manager**, not read-only. `enable` on this firmware re-asks for a
login name, not just a password, and the same `agent` credentials are accepted —
the prompt goes `switch1>` → `switch1#`. So the GUI defaults its enable credentials
to the login credentials (`--read-only` is the switch that asks for Manager not at
all), and the *read* needs Manager too: without it `show running-config` is
refused and the port names vanish.

Writes run **inside config mode**: at a plain Manager Exec prompt every
configuration command answers `Invalid input`, so `Switch::apply` sends `config`
first, and `leave_config` exits until the prompt stops naming a context (one `exit`
from `switch1(eth-23)#` only reaches `(config)#`).

**Nothing saves to NVRAM, so a reboot undoes any change.** Keep it that way unless
asked. `write` is this firmware's command — `wr mem` / `write memory` is the *other*
firmware's spelling, and `write ?` is already `Invalid input` here.

## Two firmwares, one parser

The 2824 is not the same firmware and three things bite a client written against
the 2510G: `show vlan` has an extra `Voice` column (so the number of trailing
columns must come from the header), `show spanning-tree` puts
`Cost/Priority/State` in the same `|`-block as the port, and its banner ends with
`Press any key to continue`. All three are caught by fixtures taken from the live
devices — see `crates/switch-gui/README.md` ("Two firmwares, one parser").

## "Can't reach RADIUS server." means the login was refused

**It is this firmware's generic "authentication failed" message**, printed for a
wrong password as well as for a RADIUS server that never answered, and it appears
on a *successful* login too. Root-caused 2026-10-12 (an earlier note blaming
EAP/MS-CHAP attribute mapping was wrong):

- `ak-outpost-radius-outpost` logs `Access-Request` with `ip: 10.3.1.2`, so the
  switch reaches authentik.
- authentik then writes `login_failed` events with `client_ip: 10.3.1.2`,
  `username: agent`, rejected by the **identification stage** — its plain "bad
  credentials" outcome. authentik really does have an active `agent` user, but its
  password is not the switch's local `agent` password.
- RADIUS always refuses, the switch **falls back to its own local account
  database**, lets you in, and prints the misleading message on the way. That is
  why it "happens from time to time": it happens on every login, and only shows
  when you are watching.
- Timing corroborates it: a wrong password produces the message in under two
  seconds, where a real network timeout costs ~5 s per retry.

**Confirmed from the device itself**, in the Manager context — `show
running-config` reads:

```text
aaa authentication telnet login  radius local
aaa authentication telnet enable radius local
radius-server host 10.3.1.36
```

so both the login *and* the `enable` second login go RADIUS-then-local, which means
every GUI refresh (which logs in and enables) logs **two** failures in authentik.
Until this is fixed, `login_failed` alerting in authentik is permanently noisy
because of this, and a `login_failed` for `agent` from `10.3.1.2` is not an
incident. `show aaa` and `show radius` are **not commands on this firmware** — the
config is only in `show running-config`.

**The fix** is to set the authentik `agent` password to match the switch's local
one, or drop RADIUS from that account's auth order. Separately, `RadiusProvider`
pk 7 (application `Switches`) has `authentication_flow = None` and the flow
actually used is `radius-authentication-flow` (resolved from the outpost, not the
provider) — not the cause here, but worth tidying. Debugging procedure:
`docs/authentik-oidc-notes.md` ("Debugging RADIUS").

`switch-gui` treats the message as a **note, not a failure, when a prompt follows
it** (`src/switch.rs::check_auth`): the session is usable, and refusing the connect
on it would make the GUI unusable on a switch that is answering normally — which is
what happened on `switch3` before it was a note. The note goes into the snapshot's
warnings so the page shows it.

## ⚠️ The RADIUS shared secret was leaked into a session transcript (2026-10-12)

Reading `show running-config` to root-cause the login noise, a masking filter that
expected `key:` missed the actual `radius-server key <secret>` form and the secret
went into a session transcript. **Rotate it on both ends** — the switch
(`radius-server key`) and the authentik RADIUS outpost. The same command also
exposes the local user hashes.

The standing rule follows from it: `show running-config` is the only read whose
output carries secrets, so it goes through `Switch::secret_command` (failures never
quote its output) and `parse::port_names` returns names and nothing else. Do not
run it by hand and paste the result anywhere you will keep.

## Still unverified

The **VLAN-management** commands (create, rename, delete, and the per-VLAN
`tagged` endpoint) have never been sent at all. See `crates/switch-gui/README.md`.
