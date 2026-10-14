# UniFi OS Server on NixOS

Investigated 2026-10-14. Summarises what Ubiquiti's "UniFi OS Server" actually is,
how we build it deterministically, what it takes to run it, and the shape of the
migration off the linuxserver.io container that services1 runs today.

Status: **cut over on 2026-10-14 — the appliance is the live controller.** The
old controller is parked, not deleted: its containers, images and data
(`/srv/unifi`, `/srv/unifi-db`) are untouched and its units are un-wanted, so
rollback is two edits and a switch — see [Taking it back out](#taking-it-back-out).

- Service: `machines/services1/services/unifi-os-server.nix`
- State: `/srv/unifi-os-server/` (borg-backed with the rest of `/srv`)
- UI: `https://unifi-admin.int.leighhack.org` (nginx → `127.0.0.1:11443`, LAN ACL),
  or directly on `https://10.3.1.20:11443`.
- Done: image pinned and loaded, Network bumped 10.5.67 → 10.6.106, setup wizard,
  encrypted backup restored, four APs adopted and connected, old controller
  stopped, `unifi-admin.int` pointed at the appliance.
- Known loose end: the appliance's own `updater-service` wanting to self-update.

## Why anyone is looking at this

Ubiquiti's download page tells self-hosters:

> Going forward, we recommend users upgrade to UniFi OS Server for all self-hosted
> deployments. It provides the full UniFi OS Platform experience…

That is a **recommendation, not an expiry**. It is not a security notice and no
current functionality stops. The standalone Network Application still ships
(10.6.106, 2026-09-15) and services1's container was built 2026-10-13, i.e. it is
current. The reason to plan the move is that the standalone package is the thing
Ubiquiti will eventually stop shipping — a "next few months" decision, not a
"tonight" one.

Worth being clear about what the two files Ubiquiti publishes are, because they
are not two versions of the same thing:

| Artifact | What it is |
| --- | --- |
| `unifi-uos_sysvinit.deb` | **UniFi Network Application** 10.6.106, the Java app, packaged for a Debian-ish host (needs JRE 25, MongoDB ≥3.6 <5.0). Also the payload Ubiquiti's own updater installs *into* an appliance. |
| `5172-linux-x64-5.1.42-….42-x64` | **UniFi OS Server** 5.1.42, the appliance: an installer carrying a whole container image. |

## What the appliance is

The installer is a self-extracting ELF (Rust, `rustc 1.97.1`) with its payload as
a zip appended at offset **4751312**, nine entries:

| Entry | Role |
| --- | --- |
| `image.tar` (776 MB) | OCI archive — the appliance itself |
| `uosserver` | CLI: `install`, `uninstall`, `start`, `stop`, `status`, `update`, `logs`, `support`, `shell`, `web` |
| `uosserver-service` | systemd service; the supervisor. `podman run --privileged`, waits for UniFi OS to come up, `podman exec`s into the container for `shell`/`web` |
| `updater-service` | polls Ubiquiti's download service, downloads new images, verifies, hands them to the installer |
| `purge`, `discovery` | cleanup; device discovery (`uos-discovery-client` is a stub in this image) |
| `pasta` | Ubiquiti's fork of `passt`/`pasta` — userspace port forwarding, with `--map-host-loopback`, `--no-tcp`, `--no-dhcpv6`, `--ns-ifname tap0`; `slirp4netns` as fallback |
| `portmap.json`, `mounts.json` | the ports it publishes (3478/5514/5671/6789/8080/8443/8444/8880/8881/8882/28082/9543/11084/10003/8081/1900…) and the bind mounts it makes |

The vendor flow: installer creates a dedicated **`uosserver` user** (the binaries
refuse to run as root and `uosserver-service` hard-requires that uid), runs the
container as it, and publishes ports through pasta rather than netfilter.

Inside `image.tar` — one image, `docker.io/library/uosserver:c9603dec9010`,
created 2026-09-08, entrypoint `/root/uos-entrypoint.sh`:

- **Debian 11 bullseye** (EOL) with **systemd as PID 1**. The entrypoint chowns
  `/persistent`, runs `systemd-tmpfiles`, fixes `/etc/hosts` and `/etc/resolv.conf`
  (podman's symlinked `/etc/resolv.conf` breaks `systemd-resolved`'s daemon-reload),
  then `exec /lib/systemd/systemd`.
- **`unifi-core` 5.1.132-uosserver.1** — the UniFi OS platform (the thing that
  serves the UI on 443 and does adoption/setup).
- **UniFi Network Application 10.5.67-35187-1** — *older* than the standalone
  package of the same era, and older than what services1 runs.
- **Ubiquiti MongoDB 3.6.23-ubnt** (own build, not Debian's), rabbitmq, postgres
  14 + 16, nginx, `temurin-25-jre`, `ubnt-dpkg-daemon` (in-container package
  management: this is how the appliance updates its own apps).
- Env it expects: `APP_MODEL=UOSSERVER`, `PRODUCT_NAME=uosserver`,
  `FIRMWARE_PLATFORM=linux-x64`, `UOS_SERVER_VERSION`.

## Deterministic build

Nothing here is compiled, so "deterministic" means *the build is a pure function
of a pinned vendor blob, and fails loudly if the blob changes* — three separate
links in that chain, and it is worth knowing which ones are actually solid.

**1. Discovery.** `https://download.svc.ui.com/v1/downloads/products/slugs/unifi-os-server`
is a JSON API (`name`, `version`, `file_url`, `date_published`) listing every
installer Ubiquiti publishes — currently 59 entries, latest UniFi OS Server
**5.1.42 (2026-09-10)**, plus the `UniFi Network Application … for UniFi OS`
.debs. This is what a bump script should read; there is no other machine-readable
source. Filenames are `fw-download.ubnt.com/data/unifi-os-server/<hex>-linux-x64-<version>-<uuid>.<version>-x64`
and are stable per release, so a `fetchurl` pin does not rot under a re-upload.

**2. Pinning.** `fetchurl` with the SRI hash. Our copy of the 5.1.42 x64 installer
hashes to `sha256-9hEek5akLHQBb13emwH770hqb+ae/hN5Nebji3wi+U0=`, byte-identical to
the pin in the reference packaging, which is the check that a downloaded file is
the published one rather than "a file that installs".

**3. Extraction.** `bsdtar -xf "$installer" image.tar` — libarchive finds the zip's
central directory through the ELF prefix and extracts one member. Verified
byte-identical to an independent inflate of the same entry
(`image.tar` = `sha256 b0a3e0a3fc2d2644062db4f7950fccfe0bc24407c6e9c565da9860a37217e2f7`).
The reference packaging carves with `binwalk -e` instead, which works but drags a
Python package into a `nativeBuildInputs` for no gain.

**4. Load.** The OCI archive is content-addressed, so `podman load` yields the same
image ID (`sha256:8950cb90…`, the config digest) on every machine at every time.
This link is genuinely deterministic — it is the *only* part of the pipeline that
is not taking Ubiquiti's word for something.

**What determinism does not reach**, and no amount of Nix changes it:

- The image's contents are closed Ubiquiti `.deb`s. We can hash them, never
  reproduce them.
- `updater-service` and `ubnt-dpkg-daemon` exist to mutate the running system:
  pull a new image, swap it, update apps in place. A declarative host and an
  appliance that updates itself are two writers to one state. We pin the image and
  let the flake bump it; the in-container updater is the thing to distrust, not the
  fetch.
- The Network Application inside the image **lags** the published one (10.5.67 vs
  10.6.106 at time of writing), so "which version do we run" has two answers: the
  image pin and whatever the appliance has been talked into installing since. The
  second one lives in container state, not in the flake.
- Store cost: ~1.6 GB per version (786 MB installer + 776 MB `image.tar`). GC
  handles it; a version bump is not free.
- `update-containers.service` (the nightly image updater) runs `podman pull`
  against every running container. For this locally-loaded image the pull fails
  harmlessly and the digest is unchanged, so it does not restart anything — but it
  will log a failure every night.

## Running it

`oci-containers` + `imageFile` (load from the store, never pull), `privileged`,
`--systemd=always`, `--pids-limit=8192`, `--add-host=host.docker.internal:host-gateway`.
State is seven bind mounts under one directory:

| Host | Container | | Host | Container |
| --- | --- | --- | --- | --- |
| `persistent` | `/persistent` | | `unifi` | `/var/lib/unifi` |
| `data` | `/data` | | `mongodb` | `/var/lib/mongodb` |
| `srv` | `/srv` | | `log` | `/var/log` |
| | | | `rabbitmq-ssl` | `/etc/rabbitmq/ssl` |

Four things the appliance assumes that a container does not give it, each fixed by
a systemd drop-in mounted into the container (these are the ones that cost
someone hours; they are all silent failures):

1. **`timedatectl` must report a synchronised clock** or `unifi-core` waits
   forever → a fake `unifi-os-server-fake-ntp.service` `Type=oneshot`
   `RemainAfterExit` unit, registered in `/etc/systemd/ntp-units.d/`.
2. **`/data/unifi-core/config/http` must exist** before `unifi-core` starts →
   `ExecStartPre=-/bin/mkdir -p`.
3. **`/var/log/nginx` must exist** (it is inside the bind-mounted `/var/log`,
   which arrives empty) → same trick for `nginx.service`.
4. **MongoDB's datadir arrives owned by the host** because it is a bind mount →
   `ExecStartPre=+… chown mongodb:mongodb` (`+` = run as root, since the unit is
   `User=mongodb`).

Two identity details, both in `preStart`:

- `uos_uuid` is **derived from `/etc/machine-id`** (`uuidgen -s -n @dns -N
  "$(cat /etc/machine-id)"`), not random, so a recreated container keeps the same
  appliance identity and does not look like a new device to Ubiquiti's cloud.
- `system_ip` in `system.properties` is the address handed to devices as their
  inform URL (`http://<system_ip>:8080/inform`). On services1 that is
  `10.3.1.20` — the same address the old controller answers on, which is what
  makes a cutover a port change rather than a reconfiguration of every device.

## Getting the app version you actually need

The image pins Network Application 10.5.67, and Ubiquiti publishes 10.6.106 for
the same platform. The supported way to move it is **inside** the appliance:

```console
$ sudo podman exec unifi-os-server uos runnable latest-versions unifi
{"release":{"version":"10.6.106-36011-1","url":"https://fw-download.ubnt.com/data/unifi/f1ba-uos-deb11-amd64-10.6.106-36011-1-….deb"}}
$ sudo podman exec unifi-os-server uos runnable install unifi --version 10.6.106-36011-1 --progress
$ sudo podman exec unifi-os-server uos runnable current-version unifi
10.6.106-36011-1
```

`uos runnable` (`current-version`, `latest-versions`, `check-version`, `install`,
`uninstall`) is Ubiquiti's own installer front-end; `install` takes `--version`,
`--direct-url`, `--download-only` and a `--max-release-channel`, and it is what
the appliance's update flow uses under the hood. Verified on the trial: the bump
leaves `unifi` and `unifi-core` both `active` inside the container, no restart of
the container needed.

Two consequences worth sitting with. The `.deb` Ubiquiti publishes as "for UniFi
OS" is the same artifact through a different door — `dpkg -i` on it inside the
container also works, which is why that file exists — but `uos runnable` resolves
dependencies and talks to the release service, so prefer it. And **this is state,
not build**: the flake pins the image, the appliance's own package database
records which runnables have been installed over it, and nothing in the flake
shows that we are running 10.6.106 rather than the image's 10.5.67. Recreating the
container from the pinned image reverts the app version while keeping the data
directory, which is a way to get an inconsistent pair; if the app version ever
matters, record it next to the pin.

## Migration

The one fact that decides the order of everything: **the appliance is a
downgrade.** Network 10.5.67 + MongoDB 3.6 cannot restore a backup written by
Network 10.6.106 + MongoDB 4.4 — the restore is refused, not silently mangled.
So:

1. Bring the appliance up **beside** the old controller, on offset ports, so the
   old one keeps serving the network the whole time.
2. Complete the UniFi OS setup wizard. This does **not** need a Ubiquiti account:
   the wizard offers *Proceed without a UI account → Continue anyway*, then a
   local console password. (The reference packaging's VM test blackholes
   `account.ui.com`, `sso.ui.com`, `unifi.ui.com` in `extraHosts` specifically to
   prove it works with no account and no cloud.)
3. **Bump Network inside the appliance to match or exceed the backup** — that is
   what the `unifi-uos_sysvinit.deb` ("for UniFi OS") is: the same dpkg Ubiquiti's
   own updater installs. Until this is done, no restore is possible.
4. Restore the old controller's `.unf` (autobackups live in
   `/srv/unifi/data/backup/autobackup/`; snapshot taken at
   `/srv/unifi-pre-uos-2026-10-14/`).
5. Verify in the appliance that sites, clients, port profiles and device config
   came back.
6. **Cutover**: stop the old containers, move the appliance's published ports from
   the offsets to the real numbers (8080, 8443, 6789, 8880, 8843, 28082, 3478/udp,
   10001/udp), restart. Devices re-inform at the same `system_ip`; nothing on the
   devices changes.
7. Only then retire `/srv/unifi`, `/srv/unifi-db` and the old vhost.

### Backups are encrypted, and the key is not on disk

A `.unf` from a current controller is not a tarball or a gzip — the first bytes
are high-entropy with no magic, because autobackups are **encrypted with the
site's backup key**. There is no `secret.key` anywhere under `/srv/unifi`: the key
lives in the controller's database and is only ever materialised when someone
downloads it from the old controller's UI (Settings → System → Backups → *Backup
Encryption Key* → Download). A restore of an encrypted `.unf` asks for that file,
so **download it from the old controller before touching the restore**, and treat
it as a credential: it decrypts a file containing every device PSK, port profile
and local credential in the network.

This is also why "we have nightly borg of `/srv`" is not a backup strategy for the
controller on its own: it is a backup of the *running* controller, and the
appliance can only be fed a `.unf` plus the key.

### The setup wizard, and why it was not scripted

The wizard's own API is discoverable from the setup bundle it serves
(`/main.*.js`): `POST /api/setup/start` (returns `setupId`), `POST /api/setup`
(the form: console name, `password`, `ssoUsername`, `tosAndEula`),
`POST /api/backup/upload` and `POST /api/backup/setup/restore` for restoring
during setup, `POST /api/auth/login` afterwards. All of it is reachable
unauthenticated before the console is set up (405s, not 401s), which is how the
reference packaging's Selenium test drives it.

It was left interactive for two reasons, not because it cannot be driven: the
console password should be chosen by whoever owns the console rather than minted
by an agent or a script, and a half-driven wizard can leave the appliance in a
partially-configured state that is harder to read than a browser showing the step
it stopped on. It is five clicks.

Things that are *not* migrations and get mistaken for them:

- Adopting devices by hand onto the new controller while the old one runs: both
  controllers then fight over the same devices.
- Copying `/srv/unifi/data` into the appliance's `/var/lib/unifi`. Different
  MongoDB versions and a different app layout; the `.unf` restore is the supported
  path.
- Treating "the UI loads" as migrated. The UI loading means unifi-core is up; the
  Network Application is a separate service inside the container.

## Where the trial stands

Deployed 2026-10-14 21:22, confirmed in the same invocation
(`/run/current-system` == `system-good`). Observed, not inferred:

- `podman-unifi-os-server.service` active, **0 restarts**; container `Up`,
  `systemd` is PID 1 inside it, no failed units.
- Running inside: `mongodb`, `nginx`, `postgresql@14-main`, `rabbitmq-server`,
  `epmd`, `ucs-agent`, `unifi-directory`, `unifi-identity-update`, `uos-agent`,
  `uos-discovery-client`, plus `unifi-core` and `unifi`.
- The four drop-ins are applied — `unifi-core.service` reports
  `/etc/systemd/system/unifi-core.service.d/prestart-fix.conf` in its `Drop-In`
  list, and its `ExecStartPre` shows the `mkdir -p /data/unifi-core/config/http`
  line running; the fake NTP unit is `active`. Without them the container sits at
  "starting" forever.
- `https://10.3.1.20:11443/` → **200**, serving `window.UNIFI_OS_MANIFEST` with
  `"model":{"id":"UOSSERVER"…}`. Measured footprint of the whole appliance
  container: **1.46 GB** (`unifi` 799 MB, `unifi-core` 181 MB, `mongodb` 71 MB —
  `unifi-core` is capped at 1.0 G by an in-image drop-in), against 7.6 GB
  available on services1 with the old controller still running. Budget roughly
  2 GB for a settled appliance.
- Network Application bumped to **10.6.106-36011-1** with
  `uos runnable install`, matching the backup.
- Old controller unaffected throughout: `podman-unifi` and `podman-unifi-db`
  active, `https://127.0.0.1:8443/` → 302, `http://127.0.0.1:8080/inform` → 400
  (which is the correct answer to a GET with no device payload).

Done by hand, in this order: setup wizard (no UI account, local console
password), backup encryption key downloaded from the old controller, restore of
`autobackup_10.6.106_20261012_0030_….unf`, then the cutover below.

## Cutover (2026-10-14)

The appliance restored the backup and showed the four APs — **offline**. That is
the correct behaviour of a controller that cannot be reached, not a broken
restore: devices are configured to inform `http://10.3.1.20:8080/inform`, and
while the appliance published `18080` its `access.log` had **zero** `/inform`
entries. `system_ip` is `10.3.1.20` for both controllers, so the network belongs
to whichever process holds host 8080, and both cannot. Cutover is therefore:
publish the real ports, stop the old controller, and let the devices walk over.

Two lessons, both expensive to rediscover:

1. **`autoStart = false` does not stop a running container.** Setting it and
   switching left `podman-unifi` running and holding 8080 — `switch-to-
   configuration` drops the `wantedBy` but does not stop a unit that is merely no
   longer wanted — while `podman-unifi-os-server` sat `activating`, unable to
   bind, restarting. The units had to be stopped explicitly:
   `systemctl stop podman-unifi podman-unifi-db`. Until that is done the
   appliance cannot take over, and the symptom (appliance `activating`, devices
   offline) points at the wrong box. A `Conflicts=` on the appliance unit would
   make this declarative; not done yet.
2. **Verify with the appliance's own logs, not the old controller's.** The
   Network Application inside the appliance logs to `/data/unifi/logs/`
   (`server.log`, `access.log`), *not* `/var/log/unifi/`, and its MongoDB is on
   **27117**, not 27017 (27017 is a second, near-empty instance). The old
   controller logs to `/srv/unifi/logs/` on the host.

After stopping the old controller, within about two minutes:

```console
# informs arriving, and from whom
timeout 60 sudo podman exec unifi-os-server \
  sh -c "grep /inform /data/unifi/logs/access.log | tail -3"
[2026-10-14T22:24:48,215+01:00] [ 10.3.1.43 -> inform-1 ] POST 200 24ms /inform

# adopted, and connected right now
timeout 90 sudo podman exec unifi-os-server sh -c 'mongo --port 27117 ace --quiet \
  --eval "var now=Math.floor(Date.now()/1000); db.device.find({},{name:1,connected_at:1,last_seen:1,adopted:1})
            .forEach(function(d){print(d.name+\" adopted=\"+d.adopted+\" connected_ago_s=\"+(now-d.connected_at))})"'
AP2 - Pi Room adopted=true connected_ago_s=34
AP4 - Social adopted=true connected_ago_s=150
AP3 - Fabrication adopted=true connected_ago_s=152
AP1 - Main Space adopted=true connected_ago_s=33
```

Four U7LT/U7PG2 access points, `adopted=true` from the restored backup, all
connecting within the first two minutes. The `ERROR sdn - Failed to execute cloud
proxy GET request: ztp/consoles/auto-adoptions` lines in `server.log` are the
cloud auto-adoption poll failing because there is no Ubiquiti account; harmless,
and expected for this install.

## Taking it back out

The appliance is additive: delete the `./unifi-os-server.nix` line from
`machines/services1/services/default.nix`, `just switch`, and the container is
removed. `/srv/unifi-os-server` is state and stays until deleted by hand.

Since the cutover, backing out is two steps longer: the same edit also removes the
`autoStart = lib.mkForce false` overrides **and the `unifi-admin.int` vhost**, both
of which live in this file now — so re-add the vhost to `./unifi.nix`
(`proxyPass = "https://127.0.0.1:8443"`) or the domain 404s. Then, per lesson 1
above, the switch will **not** start the old controller while the appliance is
running, and the two still contend for 8080: stop the appliance first
(`systemctl stop podman-unifi-os-server`), switch, then `systemctl start
podman-unifi` if it did not come up by itself. The old controller's data was never
modified, so this is a restart, not a restore.

## Open questions

- ~~Does the 10.6.106 `.unf` restore into the appliance's MongoDB 3.6?~~ **It
  does.** After bumping Network with `uos runnable install`, the encrypted backup
  from the 4.4-backed controller restored cleanly, sites and device config
  included, and all four APs came back `adopted=true`. The version gate is on the
  Network Application, not on the MongoDB wire version, and the restore replays
  the dump through the appliance's own 3.6 tooling.
- ~~Is the UniFi OS UI proxyable?~~ **It is**, and `unifi-admin.int.leighhack.org`
  now fronts it. Verified after the switch: page 200 carrying `UNIFI_OS_MANIFEST`,
  a 1 MB JS bundle served byte-for-byte, and `/proxy/network/ws` answering 401
  through the proxy exactly as it does direct — pass-through working, not failing.
  Three things make it work and none are optional:
  - **`websockets = true`.** The UI opens
    `new WebSocket("wss://" + window.location.host + path)`, so it dials *the
    proxy*, and without the Upgrade dance the dashboard loads and never updates.
  - **Long timeouts.** nginx's 60 s `proxy_read_timeout` drops an idle websocket,
    which looks like a dead UI until you reload; `timeouts = "3600s"`.
  - **Upstream TLS left alone.** nginx verifies nothing and sends no SNI by
    default, which is what you want against the appliance's self-signed cert. Set
    `proxySSLVerify = true` and this breaks.

  There is **no host validation to fight**: the appliance answers 200 for any
  `Host`, and every asset URL is root-relative, so nothing needs rewriting. The
  fiddly part was finding the websocket path — every path on the appliance returns
  the SPA's catch-all 200, so existence means "the one that doesn't", and
  `/proxy/network/ws` answers 401.
- ~~Do we want the UniFi OS UI behind nginx-sso?~~ Not currently, and note
  `unifi-admin.int` was **never** behind it: `mkIntVhost` is the LAN-ACL vhost, the
  SSO one is `sso-vhost.nix` for public apps. So the controller's front door is
  "reachable from the LAN and the tailnet ACLs, no second factor". If that ever
  needs changing, the SSO `auth_request` dance plus UniFi OS's own websocket and
  portal flows is the work, and it is not done here.
- `update-containers`' nightly `podman pull` failing on a locally-loaded image is
  harmless but noisy; either teach that script to skip `imageFile` containers or
  accept the log line.
- Whether to consume `github:rcambrj/unifi-os-server` as a flake input instead of
  our own module. Its module is the reference (MIT, weekly bumps, a VM test that
  drives the setup wizard with Selenium), but it hard-codes its own ports, wants
  `virtualisation.podman.enable` assertions, and its package build needs binwalk.
  Ours is 200 lines and fits the repo's service-file shape; the trade is that
  version bumps are ours to remember.
