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
`usertweaker`, `vulcanbot`, `zigbee2mqtt`, `hackspaceapi`, `home`). None of it is
declarative; the flake only points at this box.

**Two of those are parked, and the thing that makes that look confusing is Home
Assistant's MQTT dial (measured 2026-11-02).** `mosquitto.service` and `zigbee2mqtt.service`
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
