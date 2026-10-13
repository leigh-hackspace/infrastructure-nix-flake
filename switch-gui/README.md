# switch-gui

Ports and VLANs on the network's ProCurve switches, in a browser.

Three switches, all managed over **Telnet** — none has an API, and their own web
UIs need a Manager login and still will not answer "which VLAN is port 17 on" in
one screen. This tool reads the configuration with `show` commands, merges the
tables, and serves one page per switch at `https://switch.int.leighhack.org`
(`/` lists them, `/switch/switch2` is one switch):

| name      | address    | model                    | revision | `System Name` |
| --------- | ---------- | ------------------------ | -------- | ------------- |
| `switch1` | `10.3.1.2` | J9280A Switch 2510G-48   | `Y.11.44` | `switch1`     |
| `switch2` | `10.3.1.3` | J9280A Switch 2510G-48   | `Y.11.44` | `switch2`     |
| `switch3` | `10.3.1.4` | J4903A Switch 2824       | `I.10.107` | `switch3`     |

The names are the switches' own `System Name`, and they are the same table
`common/hosts.nix` gives the gatus checks, so the GUI, the monitoring and the
devices cannot disagree about which switch is which.

One login serves all three: the same `agent` account is a Manager on each, so
there is one credential pair (`SWITCH_GUI_LOGIN`/`_PASSWORD`) and one Telnet
session **per switch**. The CLI is one console per device, so the mutex is per
switch: a slow read on the 2824 cannot hold up a page for a switch that answered
normally, and a switch that is down keeps its last good snapshot instead of
blanking the other two.

Deployment: `machines/services1/services/switch-gui.nix` (services1 proxies it;
the binary listens on `127.0.0.1:8097`). The switch list is built from
`common/hosts.nix` and reaches the app as `SWITCH_GUI_SWITCHES`.

## What it shows

One row per port and per trunk group, merged out of four commands, because no
single one of them has the whole picture:

| command                | what only it knows                                            |
| ---------------------- | ------------------------------------------------------------- |
| `show vlan`            | the VLAN list, including names with spaces (`AD Lab`)          |
| `show vlan <id>`       | port membership (`Untagged`/`Tagged`) and `Up`/`Down` per member |
| `show spanning-tree`   | every port's STP state (`Forwarding`/`Disabled`/…) and media    |
| `show trunks`          | trunk group/type, and names for the few ports in a trunk        |
| `show interfaces`      | per-port byte/frame/error counters                              |
| `show running-config`  | **port names** — the only place this firmware reports them       |

`show port name` and `show ports` do **not** exist on this firmware at all (as
Manager, `show port name` answers `Ambiguous input: port`), and the `Name`
column of `show vlan <id>` is blank for every port whose membership is dynamic —
which on `switch1` is every port.  So the running config is where names come
from, and it is read for that alone: see "Reading names out of the running
config".

Below the tables is a **VLAN membership matrix** — one row per port, one vertical
column per VLAN — which is the only place membership is edited.  See "Editing
VLAN membership".

Every action is audited with the switch it went to (`/api/audit`), because with
three devices a log line that does not name one is not evidence of anything.

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

The switches come from `SWITCH_GUI_SWITCHES` (or `--switches`), so a local run
against one device is `--switches switch3=10.3.1.4`.

Tests: `cargo test` — 101 of them, no network, no switch. The parser fixtures are
captured output from the real devices, one per firmware.

## Two firmwares, one parser

The 2510G (Y.11.44) and the 2824 (I.10.107) are not the same tables, and the
differences are caught by fixtures rather than assumed:

* **`show vlan` has an extra column on the 2824** — `Voice`, between `Status`
  and `Jumbo`. Reading the row from the right is the only safe way (the `Name`
  column can hold a space, `AD Lab`), so the number of trailing columns has to
  come from the header: assuming two makes every 2824 row's `Status` read as
  `No` and the whole VLAN list disappears.
* **`show spanning-tree` puts `State` in a different `|`-block.** On Y.11.44 the
  port and media are in the first block and `Cost/Priority/State` in the second;
  on I.10.107 `Cost/Priority/State` are in the *first* block with the port. A
  fixed index reads every 2824 port's STP state as `128` (its priority). The
  block index is read off the header, and the state is taken as the last column
  of that block, which also survives rows whose later columns are empty.
* **The 2824's banner ends with `Press any key to continue`** and the CLI will
  not ask for a login name until it gets a keypress. A login name typed before
  the gate loses its first character to the keypress — `agent` arrives as `gent`,
  RADIUS refuses it, and the switch prints its generic authentication failure.
  `Switch::connect` answers the gate, then waits for the prompt. The 2510G does
  not print the gate at all, so this is a no-op there.
* **Every table on the 2824 pages**, including `show vlan` (5 VLANs), so the
  paging handler is on the hot path for that switch rather than only for the
  48-port counters table.
* **A login on the 2824 can take 15-25 s**, because RADIUS is tried first and its
  retry ladder is ~5 s per attempt (see the RADIUS section). That is why the login
  reads use a separate, longer timeout (`SWITCH_GUI_LOGIN_TIMEOUT`, default 45 s)
  rather than the 20 s command timeout: at 20 s the read times out on `switch3`
  before the prompt ever arrives.

The 2824 is also the switch that cannot be reached over Telnet from the dev box
at all times: it answers, but slowly (a full read is nearer a second than a
tenth).

## The switch CLI, the parts that will bite you

Everything below was established against the live devices (`10.3.1.2` and
`10.3.1.4`), not read out of a manual. The code comments point back here.

**Lines end in LF, never CR LF.** The CLI counts a CR *and* an LF as a keypress,
so `agent\r\n` submits the username and then submits an **empty password**: you
get `User authentication failure.` and conclude the credentials are wrong.
`telnet::send_line` sends a bare LF.

**The banner swallows input.** Anything typed before the CLI prints
`Please Enter Login Name:` is discarded — and then the *password* you sent next
is read as the login name. So `switch::Switch::connect` waits for each prompt
rather than typing on a timer. On the 2824 there is a key gate in front of that
prompt (see "Two firmwares, one parser").

**It pages, and ignores `NAWS`.** Long tables stop with

    -- MORE --, next page: Space, next line: Enter, quit: Control-C

Advertising a 100-row window changes nothing; the only fix is to answer the
marker with a space. `Switch::command` does exactly that, and deletes the marker
*without* the rest of the line, because the interrupted row continues after it.
On the 2824 the marker can sit at the end of a line with no `ESC E` after it, so
the wait predicate has to accept a response that ends on the marker as well as
one that ends on a prompt.

**It echoes**, prefixed with the prompt and terminated by `ESC E`, so the first
line of every response is the command you sent (`strip_echo`).  It also paints
the prompt for the *next* command on the **end of that same row**: a command that
prints nothing answers `interface 23 name "x"switch1(config)#`, with no line break
anywhere in it.  That is the most consequential thing on this page — it is what
made every write look like it had failed.  See "The write path".

**And it ends its lines with LF, then CR-paints to the next row.**  So what
`str::lines()` gives you is `"\r   name \"AP3\""`: every line *starts* with a
carriage return.  Anything that splits from the right and `trim()`s never notices
— which is why every table parser in `parse.rs` works and the one parser that
asks whether a line is indented did not.  Strip the leading CR before testing for
indentation.

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

Each switch has two CLI contexts and the prompt character tells you which:
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
would make the next *read* look like a parser bug. On the 2824 the config-mode
prompt is `switch3(config)#` — the same shape, so the prompt matcher works for
both.

Nothing here writes NVRAM (no `write`, no `copy running-config startup-config`),
so **every change is in RAM and a reboot undoes it**. Say that out loud before
trusting one. If it is ever changed: on Y.11.44 the command is `write`, whose own
help line is "Write current command set to memory", and `write ?` already answers
`Invalid input: ?` — so `wr mem`/`write memory` is the *other* firmware's spelling
and should not be assumed here.

## The write path, and what was wrong with it

**Writes work.**  Everything in the earlier version of this section — "the next
command is never echoed", "the difference is telnet option negotiation, capture
the TCP stream" — was wrong, and it was wrong in a way that made the write path
less safe rather than more.  What follows is what 2026-10-13 measurement found.

A raw Telnet client that answers the negotiation exactly as this app does (every
`WILL` with `DONT`, every `DO` with `WONT`) logs in, enables, enters config mode
and applies a port name on `switch1`:

```text
TX> "config"
RX  configswitch1(config)#
TX> "interface 23 name \"TEST WRITE\""
RX  interface 23 name "TEST WRITE"switch1(config)#
```

Echoed, prompt back, in 0.15 s.  Negotiation was a red herring: `nc` works
because it answers *nothing*, and answering `WONT`/`DONT` is equally acceptable
to this CLI.  The idle repaint and the cursor parked at column 18 that the old
note read as "an empty input line, so the parser never saw it" was the cursor
sitting at the end of the echoed command — on the same row as the prompt.

The bug was `is_prompt`, which required a line to be *nothing but* a prompt.
Since this CLI paints the echo and the next prompt on one row, a command that
prints nothing — every configuration command — answered a line the matcher could
not accept.  So each write waited out its 20 s, the retry sent it again, and the
operator was told the console had never echoed a command the console had echoed
twice.  `show running-config` after a "failed" write showed `interface 23` /
`name "TEST WRITE"`: **the change had been applied**, and the only thing missing
was the truth about it.

Three consequences, all now in the code:

* `is_prompt` reads the prompt as a **suffix** of the line, since that is where
  the CLI puts it.  The name is not anchored at its start either, because after
  `exit` the row is `exitswitch1#`.  The cost is that a response line ending in
  `word#` would end a response early; no row in any captured table does, and
  `no_response_row_ends_like_a_prompt` pins that against every fixture.
* **The retry was the dangerous part, not the fix.**  A command that timed out
  with no echo was resent on the theory that the console swallows input while it
  repaints; for a configuration command that had already been applied, a resend
  is a second application.  `Switch::command` now decides from the partial buffer
  the old code threw away: no echo at all means the line never reached the parser
  and one resend is safe, an echo followed by silence means it did reach it and
  the command is **never** resent — the error says the command may have been
  applied and to re-read the switch, which is the only real confirmation there
  is.  `telnet::ReadFail` exists to carry that partial buffer.
* `leave_config` exits until the prompt stops naming a context instead of sending
  one `exit`.  `interface 5` answers `switch1(eth-5)#`, and one `exit` from there
  lands back in `(config)#`, leaving the cached console inside config mode so the
  next *read* fails like a parser bug.  A fixed number of `exit`s is also wrong:
  at an Exec prompt `exit` terminates the session — its own help says so — so the
  prompt is consulted before each one.

Verified end to end through the app's own HTTP API on port 23 of `switch1` — a
port that is Down, has moved zero bytes, is untagged in the default VLAN and had
no name: `POST /api/port/23/name` returned `applied:` in 0.8 s (was 40 s and a
false failure), `show running-config` showed the name **once**, not twice, and
`no name` put it back.  Membership was exercised the same way: adding tagged 225
sent exactly `vlan 225 tagged 23`; moving to `untagged 226, tagged 227` sent
exactly the four commands the diff implies, in the right order; and the port came
back to its original state and was confirmed clean in all three VLANs.

Still unverified: the **VLAN-management** commands (create, rename, delete, and
the per-VLAN `tagged` endpoint), which have never been sent at all.

Anything the switch does refuse comes back as its own `Invalid input:` text,
surfaced verbatim — that part is tested, and a refusal is no longer reported as
a success.

## Reading names out of the running config

Port names are the one setting this firmware will not report in a table.  The
`Name` column of `show vlan <id>` is blank for every dynamically-learned member,
which on `switch1` is all fifty ports, and `show port name` does not exist.  The
names live in `show running-config`, one block per configured port:

```text
interface 22 
   name "AP3" 
exit
```

Two things make that input awkward, and both are pinned by tests in
`parse::port_names`:

* **A `vlan` block has a `name` line too** (`vlan 225` / `name "Shared"`), in the
  same shape.  Reading it as a port name invents a port called `225`, so a block
  is open only between `interface <id>` and the next line at column 0.
* **Every line arrives behind a carriage return.**  The CLI ends its lines with
  LF and then CR-paints to the next row, so `str::lines()` yields `"\r   name
  \"AP3\""`.  Asking whether that line is indented answers "no", every block
  closes before its `name`, and the parser returns nothing — while the command
  succeeds and the page quietly shows no names at all.  Nothing else in `parse.rs`
  notices the CR, because everything else splits from the right and `trim()`s it
  away; this is the only parser that cares about leading whitespace, and the only
  one with a CR-shaped test.

And the reason this command is treated differently from every other read: **its
output carries secrets** — the RADIUS shared secret, the local user password
hashes, the SNMP community.  `Switch::secret_command` exists because every
failure path in `command` quotes what it was reading (a timeout appends the tail
of the buffer) and those errors reach the page and the journal.  This repo has
already leaked the RADIUS secret once by reading this very command into a
transcript, and the masking filter that missed it is why the answer is "withhold
the failure entirely" rather than "filter it": a filter is one syntax change
away from leaking again.  `parse::port_names` returns names and nothing else, so
a secret has nowhere to go even if that ever changes.

## Editing VLAN membership

The switch's own model is VLAN-first — `vlan 225 tagged 7,9-11` is a statement
about a VLAN and a pile of ports — but an operator at a patch panel thinks per
port.  The matrix is the per-port question ("which VLANs is 7 in, and which one
is native?") answered on one screen: a checkbox per VLAN, the untagged VLAN a
dropdown, and the VLAN names as **vertical** headers because ten VLANs across
fifty ports only fit if a column costs the width of a checkbox.

What the form sends is the **whole wanted row**, never the click that changed it,
and `commands::port_members` turns it into the difference against the snapshot
the page was built from.  That is what makes a two-checkbox edit cost two
commands instead of one per VLAN the port belongs to, and it is why a row is
marked as changed the moment it stops matching the switch: this page is a cache,
so it shows what it is *about to change* rather than pretending it *is* the
switch.  Nothing is sent until Apply, and the reload afterwards re-reads the
device, so what is on screen after an apply is what the switch says.

The order of the generated commands is not cosmetic — take away, then move the
native VLAN, then add.  A port cannot be tagged and untagged on the same VLAN at
once, so moving port 7 from `untagged 225, tagged 226` to `untagged 226` has to
`no vlan 226 tagged 7` **before** it sets the untagged side.  And the untagged
move is `no`-then-set, because a port has exactly one native VLAN and setting the
new one without releasing the old leaves it showing under both.

Two guards worth knowing because they look like bugs otherwise:

* A **trunk member** is shown with its boxes disabled and the reason on hover.
  The switch refuses to change the membership of a port in a link-aggregation
  group ("remove it from the trunk first"), and a trunk *group* has never been
  exercised here, so neither gets a live control.
* `tagged=` and `untagged=` are both **required** on the API.  A missing
  `tagged=` would be indistinguishable from "tagged in nothing", and a request
  that lost its query string would then strip every membership off the port.

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

`src/switch.rs::check_auth` reports both readings rather than guessing — and it is
a **note, not a failure, when a prompt follows**: the session is usable, and
refusing the connect on that message would make the GUI unusable on a switch that
is answering normally (which is what happened on `switch3` before it was a note).
The note goes into the snapshot's warnings, so the page shows it.

## Audit log

Every action (including refusals and read-only denials) is kept in memory, last
200 entries, at `/api/audit` — deliberately not on disk, since the switches have
their own logging and this box should not accumulate state for a tool that owns
nothing. Each line names the switch it went to.
