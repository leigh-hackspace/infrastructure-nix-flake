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
