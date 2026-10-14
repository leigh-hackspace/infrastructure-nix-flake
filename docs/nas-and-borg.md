# The NAS and the borg backup server

Moved out of `AGENTS.md` on 2026-10-14. `AGENTS.md` keeps the rules: the NAS
dependency rule (every service that needs it declares `after` **and** `requires`
`wait-for-nas.service` and keeps restarting forever), the automount gotcha, and
"never give up" (`TimeoutStartSec = 0` on the wait units must stay). This file is
the detail about the box itself and the backup pipeline.

## NAS

TrueNAS at **10.3.1.6** (`nas` in `common/hosts.nix`; `nas1`/10.3.1.5 does not
answer ping and its Proxmox NFS mount is disabled). NFS shares: `/mnt/cameras`,
`/mnt/filestore`, `/mnt/backups`. **It is slow to come up after a power cut** —
which is a fact about every dependent service, not an inconvenience: see the NAS
dependency rule and `docs/monster-proxmox.md`.

Everything about the NAS is out-of-flake, like the rest of the appliance.

### Passwordless sudo for `leigh-admin` (verified 2026-10-13)

`leigh-admin@10.3.1.6` is keyed with the machine-hop key and has passwordless
sudo — **set as the middleware user record** (`sudo_commands_nopasswd =
["ALL"]`, via the REST API on `/run/middleware/middlewared.sock`), *not* by editing
`/etc/sudoers`.

Why, because the obvious approach is a trap: middleware regenerates
`/etc/sudoers` from `middlewared/etc_files/local/sudoers.mako` at boot, so a hand
edit is wiped by the next `etc.generate('user')` — and the generated file has no
`#includedir`, so a `/etc/sudoers.d` drop-in is never read either.

Its shell is zsh: never `echo ===` (zsh reads `===` as command expansion).

## The borg backup server

An **Incus container on the NAS** named `borg-server` (out-of-flake):

```bash
ssh -i ~/.ssh/agent-hop-key leigh-admin@10.3.1.6
sudo incus exec borg-server -- <cmd>
```

- The repo dataset `/mnt/sas-10k/backups` is mounted in it as `/backups`.
- Its **proxy device forwards host 3022 → container 22**, which is the port the
  `borgbackup` job in `machines/services1/services/backup.nix` uses.
- The `backups` user inside it has **no forced command** in `authorized_keys`
  (single key, `backups@services1.int.leighhack.org`).
- borg there is a **standalone PyInstaller binary** at `/usr/local/bin/borg` — the
  image has no pip — so upgrading means dropping in the GitHub release asset
  (`borg-linux-glibc235-x86_64-gh`) and keeping the previous copy for rollback.
- Upgraded to **1.4.5** on 2026-10-13 (was 1.4.3). Container image is Ubuntu
  noble, serial `20251210_07:42`.

**Keep the server on the same 1.4.x line as the client.** nixpkgs ships borg
**1.4.5** on services1 and a borg 2.x `serve` cannot talk to a 1.4 client, so the
2.0.0bx releases are not an option while the job is a 1.4 client.

## Open hazards (as of 2026-10-13)

- **The 2026-10-12 job run failed and nothing recorded why.**
  `services1-backup-srv-2026-10-12T00:00:01.failed` is in the repo, and the journal
  for that run shows only two "file changed while we backed it up" warnings
  (`/srv/unifi/logs/tasks.log`, `/srv/frigate/config/frigate.db-wal`) before
  `status=1/FAILURE`. borg exits **1 for a fatal error and 2 for warnings**, so
  there was a real error the journal did not capture; the runs on 10-11 and 10-13
  are clean.
- **No `borg check` has ever been run** against the repo (422 GB compressed,
  ~1.0M chunks).
- **Nothing monitors the borg server.** `common/monitoring-endpoints.nix` has no
  check for `nas2.int:3022`, so a dead `borg-server` container would only show up
  as a failed nightly job.
