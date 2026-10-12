# switch-gui

Ports and VLANs on the network's ProCurve core switch, in a browser.

The switch is an **HP/HPE ProCurve J9280A Switch 2510G-48**, software revision
`Y.11.44`, at `10.3.1.2` (`common/hosts.nix` → `switch`). It is managed over
**Telnet**; it has no API, and its own web UI needs a Manager login and still
will not answer "which VLAN is port 17 on" in one screen. This tool reads the
configuration with `show` commands, merges the tables, and serves one page at
`https://switch.int.leighhack.org`.

Deployment: `machines/services1/services/switch-gui.nix` (services1 proxies it;
the binary listens on `127.0.0.1:8097`).

## What it shows

One row per port and per trunk group, merged out of four commands, because no
single one of them has the whole picture:

| command                | what only it knows                                            |
| ---------------------- | ------------------------------------------------------------- |
| `show vlan`            | the VLAN list, including names with spaces (`AD Lab`)          |
| `show vlan <id>`       | port membership (`Untagged`/`Tagged`) and `Up`/`Down` per member |
| `show spanning-tree`   | every port's STP state (`Forwarding`/`Disabled`/…) and media    |
| `show trunks`          | port **names** (`NAS2 Port 1`) and trunk group/type             |
| `show interfaces`      | per-port byte/frame/error counters                              |

`show port name` and `show ports` do **not** exist on this firmware at all (as
Manager, `show port name` answers `Ambiguous input: port`), so `show trunks` is
the only source of a port name — and only for ports in a trunk.

## Signing in

authentik, restricted to the `Infra` group. Not the switch's own login: the
switch authenticates against RADIUS, so a password here would be a second,
weaker door to the same device. The client contract (S256 PKCE, unpadded
challenge, `groups` claim) is in `src/oidc.rs` and the three authentik traps are
in `docs/authentik-oidc-notes.md`.

Writes additionally need the switch's **Manager** context, which the `agent`
account does not have — see "Privilege" below.

## Running it locally

```bash
SWITCH_GUI_LOGIN=agent SWITCH_GUI_PASSWORD='…' \
  ./target/release/switch-gui --bind 127.0.0.1:8097 --dev-user you
```

`--dev-user`/`--no-auth` skip sign-in and are **refused on a non-loopback bind**
(the same rule filestore's `--no-auth` follows). With no OIDC client configured
and a loopback bind, `--dev-user` alone is enough.

Tests: `cargo test` — 84 of them, no network, no switch. The parser fixtures are
captured output from the real device.

## The switch's CLI, the parts that will bite you

Everything below was established against `10.3.1.2`, not read out of a manual.
The code comments point back here.

**Lines end in LF, never CR LF.** The CLI counts a CR *and* an LF as a keypress,
so `agent\r\n` submits the username and then submits an **empty password**: you
get `User authentication failure.` and conclude the credentials are wrong.
`telnet::send_line` sends a bare LF.

**The banner swallows input.** Anything typed before the CLI prints
`Please Enter Login Name:` is discarded — and then the *password* you sent next
is read as the login name. So `switch::Switch::connect` waits for each prompt
rather than typing on a timer.

**It pages, and ignores `NAWS`.** Long tables stop with

    -- MORE --, next page: Space, next line: Enter, quit: Control-C

Advertising a 100-row window changes nothing; the only fix is to answer the
marker with a space. `Switch::command` does exactly that, and deletes the marker
*without* the rest of the line, because the interrupted row continues after it.

**It echoes**, prefixed with the prompt and terminated by `ESC E`, so the first
line of every response is the command you sent (`strip_echo`).

**It paints the screen.** Real output arrives wrapped in ANSI clears and cursor
moves (`ESC [ 2 J`, `ESC [ 2 4 ; 1 H`, `ESC [ ? 2 5 l`), which is why almost
every parse starts with `strip_ansi`.

**Trunk members have no rows of their own.** `show spanning-tree` lists `Trk1`
and `Trk3` (with an empty Type column) and never lists ports 5, 6, 47 or 48; the
VLAN tables likewise show the group, not the members. So a member port inherits
its group's state — and the page says so in a tooltip rather than implying the
port was measured (`switch::inherit_trunk_states`).

**Numbers are truncated with `...`** when a column is too narrow
(`2,993,652...`), so counters are kept as strings. A port's real total is not
recoverable from this view.

## Privilege: `enable`, and why writes need config mode

The switch has two CLI contexts and the prompt character tells you which:
`switch1>` is Operator (read-only), `switch1#` is Manager.

**`enable` is not a password prompt — it re-asks for a login name and then a
password**, i.e. it is a second login. The `agent` account answers that second
login with its own credentials and reaches the Manager context, so the app
defaults `SWITCH_GUI_ENABLE_LOGIN`/`_PASSWORD` to the login account rather than
naming a separate one. `--read-only` asks for Manager not at all, so the session
cannot write even though the account could.

**Configuration only works inside config mode.** At the plain Manager Exec
prompt `port 1 name x` answers `Invalid input: port`; `config` (or
`configure terminal`) takes you to `switch1(config)#`, and `exit` comes back.
`Switch::apply` therefore sends `config`, the commands, then `exit` — always the
`exit`, including after a refusal, because a cached console left in `(config)#`
would make the next *read* look like a parser bug.

Nothing here writes NVRAM (no `write`, no `copy running-config startup-config`),
so **every change is in RAM and a reboot undoes it**. Say that out loud before
trusting one.

## The write path, and why it is off

**The deployed service runs `--read-only`.** Reads, the Manager context and
config mode all work; no write has been made to work, so the buttons are off
rather than offered on a promise.

What is proven, against the live switch:

- `enable` with the login account reaches `switch1#` (`privilege: Manager`).
- `config` is sent, echoed and accepted — the console answers `switch1(config)#`.
- The **next** command is never echoed. `interface 5 name "NAS2 Port 1"` goes
  out and the read times out with a buffer containing nothing but the CLI's idle
  repaint and the cursor parked at column 18, i.e. an empty input line. No echo
  means the parser never saw it.
- The same bytes typed by hand over `nc` apply cleanly, at 0.15 s and at 2 s
  after `config`.

Ruled out, each by a deploy and a live attempt: the command syntax (the CLI's
own `?` at `switch1(eth-5)#` confirms `name`/`enable`/`disable`, and the one-shot
`interface 5 name "…"` form works manually); entering interface context instead
of using the one-shot form; a settle delay before the command (400 ms); resending
when no echo comes back; two Manager sessions contending (tested with the
service stopped); and the prompt matcher, which accepts `switch1(config)#`.

What is left, and what would settle it: the difference between this app's
session and `nc` is **telnet option negotiation** — the app answers IAC, `nc`
sends none. Capture the app's TCP stream (`tcpdump -i any host 10.3.1.2 and
port 23 -w /tmp/w.pcap` while a write is attempted) and compare it with the same
exchange from `nc`: if the bytes never leave services1, the client is the
problem; if they arrive and are unanswered, the switch's console is deciding
something about this session. Until then, treat every write command in
`src/commands.rs` as unverified, including the VLAN ones, which have never been
sent at all.

Anything the switch does refuse comes back as its own `Invalid input:` text,
surfaced verbatim — that part is tested, and a refusal is no longer reported as
a success.

## "Can't reach RADIUS server"

The switch authenticates against RADIUS (authentik), and this error is **not**
what it says: it is the switch's generic *authentication failed* message, and it
is printed for a wrong password as well as for a RADIUS server that never
answered. Measured on this device:

| attempt                              | switch says                                     |
| ------------------------------------ | ----------------------------------------------- |
| valid user, correct password         | `switch1>`                                      |
| valid user, **wrong** password       | `Can't reach RADIUS server.` + `User authentication failure.` |
| empty password                       | `User authentication failure.` only              |
| nonexistent user, wrong password     | `Can't reach RADIUS server.` then `User authentication failure.` at ~10 s |

The first message arrives in **under two seconds**, so it is not a network
timeout (the switch's own retry ladder would take ~5 s per attempt).

## Why it happens (root-caused on the authentik side, 2026-10-12)

The switch really does reach authentik, and authentik really does refuse the
login — because the password does not match. `ak-outpost-radius-outpost` logs
`{"code":"Access-Request", "ip":"10.3.1.2"}` for each attempt, and authentik
writes a `login_failed` event with `client_ip: 10.3.1.2`, `username: agent`,
rejected by the **identification stage** (`model_name: "identificationstage"`),
which is its plain "bad credentials" outcome. authentik has an active `agent`
user; its password is simply not the switch's local `agent` password.

The switch then **falls back to its own local account database**, authenticates
there, and lets you in — after having printed the misleading message. So the
sequence on every single `agent` login is: RADIUS asks authentik → refused →
message printed → local database lets you in. Nothing is broken about your
access; the message is noise from a source you are not actually using.

An earlier version of this note blamed an EAP/MS-CHAP attribute mapping. That was
wrong: the reject is a normal Access-Reject from a password check.

Fixing it means either setting the authentik `agent` password to match the
switch's local one or dropping RADIUS from the auth order. The device config,
read in the Manager context, is:

```text
aaa authentication telnet login  radius local
aaa authentication telnet enable radius local
radius-server host 10.3.1.36
```

so the login *and* the `enable` second login each log a `login_failed` in
authentik — a GUI refresh does two. (`show radius` and `show aaa` are not
commands on this firmware; the config is only in `show running-config`, which
also exposes the shared secret, so do not paste it anywhere you will keep.)

Two consequences worth knowing:

* A login failure is **not evidence that authentik is down** — and, conversely,
  a `login_failed` for `agent` from `10.3.1.2` is not an incident either.
* Monitoring that watches `https://id.leighhack.org` cannot see the RADIUS path
  at all: the web UI stays up while the outpost is broken.

`src/switch.rs::check_auth` reports both readings rather than guessing.

## Audit log

Every action (including refusals and read-only denials) is kept in memory, last
200 entries, at `/api/audit` — deliberately not on disk, since the switch has
its own logging and this box should not accumulate state for a tool that owns
nothing.
