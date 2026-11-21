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
  here needs a PPD at all.
- **`restartTriggers` carries the table**, so an edited row re-registers the queues
  on the next switch. Necessary rather than obvious: this image never reboots
  itself, so waiting for a reboot to pick up a changed printer is waiting forever.

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
3. **Before believing a switch landed:** `readlink -f /run/current-system
   /nix/var/nix/profiles/system-good` (the GOLDEN RULE) and `findmnt -no SOURCE
   /exports/netboot-squashfs` against `nix eval .#nixosConfigurations.pi-room-sys.config.system.build.squashfsStore.outPath --raw`
   — machines/pi-room-sys/README.md's export-remount trap means a client can
   silently boot the previous image, and a printer row that did not cross looks
   like a printer row that failed.

## What the rendered unit actually says

AGENTS.md rule 10 — a changed flag has to be read back from a real build rather than
trusted from what was typed, and NixOS rewrites quoting. What `cups.service` renders
as, read off the built `unit-cups.service` and its `cups-post-start` on 2026-11-21:

```
[Unit]
After=network.target
Wants=network.target
X-Restart-Triggers=/nix/store/<hash>-X-Restart-Triggers-cups   # carries ./printers-data.nix
[Service]
ExecStartPost=…/cups-post-start
ExecStartPre=…/cups-pre-start
[Install]
WantedBy=multi-user.target            # i.e. startWhenNeeded = false took effect
```

and in the post-start script:

```
lpadmin -p brother -D '…brother…' -E -m everywhere -v net+ippt://10.3.2.30:631/ipp/print
lpadmin -p toshiba -D '…toshiba…' -E -m everywhere -v ipp://10.3.1.50:631/printers/toshiba
```

Both facts that matter are visible there: the addresses came out of
`common/dns-records.nix` (they are not written in the table), and `-E` means the
queues are enabled rather than merely created.

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
