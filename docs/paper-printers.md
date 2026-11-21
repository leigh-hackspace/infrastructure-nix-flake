# The paper printers (and the Pi Room image that prints to them)

Disambiguation first, because the network has two kinds of printer and the flake
used to have only one kind of doc: **`docs/printer-servers.md` is the 3D ones** —
`3d-blue`/`3d-lime`, Klipper + Moonraker, monitored by
`crates/moonraker-exporter`. **This file is the sheet-of-paper end**: the two
devices whose leases `common/dns-records.nix` reserves as `brother` and `toshiba`,
and the CUPS plumbing `machines/pi-room-sys/` puts on the Pi Room's PCs so a
document open on a desk can actually go out of a machine.

Nothing else in the flake touches paper printing. There is no third paper printer:
`common/dns-records.nix` is the network's list of devices and these are the only
two whose records are not 3D, camera, switch or box.

## The two devices

| Record | IPv4 | VLAN | MAC | Status read 2026-11-21 |
| --- | --- | --- | --- | --- |
| `brother` | 10.3.2.30 | 226 (the WiFi VLAN) | `00:41:0e:0d:00:3b` | lease live |
| `toshiba` | 10.3.1.50 | `null` (untagged) | `00:80:91:76:6c:0a` | lease live |

The addresses are `common/dns-records.nix`'s fact and nothing in the image repeats
them: `machines/pi-room-sys/printers.nix` reads the IPv4 out of the record its row
names, and `machines/gw/lib/check-network-tables.nix` refuses a row whose record is
gone — from `just check-eval`, so the finding prints on the router too, where the
flake step cannot run. Renumber a printer and the image does not quietly print into
a dead address; it refuses to evaluate.

## What the image does

`machines/pi-room-sys/printers.nix` is the module;
`machines/pi-room-sys/printers-data.nix` is its table. Shape:

- **`services.printing.enable = true`, `startWhenNeeded = false`.** The image has
  no disk — root is tmpfs and `/var/lib/cups` is where the queues live — so queue
  state is re-made every boot, and the module's default of socket-activating CUPS
  would register the queues *inside* the first print job rather than before it.
- **One CUPS queue per printer, named after the record.** A printer has one name
  from the router's lease table through to `lp -d`, and the module asserts the
  queue name and the record name cannot diverge, because "print to the toshiba"
  stops meaning one thing when they do.
- **`model = "everywhere"`, i.e. driverless IPP.** `lpadmin(8)` (https://man7.org/linux/man-pages/man8/lpadmin.8.html)
  says the model `everywhere` "queries the printer referred to by the specified IPP
  device-uri" and that "models other than `everywhere` are deprecated". That is not
  a style choice here: the vendor drivers these two would otherwise want — Brother's
  `beh` PPDs, Toshiba's `estudio*` — are **not in nixpkgs**, so a driver would have
  to come from outside the flake. `cups-filters` and ghostscript, which the
  `services.printing` module adds unconditionally, are what turn the document into
  something the device understands. At this lock the package set the image pulls is
  `cups-2.4.19` with `cups-filters-2.0.1`, `libcupsfilters-2.1.1` and
  `ghostscript-with-X-10.08.0` — read off a `dry-build` of the image, not assumed —
  so `everywhere` is the current incantation rather than a legacy one, and nothing
  here needs a PPD at all. **The same sentence is why a queue only exists while its
  printer is awake**: registering it *is* a query to the device, so the pass that runs
  during boot fails for a printer that is switched off, with `Unable to connect …
  Host is down`. The retry unit below is what turns that into "one queue missing"
  instead of "no printers at all".
- **Registration is a unit of its own (`paper-printers.service`), not `cups.service`'s
  post-start.** `hardware.printers.ensurePrinters` generates an ExecStartPost for
  `cups.service` that runs with `set -e`, so one `lpadmin` that cannot register its
  queue fails the *scheduler* — and systemd's start rate limit then leaves CUPS
  permanently failed, which a desktop shows as "no printers" rather than "one
  missing". That is not hypothetical: it is what the first deploy did on 2026-11-21
  (receipts below). The unit retries each row independently, tolerates a sleeping
  printer (`lpstat -p` is the "already there" test, so a retry is free), and exits 0
  once every row has a queue, which leaves it `inactive (dead)` on a healthy boot.
  Its script carries the device URIs, so an edited row changes the unit and a switch
  restarts it — the job `restartTriggers` used to do.
- **`ipp`/`ipps` only.** `lpadmin -v` accepts no other scheme; a row cannot spell
  `net+ippt` (which this table did until 2026-11-21) because there is no such
  device-uri scheme and the module's assertion now refuses it.

## Protocol evidence, measured 2026-11-21 (probes from aibox and services1)

**`brother` is IPP-native at `:631/ipp/print`.** Proven, three requests:

```
GET  http://10.3.2.30:631/ipp/print          -> 405   (the path exists; POST is the method)
POST http://10.3.2.30:631/ipp/print          -> 200, 72 bytes
     body: 01 01 …  "attributes-charset=utf-8", "attributes-natural-language=en"
GET  http://10.3.2.30:631/printers/          -> 404   (not a share of a CUPS server)
GET  http://10.3.2.30:631/nic/nicFilter      -> 404   (no DNDM)
GET  http://10.3.2.30/                        -> 301  (its :80 redirects; no web UI to read)
```

The device answers *nothing* else: an earlier probe round the same day found
10.3.2.30 refusing a connection on 80, 631 **and** 443 while its lease was live.
The two passes differ because the printer was powered off in one and on in the other,
and a powered-off printer with a live lease is exactly the thing that makes a
protocol probe read as "the network is down" — which is why the endpoint claim above
is the later pass and the lease row is quoted with its timestamp. The 72-byte Get-Printer-Attributes reply is short against
the group length the device announces (327), so the attribute set that arrives is
partial: **whether that is enough for the filters to rasterise a page is
unmeasured**, and is exactly what a test print answers.

**`toshiba` is a CUPS-shaped IPP endpoint, and its queue name is a guess.** Its `:80`
is TopAccess (`Server: Apache`, `TopAccess`, `eFiling`, `gblTAUrl`) — a Toshiba
e-STUDIO — and its `:631` is its own:

```
OPTIONS http://10.3.1.50:631/printers/       -> 200, Server: CUPS/1.5, Allow: GET,HEAD,OPTIONS,POST,PUT
GET   http://10.3.1.50:631/                  -> 404 (342-byte CUPS page)
GET   http://10.3.1.50:631/version/          -> 404
GET   http://10.3.1.50:631/ipp/print         -> 404  (and 404 at :80) — not IPP-native
GET   http://10.3.1.50:631/nic/nicFilter     -> 404  (no DNDM)
https://10.3.1.50:443                         -> does not connect
```

So the URI shape is `ipp://10.3.1.50:631/printers/<name>`. **`<name>` is the one
value in the table that is a guess** (`toshiba`, the record name), and no request
from the network can find the real one: `GET /printers/` and `GET /admin/` and
`GET /jobs/` on that server send **no body at all** (curl times out at 0 bytes), a
POST there is refused the same way, and `OPTIONS /printers/<anything>` returns 200
for *every* name, so there is no name oracle either. That is why the row is marked
GUESS in `printers-data.nix` rather than filled with a plausible model string, and
why `docs/follow-ups.md` carries it as an open item.

**Where the real name already lives:** `10.3.1.30:631` — `apps1`'s `cupsd` — was
this network's print server before this change, and Home Assistant's `ipp` entry
"Big Laser Printer" dials it (docs/home-assistant-on-services1-2026-11-16.md). Its
`/etc/cups/printers.conf` therefore already holds working queues for paper
printers. It was **unreadable while this was written up**: `curl` to
`10.3.1.30:631` failed from both aibox and services1, and the router's lease table
matched **no** lease for `10.3.1.30` or for `10.3.1.11` (monster1, its host) — both
off-network that day. So the shape of the answer is already known and costs one
`lpstat -p` on that box once it is back: **if it registers these printers as shares,
the Pi Room image should dial that print server, not the devices**, and that is a
row edit rather than a redesign.

## Proving it, in the order the proofs mean

1. **`just paper-printers`** — run from any flake box. It re-reads the table's rows,
   asks each device what it answers today (the same requests as above), reports
   whether each record still has a live lease through the router's own lease table,
   and prints the verdict against what the image currently declares. This is the
   device half only: it says the endpoint is there, not that a page came out.
2. **The proof that counts is paper.** Boot the image with `just pxe-client-uefi`
   (on aibox — see that section's rule that the BIOS client is not evidence), then
   ssh into the guest from aibox: the README's "the booted guest is not a black box"
   applies here too, so `lp -d brother -n1 <file>` runs inside the client and the
   answer is a sheet from the device. On a board, `lpstat -p` and a print job from
   the desk app. The visible failure this rule exists to prevent is a queue that
   exists, enables, and prints nothing — every check inside CUPS stays green through
   that, as it stayed green through the MQTT name that 404ed (docs/mqtt-int-legacy-answer-2026-11-02.md).

   **`lpstat -p` lists the queues that exist, which is not the same as the rows.**
   A row whose printer was asleep when the client booted is *not* registered, by
   design: `-m everywhere` asks the device. So read `journalctl -u paper-printers`
   next to it — `registered brother` / `toshiba has no queue yet` — before calling a
   missing queue a fault, and do not "fix" it by creating queues that cannot print.
3. **Before believing a switch landed:** `readlink -f /run/current-system
   /nix/var/nix/profiles/system-good` (the GOLDEN RULE) and `findmnt -no SOURCE
   /exports/netboot-squashfs` against `nix eval .#nixosConfigurations.pi-room-sys.config.system.build.squashfsStore.outPath --raw`
   — machines/pi-room-sys/README.md's export-remount trap means a client can
   silently boot the previous image, and a printer row that did not cross looks
   like a printer row that failed.

## What the rendered units actually say

AGENTS.md rule 10 — a changed flag has to be read back from a real build rather than
trusted from what was typed, and NixOS rewrites quoting. Read off the built toplevel
(`/nix/store/9p1k4brx…-nixos-system-pxeclient-…`) on 2026-11-21, after the first
deploy's failure below:

```
# cups.service — no ExecStartPost at all any more
[Service]
ExecStart=…/cups-2.4.19/sbin/cupsd -l
Type=notify

# paper-printers.service
[Unit]
After=cups.service
PartOf=cups.service
Requires=cups.service
StartLimitIntervalSec=0
[Service]
ExecStart=…-paper-printers-provision
Restart=on-failure
RestartSec=20s
Type=simple
[Install]
WantedBy=multi-user.target
```

and in the provision script, one guarded pair per row:

```
if lpstat -p brother >/dev/null 2>&1; then :
elif lpadmin -p brother -D '…brother…' -E -m everywhere \
     -v ipp://10.3.2.30:631/ipp/print; then
echo "paper printers: registered brother"
else … pending="$pending brother"
fi
```

Three facts that matter are visible there: the addresses came out of
`common/dns-records.nix` (they are not written in the table), `-E` means the queues
are enabled rather than merely created, and **the failure of one row can no longer
take the scheduler with it** — the guard is per row and the loop, not the scheduler,
is what retries.

## What the first deploy got wrong (2026-11-21 17:44, measured on a booted client)

The image shipped at 16:35 registered no queues and left CUPS dead, and the desktop
therefore offered **no printers at all** over both devices. Measured by booting
`just pxe-client-uefi` on aibox, sshing into the guest (10.3.14.204) and reading:

```
systemctl is-active cups        -> failed
lpstat -p -d                    -> lpstat: Scheduler is not running.
journalctl -u cups              -> cups-post-start[1124]: lpadmin: Bad device-uri scheme "net+ippt".
                                   cups.service: Failed with result 'exit-code'. … start-limit-hit
```

Two independent causes, and the shape of the fix follows from both:

- **`net+ippt` is not a device-uri scheme.** `lpadmin -v` takes `ipp`/`ipps`
  (measured: `ipp://10.3.2.30:631/ipp/print` registers the queue, rc=0, on the same
  guest). The `net+` spellings the table and its assertions used to allow were a
  belief, not a measurement, and nothing refused them until a booted client did.
- **`-m everywhere` queries the device, so a printer that is off cannot be
  registered.** Provoked deliberately on the same guest: `lpadmin -p toshiba -v
  ipp://10.3.1.50:631/printers/toshiba -m everywhere` → `lpadmin: Unable to connect
  to 10.3.1.50:631: Host is down` (the toshiba answered nothing all day — see
  `just paper-printers`). And because that `lpadmin` ran in `cups.service`'s
  post-start with `set -e`, it did not merely skip the queue: it failed the
  scheduler. Order mattered too — the `brother` row failed first, so a broken
   *scheme* and a sleeping *printer* produced the same symptom.

What that means for a user in the room: with this fix, `brother` gets a queue and
prints; `toshiba` gets one the first time it is awake while a client is up, and the
unit says so in the journal. **A powered-off printer has no queue, and that is the
design** — the alternative (a queue that exists and prints nothing) is the failure
mode this doc's "proving it" section exists to catch.

## Cost, measured on the image 2026-11-21

- **The image builds.** `nix build --no-link '.#nixosConfigurations.pi-room-sys.config.system.build.toplevel'` on aibox finished rc=0 at `/nix/store/razr37ljhq00kyq6pgr54gdyngci984a-nixos-system-pxeclient-26.11.20261112.2074cc6`, having built 479 derivations — the image had not been built in that store since the 2026-11-19 netboot rebuild, so that count is a fresh build rather than an incremental one, and it is the compile proof this change needed.
- **Store closure: 20.2 GiB over 2,389 paths** (`nix path-info -h --closure-size` on that toplevel, same day) — what the image's `squashfsStore` carries, compresses at zstd level 10 with one thread, and hands out over NFS at boot: `machines/pi-room-sys/README.md`'s "the only cost worth counting".
- **What the printers add is small against that:** `cups-2.4.19`, `cups-filters-2.0.1`, `libcupsfilters-2.1.1`, ghostscript and the filter backends, read off a `dry-build` listing of the image. The closure is dominated by what the image already carried — libreoffice, `nix-software-center`, the font/tex stack. The delta was *not* isolated by building the image twice (two quarter-hour builds on the network's builder, for a number nothing acts on); it is bounded by the packages just named.
- **The squashfs HAS been re-cut, and both sides are measured.** The deploy landed on aibox on 2026-11-21 16:35 (generation 479, `nixos-confirm` chained in the same invocation, `readlink` of `/run/current-system` and `system-good` equal, zero failed units), and aibox's export `/exports/netboot-squashfs` — the live media, mounted off a store path named `…-squashfs`, not off `/srv/netboot` — now holds `fmgb2d9vj7dr80dg71754l2fff3z30r8-squashfs` at **7,552,843,776 bytes over 2,390 store entries** where the image aibox cut at its previous switch was **7,506,362,368 bytes over 2,360 entries**. So the printers cost **+46,481,408 bytes (44.4 MiB, +0.62%)** and 30 entries: that is the whole boot-time cost, and the switch took about 8 minutes wall rather than the 2026-11-19 run's 18 min + 46-minute postStart, because nixpkgs did not move and the image's closure had already been built by the direct `nix build` above.
- **CUPS the *package* was already in the image; `services.printing` is what adds the *service*.** Both squashfs images were opened read-only and listed (`mount -o loop,ro`): the old one already carried `cups-2.4.19` and ghostscript — pulled by the desktop's own deps — but had no `unit-cups.service`, no `cups-progs`, no `cupsd.conf`/`cups-files.conf`, no `cups-filters`/`libcupsfilters` and no printer table; the new one has all of them plus `2i1s80fc…-paper-printers-table`. That is why the delta is 44 MiB and not "a CUPS stack": the filter chain and the queues are the new half, and the reason anyone had no printer before was the service, not the library.
- **The same switch moved aibox's strata engine** with upstream's 0.1.63 pin (`flake.lock`'s `strata` rev `3142b66b9`): `strata.service`'s `ExecStart` now dials `/nix/store/wd40i8bgr2dg0hwc2j5c1544r6cr1pws-strata-unstable` where it dialed `sr4rrkb7qab341ki583gi62fxkzk6zr4-…` before. `machines/aibox/STRATA.md` carries bump receipts through engine 0.1.59.1 only, so the 0.1.63 leg on the HIP side has no receipt written — see `docs/follow-ups.md`.

## Also dark that day, so it can be told apart from a printer fault

`10.3.1.30` (apps1, and with it `cupsd` on `:631`), `10.3.1.11` (monster1) and
`10.3.1.21` (services2) answered nothing and had no lease in the router's live table.
`10.3.1.1:8091` (the router's own dashboard) was unreadable from aibox, but
`https://network-info.int.leighhack.org/api/leases` was, and reported
`"stale":null` with `source_age` 17 s — i.e. the lease channel was live, which is
what makes the lease answers above usable at all (rule 29: a frozen lease table
otherwise looks like a quiet network).

## The second deploy, 2026-11-21 17:58 (the fix, measured on a booted client)

The fault above was fixed and deployed the same evening: `paper-printers.service`
replaced CUPS' post-start, `brother`'s scheme became `ipp`, and the assert list lost
the `net+` spellings. Receipts, both sides:

- **aibox switched clean** at 17:58 (`switch rc=0`, `nixos-confirm` chained in the same
  transient unit, `readlink -f /run/current-system /nix/var/nix/profiles/system-good`
  equal, `systemctl --failed` empty), and `/exports/netboot-squashfs` was re-cut from
  `fmgb2d9vj7dr80dg71754l2fff3z30r8-squashfs` (7,552,843,776 bytes) to
  `4w4hwmx2j8r7j8b2srcml0k9xi1dlk9a-squashfs` — **7,552,860,160 bytes, +16,384**:
  the unit's script is 16 KiB against the post-start script it replaced.
- **A guest booted from it** (`just pxe-client-uefi`, 10.3.14.231) answered
  `cups -> active`, `paper-printers -> active`, `systemctl --failed` empty, and its
  journal read `paper printers: brother has no queue yet — its device did not answer`
  / `… toshiba …` with `still missing brother toshiba — retrying in 20 s`. **No
  queue, because both printers were switched off or asleep** — measured independently
  of the client: the router's own neighbour table read `10.3.2.30 dev vlan226
  INCOMPLETE` and `10.3.1.50 dev vlan225 FAILED` at the same moment, while `brother`
  had answered `POST :631/ipp/print → 200` from aibox at 17:45. So the scheduler
  survives, the rows are each attempted, and the queues arrive when the devices do.

**What is still unproven is the same thing as before: a sheet of paper.** The
registration command itself is proven — on the previous guest
`lpadmin -p brother -v ipp://10.3.2.30:631/ipp/print -m everywhere -E` returned 0 and
`lpstat -p` then read `printer brother is idle`
(`docs/follow-ups.md` carries it as the open item).

## The third report: a printer that is switched off keeps its place in the dialog (2026-11-21 18:5x)

The next report after the 17:58 deploy was *"i just tried on a pi room computer and
could not see anything other than the option to print to PDF"* — and the image was
working exactly as designed, which was the finding. Measured, in order:

- **The client was on the new image.** `dnsmasq`'s log on the router shows
  `pi-room-01` (10.3.14.101) DHCPing at 18:14–18:15, i.e. after the 17:58 re-cut, and a
  guest booted from that same export answered `cups -> active`,
  `paper-printers -> active`, `systemctl --failed` empty.
- **Both printers were off the network, not answering slowly.** The router's own
  neighbour table read `10.3.2.30 dev vlan226 INCOMPLETE` and `10.3.1.50 dev
  vlan225 FAILED`; `arping` from the router got 0 replies from either, while the
  cameras on the same VLANs (10.3.2.10, .17, .11) answered, so it is those two devices
  and not their VLANs; and neither has DHCPed since 17:44 (`brother`) / 17:19
  (`toshiba`), having both renewed earlier the same afternoon while they were awake.
- **So the queue table was empty by design**: `-m everywhere` asks the *device* for its
  attributes, so a dark printer registers nothing, and the dialog showed only the
  virtual "Print to PDF" destination. One printer off = one missing destination;
  both off = a print menu that looks broken.

**The correction, measured on a booted client of the 2026-11-21 18:36 image:** a row
that cannot go driverless gets a *placeholder* queue instead of nothing.

- `lpadmin -p darktest -E -v ipp://10.3.2.30:631/ipp/print` — no `-m`, no `-i` — returns
  **0 against a device that is completely dark**, and `lpstat -p` then reads
  `printer darktest is idle. enabled since …`. That is the whole trick: the destination
  needs a URI, not a PPD.
- A failed driverless attempt **against an existing queue deletes it** — after
  `lpadmin -p darktest -m everywhere` answered `Unable to connect to 10.3.2.30:631:
  Host is down`, `lpstat -p` read `No destinations added.` So the placeholder is
  re-created on every failure path, and the upgrade is gated behind a TCP probe
  (`/dev/tcp/<addr>/631`, 3 s) rather than attempted blind.
- The placeholder is held with `cupsreject -r`, so a job is refused with
  *"the printer is switched off or asleep; this queue accepts jobs when it answers"* —
  visible in the dialog, and better than a PPD-less queue spooling a document that would
  print as whatever arrives once the printer wakes.
- On the booted client both destinations existed: `lpstat -p` listed `brother` and
  `toshiba` with that reason underneath each, `/run/paper-printers/{brother,toshiba}`
  each said `placeholder`, and the journal read
  `paper printers: brother is not answering — queue held with a reason until it does`
  every pass without re-running `lpadmin` once the queue was held.

**Still unproven:** the placeholder → driverless upgrade on a device waking up. The
driverless registration itself is proven (17:45, `brother` awake, rc=0, `printer brother
is idle`), and the upgrade is the same call behind a TCP gate, but it has not yet been
watched happen — the open item is in `docs/follow-ups.md`, and it needs nothing but a
printer being switched on while a client is booted.
