# Monster — bringing the VMs back after a power cut (verified 2026-09-30)

Moved out of `AGENTS.md` on 2026-10-08; the short version and the landmines stay
there. Read this before touching a stopped VM.

Everything is slow right after a cold start because the NAS is still
importing and all VM disks are `cache=direct` qcow2s on `nas2-nfs`: measured
NFS READ RTT ~25 ms steady / up to 1.8 s during the start storm (a LAN NFS
read should be <1 ms), NAS `aqu-sz` ~6.5 and `%util` 82–84 % on the 7200 rpm
`sas-10k` disks. Expect guest boots to take many minutes and don't chase it as
a VM fault.

```bash
ssh -i ~/.ssh/agent-hop-key root@10.3.1.11
qm list                       # everything 'stopped'
pvesm status                  # nas2-nfs must say 'active' BEFORE starting anything
ha-manager status             # HA services may be in 'error' after 5 failed restarts
systemd-analyze blame | head  # host boot: ~44 s, of which pve-guests ~8 s on NFS
```

1. **Wait for the NAS export.** `pvesm status` / `mount | grep nas2-nfs` must
   show it online. NAS = `nas2` (10.3.1.6). `leigh-admin@10.3.1.6` is keyed
   with the machine-hop-key but has **no passwordless sudo**, so only
   `zpool list`, `lsblk`, `iostat`, `/proc/spl/kstat/zfs/...` work
   (`zpool status`/SMART need root — use the TrueNAS UI). Its shell is zsh:
   never `echo ===` (zsh reads `===` as command expansion).
2. **Clear HA error state — disable, *wait*, then start.** Doing both back to
   back in one loop races: the start is rejected with `service 'vm:127' in
   error state, must be disabled and fixed first`.

   ```bash
   for i in 108 109 127 128; do ha-manager set vm:$i --state disabled; done
   ha-manager status        # wait until all four report 'disabled'
   for i in 108 109 127 128; do ha-manager set vm:$i --state started; sleep 3; done
   ```

   Use `ha-manager set … --state started`, **not** `qm start`, for
   HA-managed VMs.
3. **Start the non-HA autostart VMs by hand:** `qm start 107; qm start 132`.
   During the storm `qm start` can fail with
   ``start failed: … failed: got timeout`` (qemu can't daemonize before the
   timeout while opening its qcow2). That is a pool-latency symptom, not a VM
   problem — just retry; 132 then started fine on the second attempt.
4. **Verify:** `qm list | grep -v stopped`, `ha-manager status` all `started`,
   then `qm guest cmd <id> ping` (agents answer late — `mercury` came up
   first while the rest were still booting).
5. **Leave the `onboot: 0` lab VMs alone for a bit** and start them one at a
   time (each is a burst of random reads against the slow pool); ~24 GB of
   the host's 32 GB RAM is already committed by the six infra VMs.
