# Monster (Proxmox hypervisor, 10.3.1.11)

Moved out of `AGENTS.md` on 2026-10-14. `AGENTS.md` keeps the rules: root SSH is
keyed here (unlike the flake machines), check `pvesm status` before blaming a VM,
HA-managed VMs are started with `ha-manager set … --state started` and never
`qm start`, HA `error` clears disable → **wait** → start, and the `onboot: 0` lab
VMs are left alone. The full power-cut procedure is
**`docs/monster-powercut-runbook.md`** — read it before touching a stopped VM.

## The box

`monster1` runs **Proxmox VE 9.x** and hosts the off-flake VMs (apps1, authentik,
web1, mercury, the GOAD/k8s lab, …). These are *not* managed by this flake;
nothing here is declarative. Web UI: `https://monster.int.leighhack.org` (services1
nginx → `https://10.3.1.11:8006`, see `machines/services1/http.nix`).

```bash
ssh -i ~/.ssh/agent-hop-key root@10.3.1.11   # root IS keyed here
```

Handy commands: `qm list`, `qm config <vmid>`, `qm start <vmid>`,
`ha-manager status`, `pvesm status`.

## Storage: the VM disks live on the NAS

Storage `nas2-nfs` = `10.3.1.6:/mnt/sas-10k/proxmox-monster-ds1`, mounted at
`/mnt/pve/nas2-nfs`. (`nas1-nfs` → 10.3.1.5 is disabled; `nas1` does not answer
ping.)

**If the NAS is still booting after a power cut, every VM start fails with
`TASK ERROR: storage 'nas2-nfs' is not online`** — visible in `ha-manager status`
as `service vm:<id> (monster1, error)` and in `/var/log/pve/tasks/active`. The fix
is to wait for the export and retry. Check `pvesm status` /
`mount | grep nas2-nfs` before blaming the VMs, and do **not** treat the failed
start tasks as a VM-level fault. All VM disks are `cache=direct` qcow2s on this
mount, so boots take many minutes even when nothing is wrong (measurements and the
full reasoning are in the runbook).

## The VM inventory

| VMID | Name | HA-managed | `onboot` |
| --- | --- | --- | --- |
| 107 | mx1 | no | 1 |
| 108 | authentik | **yes** | 1 |
| 109 | apps1 | **yes** | 1 |
| 127 | web1 | **yes** | 1 |
| 128 | mercury | **yes** | 1 |
| 132 | Terraria | no | 1 |

**That table is the configuration, not the state, and on 2026-11-16 the two had drifted
completely.** Measured (`qm list`, `ha-manager status`, and the last write to each VM's own
qcow2): only **109 `apps1`** and **127 `web1`** were running. 108 `authentik` is correctly
stopped since the 2026-10-28 cutover (HA `stopped`, `onboot: 0`, and it must stay stopped —
see `common/dns-records.nix`'s note). 128 `mercury` was in HA `error` (below). 107 `mx1` and
132 `Terraria` had not written to disk since 2026-10-07; 125 `unifi` since 2026-06-01, superseded
by `machines/services1/services/unifi-os-server.nix`; and **every lab VM on the host had last
written between 2023-10 and 2025-04**. There are **no backup jobs and no snapshots** on this
host — `/etc/pve/jobs.cfg` is empty, `/mnt/pve/nas2-nfs/dump/` is empty — so a qcow2 on
`nas2-nfs` is the only copy of anything. Read
["Decommissioning"](#decommissioning-what-is-actually-used-measured-2026-11-16) before
assuming a VM in that table is a VM somebody is using.

## `mercury` is not on this box any more (2026-11-16)

VM 128 was the hackspace intercom (Sangoma FreePBX 15 / Asterisk 18 on CentOS 7, hostname
`freepbx`, DHCP `10.3.1.166`). It is now `machines/services1/services/pbx.nix` — bare
`services.asterisk` on services1, answering that same `10.3.1.166` as a second address, with
the handsets' SIP passwords carried across verbatim so nothing was re-typed. **Read
`docs/pbx-on-services1-2026-11-16.md` before touching the dial plan or the VM**; it owns the
measurements (5 extensions, 2 registered handsets, **0 trunks**, 305 calls in 2.4 years, no
voicemail, no recordings, no provisioning), the two bugs found in its config, the deploy order,
and the rollback (`ha-manager set vm:128 --state started`).

Why it needed this write-up: **it had been down for ten days and nobody knew.** Every HA start
attempt after the 2026-11-06 host boot failed with `storage 'nas2-nfs' is not online` — the
`docs/monster-powercut-runbook.md` race, third occurrence, and `max_restart 3` parked the
service in `error`, where it sits silently. That race is still live for every other VM here.

## Decommissioning: what is actually used (measured 2026-11-16)

Asked and answered once, so it is not re-derived per VM. Short version: **`web1` (VM 127,
10.3.1.39) is the only thing on this host with real traffic** — it answers the three public
front-door rows `webhooks.leighhack.org` (a webhook receiver: 1.79 GB in / 31 MB out over
10.3 days, ~420 inbound connections/day), `web-test.leighhack.org` (per-branch previews of the
hackspace website) and `retro.leighhack.org` (a Hugo copy of the site; the canonical
`www.leighhack.org` is on GitHub Pages). `apps1` (VM 109) has three front-doored origins and
its own Prometheus, and almost nothing dials it — a 120-second SYN capture found only gatus
health checks and Home Assistant's IPP poll; its Grafana had zero requests in 24 h and its
Prometheus had 10 of 34 targets up and no alert rules. `mx1` being off has already broken mail
from `apps1` (its `relayhost` is a name that no longer resolves). `unifi` is superseded. The
rest is three years of dormant disk.

Two things that file `AGENTS.md` says and that matter here: monster1 is excluded from the
fleet-shutdown list (`machines/gw/ups.nix`), and `docs/monster-powercut-runbook.md` is the
procedure for bringing this host's VMs back — not for deciding which ones deserve to come back.

Everything else is `onboot: 0` and started by hand: the GOAD lab
(192.168.10.0/24), rtsp, the windows/AD labs. **Starting the six infra VMs at once
needs ~24 GB of the host's 32 GB RAM** — leave the lab VMs alone unless there is
room, and start them one at a time (each is a burst of random reads against the
slow pool).

## What runs inside `apps1` (109, 10.3.1.30) — and what is off

`apps1` answers on **10.3.1.30** (`common/dns-records.nix` pins that address to the
VM's MAC `92:99:29:be:af:95`, which is `qm config 109`'s `net0`), so the box at that
address is this VM, not a look-alike. It is reached over ssh with the **machine-hop-key
for `root` and for `cjdell`** — `ssh -i ~/.ssh/agent-hop-key root@10.3.1.30`. `leigh-admin`
is **not** keyed there (measured 2026-11-01: `Permission denied (publickey,password)` for
`leigh-admin@10.3.1.30` from services1 *and* from monster1, and monster1's own `id_rsa`
is refused for every user tried). `qm terminal 109` is not an alternative either: it needs
a serial device and `qm config 109` has none, so adding one would mean a reboot.

Inside, `apps1` keeps its own hand-written docker+systemd services under `/srv`
(`grafana`, `homeassistant`, `jenkins`, `mosquitto`, `prometheus`, `uptimekuma`,
`usertweaker`, `vulcanbot`, `zigbee2mqtt`, `hackspaceapi`, `home`) — `/srv` and its own units,
whether or not they are running (three are not, below, and `homeassistant` is a fourth as of
2026-11-16, moved off this box). None of it is
declarative; the flake only points at this box.

**Home Assistant is not on this box any more (2026-11-16).** It moved to services1 as a podman
container (`machines/services1/services/home-assistant.nix`), and what answers `10.3.1.30:8123`
here now is a `socat` forwarder to `10.3.1.20:8123` whose journal is how we find any device that
hardcoded this address (`just ha-legacy-watch`). The address itself could not move — four
front-doored origins (`:8081`–`:8084`), `cupsd` on `:631`, and this box's own Prometheus all still
live here at the time of that move — which is the whole difference from the authentik `.36` move in
`docs/authentik-container-migration.md`. `home-assistant.service` is stopped and disabled, not
removed, and `/srv/homeassistant` is untouched: that is the rollback.
`docs/home-assistant-on-services1-2026-11-16.md` owns the move, the client map, and the ZHA radio
collision a flag-for-flag copy would have caused.

**The Space API left the same afternoon (2026-11-16), so `:8081` is gone from this box too.** It
is `machines/services1/services/hackspace-api.nix` on services1 now, and `api.leighhack.org`
proxies there. What remains here is what makes retiring this VM still a project rather than a
switch: `jenkins` `:8082`, `access-api` `:8083` and `user-tweaker` `:8084` (three front-doored
origins), `cupsd` on `:631` — which Home Assistant's `ipp` entry still dials at `10.3.1.30:631` —
this box's own **active** Prometheus (whose `hass` and `hackspaceapi` jobs now scrape services1),
and the docker bridge those containers live on. `hackspace-api.service` is likewise stopped,
disabled and parked in `/var/lib/systemd/disabled-units/`, with its unit and `/srv/hackspaceapi`
kept as the rollback. **One thing found there should be fixed:**
`/srv/prometheus/scrape_configs/uptimekuma.yml` carries a plaintext `admin` password for an app
this box has had masked since 2026-11-01 — see `docs/follow-ups.md`.

**Three of those are parked now, and `vulcanbot` is the newest.** `vulcan-bot.service` was
stopped, disabled, **file-removed from `/etc/systemd/system` and masked** on 2026-11-16 —
replaced in the workspace by services1's `slack-agent` (`docs/slack-agent-2026-11-16.md` §14 owns
the receipts and the reasoning, including why `disable` alone was not enough: the ask was "stay
disabled on reboot", and a mask is a `/dev/null` symlink that fails loudly where a symlink can be
re-created). Its unit text is parked here in the off-flake form of rule 23,
`/var/lib/systemd/disabled-units/vulcan-bot.service`, with a `# DISABLED — <date> — <reason> —
restore with:` header, sitting next to the one this section already knew about
(`uptime-kuma.service.2026-11-01`) — so `ls /var/lib/systemd/disabled-units/` on that box *is*
the list of what is parked there. Checked before stopping rather than after: the container's `HostConfig.RestartPolicy`
is `no`, no timer or cron names it, and nothing else in the tree does either. The container
(`Exited (0)`) and `/srv/vulcanbot` are **kept** — that is the rollback, for a week. What it
used to do that is **not** running anywhere: the MQTT `access/log` → `#doors` feed, dropped by
decision the same day ("doors not needed"). Its Monday event digest was ported to services1's
`slack-agent-digest.timer` first — §14 of that doc owns both.

The older parked pair, and the thing that makes that look confusing is Home
Assistant's MQTT dial (measured 2026-11-02). `mosquitto.service` and `zigbee2mqtt.service`
both read `is-enabled=disabled`, `is-active=inactive` on apps1, and nothing on apps1 listens
on 1883 (`ss -lntp | grep 1883` is empty there): the live broker is services1's podman
`eclipse-mosquitto` (`machines/services1/services/mqtt.nix`) and the live zigbee2mqtt is
services1's podman container too. Home Assistant's MQTT config entry dials
`mqtt.int.leighhack.org` — a `legacy` row in `common/int-dns.nix`, i.e. "answers with the
front door, served by no vhost" — so since the 2026-10-24 front-door move it dials `gw`,
where nothing listens on 1883, and every Frigate **camera** entity in HA renders Unavailable
while Frigate's HTTP-fed **sensors** keep counting. The broker's own journal is the dated
proof of who connected and when. Receipts, the repair (all three dials retargeted to the
broker's address inside off-flake runtime files) and the still-open declarative decision:
`docs/mqtt-int-legacy-answer-2026-11-02.md`. The *other* HA-on-apps1 fault of the same week —
HA core `2026.10.4` turning the Frigate integration's deprecated `via_device` into a fatal
error, and the unpinned `:stable` tag that moved the box there — is
`docs/homeassistant-frigate-via-device-2026-11-02.md`. Both are runtime state no guard reads
and no `git log` line touches: fix them on the box, and re-read this section's date before
trusting which box's broker is which.

**Uptime Kuma is permanently disabled there (2026-11-01).** It was superseded on
2026-09-24 (`e84f223`, "gatus: replace Uptime Kuma as the status page") — that change
removed the `uptime-kuma.int` vhost and left the DNS record to be pruned — but the app
was **still running and still answering**, unauthenticated, on
`http://10.3.1.30:3001`, and its own dashboard was doing a second, older copy of gatus's
job (`Cameras`, `Unifi Controller`, `3D-1..3`, …) alerting into the same Slack webhook.
Off the network's side nothing pointed at it any more: `getent hosts
uptime-kuma.int.leighhack.org` on services1 is empty, and no `.nix` in the tree mentions
`kuma` or `3001` outside three historical comments in
`machines/services1/services/{gatus,monitoring}.nix` and
`common/monitoring-endpoints.nix`.

The unit was `/etc/systemd/system/uptime-kuma.service`:

```ini
ExecStartPre=-/usr/bin/docker rm -f uptime-kuma
ExecStartPre=/usr/bin/docker pull docker.io/louislam/uptime-kuma:latest
ExecStart=/usr/bin/docker run -p 3001:3001 -v /srv/uptimekuma:/app/data -v /var/run/docker.sock:/var/run/docker.sock --name uptime-kuma docker.io/louislam/uptime-kuma:latest
Restart=always; WantedBy=default.target; Requires=docker.service
```

Nothing had `RequiredBy` on it, so taking it down is one box-local decision. What was run
(one ssh, in order — `mask` refuses while the unit file exists, so the file goes to the
registry first):

```bash
systemctl stop uptime-kuma.service            # foreground docker run: the container stops with the unit
systemctl disable uptime-kuma.service         # removes default.target.wants/<unit>
rm /etc/systemd/system/uptime-kuma.service    # already copied to the registry below
systemctl mask uptime-kuma.service            # -> symlink to /dev/null: nothing can start it, even by hand, even after a reboot
```

Measured after: `is-enabled` = `masked`, `is-active` = `inactive`, no listener on
`:3001` (`ss -lntp`), no container (`docker ls`), and `curl -m 5 http://10.3.1.30:3001/`
from services1 answers `rc=7` (connection refused). The replacement answers:
`http://127.0.0.1:8999/` on services1 is 200, i.e. gatus is serving the status page.
`/srv/uptimekuma` — the bind mount that holds `kuma.db` — was **left in place**, so the
data is still there if it is ever wanted again.

**The unit file is parked in a registry, not deleted**:
`/var/lib/systemd/disabled-units/uptime-kuma.service.2026-11-01`, whose first line is
`# DISABLED — <date> — <reason> — restore with: …`. That directory is the off-flake
equivalent of this repo's rule-23 `# PARKED` habit:
`grep -rl '^# DISABLED' /var/lib/systemd/disabled-units` *is* the list of what this box
decided not to run. Un-masking alone would not bring kuma back — the unit file has to
come back first.

Left alone on the box, on purpose: the enabled nginx vhost
`/etc/nginx/sites-enabled/uptime-kuma.int.leighhack.org.conf` and its
`/etc/letsencrypt/renewal/uptime-kuma.int.leighhack.org.conf`. The name no longer
resolves, so the vhost is inert and it is the app being gone that makes it serve nothing.
`grafana.db` and Prometheus's segment indexes still contain the kuma strings — that is
scraped history, not a dependency on the app.
