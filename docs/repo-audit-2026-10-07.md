# Repo audit — problems and improvements (2026-10-07)

A read-through of the whole flake (both machines, `common/`, and all seven Rust
crates) looking for duplication, correctness gaps, and things that could be
organised better. This is a findings list with a prioritised action order at the
end; it is a **living document** — items are struck through as they are done,
with what was actually done and how it was verified. Two passes have landed so
far (2026-10-08 and 2026-10-08b, see §7 for the second one's summary).

Method: `alejandra --check` over all `.nix`, `diff` between the per-machine
modules, line counts per crate, and a read of the rendered systemd units in the
store (not just the source) where quoting/secrets were in play.

Line references are as of the audit.

---

## 1. Repo-level quick wins

### 1.1 `just switch` / `just boot` do not confirm

The GOLDEN RULE in `AGENTS.md` is that `system.autoRollback.enable = true` rolls
the machine back to the last confirmed generation within a minute or two, so a
switch must be confirmed in the same shell invocation. The two recipes people
actually use don't do it:

```make
switch:
    sudo nixos-rebuild switch --flake .

boot:
    sudo nixos-rebuild boot --flake .
```

`switch-netboot` does chain `sudo nixos-confirm`. Make `switch` and `boot` chain
it too, so the rule is enforced by tooling rather than memory.

### 1.2 No CI, no `just check`

Nothing in the repo catches a broken module before deploy. A `just check`
covering `nix flake check`, `nixos-rebuild dry-run --flake .#services1` and
`#aibox`, `cargo clippy`/`cargo test` per crate, and the formatting check below
is the single highest-leverage addition.

Done 2026-10-08, extended 2026-10-08b. `just check` = `nix flake check` + the
assertion files (`lib/check-config-to-gitlab.nix`, `lib/check-printer-states.nix`)
+ `just fmt-check`. `just check-build` = `nixos-rebuild dry-build` for both
machines — the piece `flake check` cannot do, because it only *evaluates*: a
module that evaluates but fails to build used to reach `just switch` (that is
exactly how a wrong relative import in `dns-sync.nix` got committed). The Rust
side stays separate: `just test`, `just clippy`, `just filestore-test`.

### 1.3 Formatting is inconsistent (30 files)

`alejandra --check` reports 30 files needing formatting — **all** of
`machines/services1/`, all of `common/`, and `flake.nix`. Everything under
`machines/aibox/` is clean. So `.zed/settings.json`'s format-on-save is being
applied to one machine only.

Also: `.zed/settings.json` says alejandra, `.vscode/settings.json` says nixfmt.
Pick one. Reformat once in a single commit (so future diffs are not polluted)
and add `just fmt`.

Done 2026-10-08b. It had grown to **42** files. Reformatted in one sweep, and
made it stick: alejandra is in the devshell (the locked nixpkgs version, 4.0.0,
is the same one already on the running system), `just fmt` formats every tracked
`.nix`, `just fmt-check` fails on any that is not clean and runs as part of
`just check`, `.vscode/settings.json` now runs alejandra too, and `nixfmt` was
dropped from `common/tools.nix` so the two cannot drift. Verified behaviour-free:
built the services1 toplevel at HEAD and in the working tree and diffed the
rendered `/etc` — the only differences were the `nginx.conf` derivation hash and
trailing whitespace inside nginx comment lines.

### 1.4 Dead config and dead inputs

- ~~`flake.nix:4` — `nixos-hardware` is an input and is used nowhere.~~
- ~~`flake.nix:68` — `permittedInsecurePackages = [ "jitsi-meet-1.0.8792" ]`, but
  no jitsi service exists anywhere in the repo.~~
- ~~`machines/services1/services/default.nix` — three commented-out imports
  (affine, gitlab, samba); `machines/aibox/default.nix` has `# ./nvidia.nix`.
  Fine as parked config, but it should be labelled as parked.~~
- ~~`common/tools.nix` — `intel-gpu-tools` and `amdgpu_top` are installed on both
  machines (aibox is AMD, services1 is Intel).~~

Done 2026-10-08b. Input removed (`nix flake lock` prunes it without moving any
other pin); the jitsi exemption removed (`flake check` still passes, so nothing
else needs an insecure-package exemption); the parked imports are labelled
parked, with the caveat that a commented-out module is never evaluated and can
rot silently (gitlab and nvidia were already labelled); the GPU tools are now
per-machine — `intel-gpu-tools` on services1 (i915), `amdgpu_top` on aibox
(amdgpu/ROCm). Verified by evaluating `environment.systemPackages` on both
machines.

### 1.5 Repo hygiene / doc sprawl

~~`machines/aibox/gtx1060-*.md` (three files), `machines/services1/sso-redesign.md`,
`machines/services1/streams.ignore`, `machines/services1/README.md`.~~

`junk/`, `notes.txt`, `tmp/` and the `result` symlink are **git-ignored** (and
`streams.ignore` matches the `*.ignore` pattern), so they were never in the flake
source — checked, not a problem.

Done 2026-10-08b for the rest: the four narrative files moved to `docs/` (the
three gtx1060 write-ups and `sso-redesign.md`; the `ai.nix`/`whisper.nix`/
`nvidia.nix` comments that referenced them were updated), and `AGENTS.md` went
36.4 KB → 32.7 KB by moving detail out rather than deleting it — the Monster
power-cut runbook to `docs/monster-powercut-runbook.md`, the authentik OIDC
provider-creation detail to `docs/authentik-oidc-notes.md`, the filestore SPA
narrative to `docs/filestore-gotchas-2026-10-05.md` — keeping the landmines
inline with a pointer to the procedure. `machines/services1/README.md` had a
stale service list and a pre-sops "Required Secrets" listing; it now keeps only
what is not in the flake.

---

## 2. Duplication worth refactoring

### 2.1 Nix

| Duplication | Where | Fix |
|---|---|---|
| ~~15 identical settings: timezone, the whole `i18n.extraLocaleSettings` block, xkb layout, `console.keyMap`, `security.sudo.wheelNeedsPassword`, `programs.nix-ld`, `services.openssh`, `experimental-features`, `system.autoRollback`~~ | ~~`machines/services1/configuration.nix`, `machines/aibox/configuration.nix`~~ | Done 2026-10-08: `common/base.nix` imported by both. Per-machine files keep only `stateVersion`, `sandbox`, `boot.extraModulePackages`, aibox's autoLogin/firefox — and the boot loader moved to the shared file because it is identical on both. Verified by diffing the evaluated options (timezone, i18n, xkb, keymap, sudo, nix-ld, ssh, bootloader, kernelParams, sandbox, autoLogin, firefox, stateVersion) against the pre-change tree: identical. |
| ~~`wait-for-nas` service, verbatim except the mount list~~ | ~~`machines/services1/nfs-client.nix`, `machines/aibox/nfs-client.nix` (the aibox file says "This mirrors services1")~~ | Done 2026-10-08: `common/nas.nix` (imported for both machines) generates the `systemd.mounts`, the matching `systemd.automounts` and `wait-for-nas.service` from one `infra.nas.exports` list declared per machine in `hardware-configuration.nix` — the two halves can no longer drift. Both `nfs-client.nix` files are deleted (services1's carried ~35 lines of commented-out mounts for the old 10.47.49.x NAS; dropped). One real fix fell out: the hand-written `after = [ "mnt-ds-photos.automount" ]` named a unit that does not exist — systemd escapes the dash, the unit is `mnt-ds\x2dphotos.automount` — so that ordering was silently a no-op; the generated name is escaped properly. |
| ~~podman + never-give-up block, identical~~ | ~~`machines/services1/containers.nix`, `machines/aibox/containers.nix`~~ | Done 2026-10-08: moved to `common/containers.nix` (imported for both machines in `flake.nix`); aibox's file is gone and services1's keeps only the container-update timer. Checked the audit's question first: `nixos-utils.nixosModules.containers` is the nightly **image update** timer (`system.updateContainers`, `update-containers.service`/timer), **not** a restart policy — so the never-give-up block is not redundant, it just had to be written once. Verified podman options, the container set and every `podman-*` unit's restart settings are unchanged. |
| ~~NAS IP `10.3.1.6` hardcoded inside a **shared** module~~ | ~~`common/tools.nix:59`~~ | Done 2026-10-08: `infra.nas.host` (option, default `10.3.1.6`) is used by the generated share devices and by `wait-for-network`'s ping. Still hardcoded in `status-dashboard/src/main.rs:30`/`:632` (see the Rust row below). |
| ~~18 vhosts repeating `useACMEHost` + `forceSSL` + `recommendedProxySettings` + `CONFIG.LOCAL_NETWORK`~~ | ~~`machines/services1/http.nix` (375 lines)~~ | Done 2026-10-08: `machines/services1/lib/nginx-int-vhost-helper.nix` (`mkIntVhost { proxyPass; acl ? CONFIG.LOCAL_NETWORK; websockets; bodySize; timeouts; proxyBuffering; proxyHeaderBuffers; extraConfig; serverAliases; }`). 24 vhosts across `http.nix`, `ai.nix`, `status.nix`, `filestore.nix`, `gocardless-dashboard.nix`, `monitoring.nix`, `frigate.nix`… converted (−130 lines); the rest keep their own shape (multiple locations, `root`, redirects, SSO). |
| ~~`client_max_body_size 1024M` hardcoded with a `# TODO: Make this configurable (Llama needs it)`~~ | ~~`machines/services1/lib/nginx-sso-helper.nix:14`~~ | Done 2026-10-08: `mkSSOVirtualHost { proxyPass; bodySize ? "1024M"; timeouts ? 3600; }` — the TODO is gone and the SSO vhosts that need something else say so. |
| ~~Same constant written twice, must be kept in sync by hand~~ | ~~`--max-upload 2G` vs `client_max_body_size 2048M` (`filestore.nix`)~~ | Done 2026-10-08: one `maxUploadBytes` in the `let`; the flag and `client_max_body_size` are both derived from it. |
| ~~`import ../../../common/crane.nix { inherit pkgs crane; }` repeated 7×~~ | ~~every crate module~~ | Done 2026-10-08: `common/crane-args.nix` injects `CRANE` as a module argument (built with each machine's own `pkgs`); modules just list `CRANE` in their args. |
| ~~`fix-nix-shell` defined twice~~ | ~~`flake.nix`~~ | Done 2026-10-08: the `fix-nix-shell` binding had already gone in an earlier cleanup; what was left was the same `{ nix.registry.nixpkgs.flake = nixpkgs; }` module spelled out inline in both machines' module lists. It is one `nix-shell-registry` binding in the `let` now. Evaluated `nix.registry.nixpkgs` on both machines after the change: `exact = true`, `from = nixpkgs (indirect)`, `to = cf5e7650…` — unchanged. |

### 2.2 Rust

Done 2026-10-08 (all five dedupe items; the shared code lives in `common-rs/`,
and `common/crane.nix` grew the plumbing that makes in-repo path dependencies
work in a Nix build — see the comment there).

- ~~**`filestore/src/auth.rs` ≈ `gocardless-dashboard/src/auth.rs`**~~ — was 262
  lines each, ~95% byte-identical (PKCE S256, unpadded base64url, sha256,
  pending map, session map, `begin_login`, token exchange, introspection,
  `Infra` group gate). Now `common-rs/oidc` (`common-oidc`): one `Oidc` type
  that owns the session and pending-login maps, configured per app by
  `OidcConfig { cookie_name, client_id, client_secret, authorize_url, token_url,
  introspect_url, redirect_uri, required_group }`. Both apps deleted their
  `auth.rs`; the HTTP layers only call `begin_login`/`finish_login`/
  `session_for_cookie`/`drop_session`/`purge_expired`/`cookie_name`. The group
  gate now **fails closed** when authentik returns no `groups` claim (the
  missing-`ScopeMapping` case in AGENTS.md) instead of letting the user through.
- ~~**`dns-sync/src/json.rs` is copied into `moonraker-exporter/src/json.rs`**~~
  — now `common-rs/json` (`common-json`), the superset of both (`at`, `as_f64`,
  `as_bool`, `as_u64`); both crates `use common_json as json;`.
- ~~**Three identical `build.rs`**~~ — now `common-rs/build-spa`
  (`common-build-spa`), a *build* dependency; each `build.rs` is a 6-line call
  to `common_build_spa::embed(Config { env_var, dist_rel, app })`. The
  generated accessor is `find_asset` everywhere (frigate-monitor's `find` was
  the odd one out).
- ~~**Three near-identical `frontend/build.sh`**~~ — one recipe,
  `common/frontend-build-spa.sh <wasm-package-name>`; each frontend keeps a
  3-line `build.sh` that calls it. One behaviour: the CLI version is read from
  that frontend's own `Cargo.lock` (no hardcoded 0.2.128 in the script) and a
  mismatch **fails** with a pointer to the devshell, because the glue is
  schema-versioned — the old "0.2.x is ABI-stable, warn instead" comment in the
  frigate-monitor and gocardless-dashboard copies was wrong (and gocardless's
  header even said it built frigate-monitor's bundle).
- ~~**`cargo-vendor-ua-patch.sh` exists twice**~~ — one copy,
  `common/cargo-vendor-ua-patch.sh`; `common/crane.nix` no longer sources a
  file out of `frigate-monitor/`.
- ~~**Hand-rolled HTTP plumbing** in `status-dashboard/src/main.rs` and
  `network-status/src/main.rs` (`handle_client`, `route`, `json_str`,
  content-type tables, percent-decode)~~ — done 2026-10-08 as `common-rs/web`
  (`common-web`): request-head parsing, `Request::path()` (query string
  stripped), `respond`/`reason_phrase`, `percent_decode`.  The two copies had
  drifted and the merged behaviour fixes both drifts — see the crate header.
  **Not** shared on purpose: the routing tables and `json_str` (byte-identical,
  but sharing it means deciding on a JSON API first).  `mime_of` in
  `common-rs/build-spa` and network-status' content-type table are still
  separate lists.

How the path dependencies work (new, worth knowing before adding a crate):
`path = "./common-rs/<name>"` in each `Cargo.toml`, plus a **git-ignored
symlink** `common-rs -> ../common-rs` in the crate directory (same convention
as `gocardless-dashboard/frontend/dto`). `just shared-rs` (or entering
`nix develop`) creates the symlinks; `CRANE.cached`/`CRANE.deps` take
`sharedCrates = ["oidc"]` and copy the real crates (crate build) or stub crates
with real manifests (dependency build) into the source tree, so a shared-crate
*edit* does not invalidate the vendored-dependency cache while a shared-crate
dependency *change* does.

---

## 3. Correctness and robustness

### 3.1 Six units have `Restart = "always"` but no `startLimitIntervalSec = 0`

This is exactly the failure mode the "never give up" policy exists to prevent:
systemd permanently stops a unit after 5 quick starts.

- ~~`machines/aibox/ai.nix`~~
- ~~`machines/aibox/whisper.nix`~~
- ~~`machines/aibox/netboot.nix`~~
- ~~`machines/services1/http.nix` (nginx-sso)~~
- ~~`machines/services1/services/door-entry-management-system.nix`~~
- ~~`machines/services1/services/printer-monitoring.nix`~~

Fix with a shared `mkNeverGiveUpService` helper so it cannot recur.

Done 2026-10-08: `common/systemd.nix` (reached by every module as `INFRA`
through the flake's `specialArgs`) provides `mkNeverGiveUp` (units written here)
and `mkNeverGiveUpOverride` (units defined by nixpkgs modules, `mkForce` so the
module's own policy loses). All six units above now use it, as do the units that
already got it right by hand (`status-dashboard`, `network-status`, `filestore`,
gocardless-dashboard, frigate-monitor, both `containers.nix` files) — the helper
is now the only place the policy is written.

A seventh case the audit missed: **nginx** on both machines. The nixpkgs module
sets `Restart=always` but leaves `startLimitIntervalSec=60`, so five quick
failures killed the reverse proxy for every app on the box. It is now forced to
the same policy in `common/tools.nix` (guarded on `services.nginx.enable`).
Checked by evaluating `systemd.services.<name>` for both machines: every unit
with `Restart=always` now has `startLimitIntervalSec=0`.

### 3.2 `gocardless-dashboard/frontend/dto` is a symlink and is git-ignored

~~`gocardless-dashboard/frontend/.gitignore` contains `dto`, and the working-tree
`dto` is a symlink to `../dto`. The flake works because
`machines/services1/services/gocardless-dashboard.nix` materialises a real copy
for the sandbox, but on a fresh clone `cd frontend && ./build.sh` fails.~~

Fixed 2026-10-08 the same way the new `common-rs/` path dependencies work:
`common/shared-rs-links.sh` (run by `just shared-rs` and by the `nix develop`
shellHook) recreates every in-repo symlink — `frontend/dto` and each crate's
`common-rs` — so a fresh clone builds with one command instead of a mystery.

### 3.3 `network-status/frontend/dist` is committed while the other three SPAs are ignored

Justified (Nix builds are offline and the npm deps aren't nixpkgs-cached), but
it means the committed bundle can silently drift from `src/`. Either add a check
that `dist/` matches a fresh local build, or state the exception once instead of
in three separate files.

Done 2026-10-08b: `just network-status-dist-check` rebuilds the bundle with npm
and fails if `dist/` has drifted (naming the changed files); `just
network-status-frontend` is the rebuild. The exception is stated once, in the
justfile comment, and `AGENTS.md` points at it. Ran it: the committed `dist/`
currently matches a fresh build.

### 3.4 Hardcoded key paths and TOFU host keys

- ~~`machines/services1/network-status.nix:80` hardcodes
  `/home/leigh-admin/.ssh/agent-hop-key`.~~
- ~~`dns-sync/src/main.rs:39` hardcodes the same path as a default.~~
- ~~Both use `StrictHostKeyChecking=accept-new`, so a router reinstall silently
  re-trusts a new host key. Consider a pinned `known_hosts` and a configurable
  key path.~~

Done 2026-10-08. Both router-ssh tools now connect with
`StrictHostKeyChecking=yes` against a pinned list, and both take
`--ssh-key`/`--known-hosts`:

- the pinned list is generated in the flake as `/etc/dns-sync/known_hosts`
  (all three key types the router offers) and network-status points at the same
  file rather than keeping its own copy;
- `just router-known-hosts` prints the current `ssh-keyscan` lines to paste in,
  so accepting a re-installed router is a deliberate edit;
- network-status' `--ssh-key` now comes from a `MACHINE_HOP_KEY` binding in the
  module (the key `common/users.nix` installs) instead of a per-service literal.

Verified end to end: `dns-sync check` against the pinned list resolves the
router's dnsmasq table normally, and with an empty or wrong list it fails with
"Host key verification failed" instead of connecting. Both binaries were
checked to contain no `accept-new`.

### 3.5 `filestore/tests/run.sh` writes to fixed `/tmp/filestore-test-*.log`

~~Two concurrent runs clobber each other. Use `mktemp`.~~

Done 2026-10-08: the runner creates one `mktemp -d` scratch directory (logs and
fixture inside), exports it to the suite as `FS_TEST_WORKDIR`, and removes it on
exit (`FS_TEST_KEEP=1` keeps it for poking around; `FS_TEST_ROOT` still
overrides the fixture location). The suite's throwaway env file moved into the
same directory. Verified: `just filestore-test` → 43 tests, 0 failures, and no
`/tmp/filestore-test-*` files left behind.

### 3.6 Printer state enums are duplicated by convention

`moonraker-exporter/src/main.rs` documents `klippy: 0 startup, 1 ready, 2 error…`
and says "keep these tables in sync with monitoring-dashboards.nix". Emit string
labels, or generate the mapping from one source, so the dashboards can't drift.

Done 2026-10-08b. One source of truth: the two `const KLIPPY_STATES` /
`PRINT_STATES` arrays in the exporter (`state_code` returns the index; the label
is already a `state=` tag). `machines/services1/lib/printer-states.nix` reads
those arrays out of the Rust source at eval time and exposes `.labels`,
`.mapping` (the Grafana value mappings) and `.code` (the Prometheus alert
numbers, which were hardcoded `== 2` / `== 3`). A reorder or a new state in Rust
follows through on the next switch; a shape change throws at eval time.
`lib/check-printer-states.nix` (in `just check`) pins the current numbering, checks
mapping/code round-trip and label hygiene, and greps both consumers to make sure
they still go through the generated tables. Negative-tested both ways; and built
services1 before/after: the alert rules and dashboard JSON are byte-identical
(same store path).

### 3.7 Three overlapping monitoring systems

Gatus (uptime checks), Prometheus alert rules, and the status dashboard all
model the same facts. Gatus's endpoint list is hand-maintained and duplicates
data `dns-sync` already derives from `config.services.nginx.virtualHosts`
(`machines/services1/dns-sync.nix`). Generating gatus endpoints from the same
attrset would keep them in step for free.

Addressed 2026-10-08b, **not** as proposed — generating the endpoint list is the
wrong fix and the reason is now written in the module: the list mixes
nginx-served apps with ICMP/TCP targets that are dnsmasq hosts rather than vhosts
(cameras, switches, printers, the NAS), it watches public names too, and every
entry carries its own group and status condition, so auto-generating would invent
checks for things that are not meant to be watched and silently change what
alerts fire. What *is* cheap to prevent is the drift that actually bites: an
https `*.int.leighhack.org` endpoint left behind when its vhost is renamed or
removed, alerting forever on a name that no longer exists. That is now a module
assertion, with the offending hosts named (negative-tested). The `*.int` name
derivation itself is shared: `lib/int-vhost-names.nix`, used by both dns-sync and
gatus, so the two cannot disagree about what "an internal app on this box" means.

Consolidating the three monitoring *systems* is still open and is a judgement
call, not a refactor.

### 3.8 Test coverage

Only `filestore` has tests (the Playwright suite). The highest-value untested
targets are `frigate-monitor/src/detect.rs` (682 lines of CV logic),
`filestore/src/fsutil.rs` (the `..` component guard) and the shared JSON parser.

Done 2026-10-08 / 2026-10-08b: `common-rs/json` has 12 unit tests (the
DigitalOcean and Moonraker response shapes, escapes, number grammar, malformed
input, control characters in strings) and `common-rs/oidc` has 11 (the RFC 7636
PKCE vector, the unpadded-challenge invariant, the authorize-URL contract,
state single-use, the group gate failing closed, session expiry/purge, the
Basic-credential alphabet). `detect.rs` gained 12 (shadow heuristic, `min_area`,
`connected_regions`, and the detector's promises: seeds on the first frame,
static change fires once with a tight box, moving object never fires,
whole-frame change reseeds, replaced/removed recorded objects fire again) and
`fsutil.rs` 13 (the nine `..` spellings, ordinary names that merely *look* like
traversal, list ordering, shallow vs deep search, both caps). `just test` now
runs 71 tests, 0 failures (was 46). Two behaviours are pinned rather than
"fixed" because they are deliberate: the search scan cap is checked per
directory, and `fsutil` follows symlinks that live inside the share — its module
doc used to claim otherwise and now says what is actually guaranteed.

---

## 4. Security smells

These are mostly deliberate tradeoffs, but they should be *decided*, not
inherited.

1. **The status-dashboard restart token is baked into a world-readable unit
   file.** Reading the rendered unit in the store:

   ```
   ExecStart=.../status-dashboard --bind 127.0.0.1 --port 8088 ... --restart-token 11d01e91...
   ```

   Anyone who can read `/nix/store` or `systemctl cat` on either machine can
   restart any unit. `common/sops.nix` documents the build-time-decryption
   tradeoff, but this value is a *capability*, not just a credential. Fix:
   `sops.secrets` + `EnvironmentFile=` for the binary, and have nginx read the
   injected value from a file — or drop the LAN-token path entirely and require
   SSO for restarts.

   **Decided 2026-10-08: keep it, and document it as a LAN convenience
   credential.** The token only reaches restarts through the LAN-only `*.int`
   vhosts (already ACL-restricted to the tailnet/LAN ranges) and the dashboard
   itself binds to loopback; anyone with store read access on either machine
   already has root there, i.e. they can restart the units directly. What was
   done instead: the exposure is stated at the option and in `common/sops.nix`,
   the comparison is now byte-wise rather than short-circuiting, a missing token
   is logged instead of silently disabling restarts, and the secret is written
   without a trailing newline so the header/comparison can actually match
   (commit "status-dashboard: make the LAN restart token actually verifiable").
   If it ever needs to be a real capability, the fix is unchanged: `sops.secrets`
   + `EnvironmentFile=` for the binary and an nginx `set $token` read from a
   file, or drop the LAN path and require SSO.

2. ~~**`FRIGATE_RTSP_PASSWORD = "password"`**~~
   (`machines/services1/services/frigate.nix`) — ~~hardcoded weak secret in a
   world-readable store path.~~ **Decided 2026-10-08: it is a placeholder, not a
   secret.** Frigate's own config (`/srv/frigate/config`, outside the flake)
   overrides it at runtime and the go2rtc endpoint it guards is LAN-only; the
   value is now commented as such so nobody mistakes it for a credential.  If it
   ever has to be real, it belongs in `CONFIG.ENV_FILE`/a sops secret.

3. ~~**`--privileged` on the frigate container**~~ while the explicit
   `--device`/`--cap-add=CAP_PERFMON` list is already present. **Decided
   2026-10-08: kept, and commented.** Frigate's documented podman setup asks for
   it (camera/GPU/USB passthrough); tightening it to an explicit device list is
   the right change but breaks detection when the list is wrong, so it is now
   recorded as a tradeoff rather than left as an unexplained extra flag.

4. ~~**`borgbackup` `encryption.mode = "none"`**~~
   (`machines/services1/services/backup.nix`) — ~~acceptable if the repo is
   access-controlled, but it deserves an explicit comment saying so.~~ Done
   2026-10-08: the comment states the trust boundary (NAS share + `backups` ssh
   user + LAN) and the condition under which to switch to `repokey-blake2`.

5. ~~**`mitigations=off` on both machines** plus
   `security.sudo.wheelNeedsPassword = false`.~~ ~~A deliberate performance
   tradeoff; state it once in AGENTS.md rather than discovering it in two
   kernel-param lists.~~ Done 2026-10-08: both kernel-param lists and
   `common/base.nix` now cross-reference the same decision (trusted LAN,
   single operator, speed matters) instead of each appearing as a bare flag.

---

## 5. Smaller nice-to-haves

- ~~`just build-frontend` only builds frigate-monitor's SPA. Generalise to
  `just build-frontend <crate>`.~~ Done 2026-10-08b: `just build-frontend
  [crate]` builds any of the three Dioxus SPAs (no argument = all three). The
  parameter goes in through just's `quote()` — just 1.51 has no `positional-args`
  attribute and does not export parameters into shebang recipes, so interpolation
  is the only safe way to pass it.
- ~~The devshell runs `cargo install -f wasm-bindgen-cli` on **every** `nix
  develop` entry (network + ~1 min).~~ Done 2026-10-08 (see the devshell note
  above).
- ~~`switch-netboot` mutates `flake.lock` (`nix flake update pi-room-sys`) as a
  side effect of a deploy recipe. Split it into its own recipe.~~ Done
  2026-10-08: `just update-netboot-input` is the pin move; `switch-netboot`
  no longer touches `flake.lock`.
- ~~`just reboot` is a sysrq hard reboot listed second in the recipe list.
  Rename to `hard-reboot` so it is less likely to be hit by tab completion.~~
  Done 2026-10-08.
- ~~`machines/services1/lib/config-to-gitlab.nix` is a hand-rolled Nix→GitLab
  config serializer; if it is still used, it needs tests, otherwise delete it.~~
  Investigated 2026-10-08: it is imported by `services/gitlab.nix`, but that
  module is **commented out** of `services/default.nix`, so nothing in the
  evaluated system uses it — it is not dead code exactly (the module is meant to
  be re-enabled) but it is not deployed either.  Kept, and now tested:
  `lib/check-config-to-gitlab.nix` asserts over the serializer (top-level
  string/bool/number, `parent['child']` nesting, list rendering, the Ruby `{...}`
  form for attrs, key sort order) and over the real attrset, which is mirrored in
  `lib/gitlab-config-example.nix` so the assertions cover it while the module is
  off.  It also pins the serializer's one real limitation — string values are
  emitted inside `'...'` with **no escaping**, so a value containing a `'`
  produces broken Ruby — and asserts the deployed config contains no such value.
  Run with `nix eval --impure --raw -f machines/services1/lib/check-config-to-gitlab.nix`
  or via the new `just check`.
  Gotcha found on the way: `builtins.match` is POSIX ERE, where `\[` is an
  *invalid escape* and `(?s)` is not a thing (`.` already spans newlines) — the
  assertions use literal substring tests instead.

---

## 6. Prioritised action order

1. ~~Chain `sudo nixos-confirm` into `just switch` and `just boot`.~~ Done
   2026-10-08.
2. ~~Add `startLimitIntervalSec = 0` to the six units missing it, via a shared
   `mkNeverGiveUpService` helper.~~ Done 2026-10-08 as `INFRA.mkNeverGiveUp` /
   `INFRA.mkNeverGiveUpOverride` in `common/systemd.nix` (see §3.1).
3. ~~Extract `common/base.nix`, `common/containers.nix`, `mkWaitForNas`~~ — done
   2026-10-08 (the NAS machinery is `common/nas.nix` + `infra.nas.*`). Remaining
   from this list: `mkIntVhost` (§2.1).
4. ~~Create a shared in-repo Rust crate for `oidc`, `json`, and the asset
   embedding `build.rs` — ~700 lines deduped across five crates.~~ Done
   2026-10-08: `common-rs/{json,oidc,build-spa}` (see §2.2).
5. ~~Add `just check`~~ — done 2026-10-08, completed 2026-10-08b: it runs
   `nix flake check`, the nix-level assertion files and `just fmt-check`; the
   whole tree is alejandra-clean and the devshell ships alejandra (§1.3). The
   dry-build-both-machines step is `just check-build`, deliberately separate
   because it compiles.
6. ~~Move the restart token out of `ExecStart` into an `EnvironmentFile`.~~
   **Contradicts §4.1, which decided to keep it** — the code matches that
   decision (`common/status-dashboard.nix` still passes `--restart-token`).
   Strike this item; the fix is recorded in §4.1 in case the decision changes.
7. ~~Shrink `AGENTS.md` and move the narrative notes into `docs/`.~~ Done
   2026-10-08b (§1.5).

---

## 7. Second pass (2026-10-08b) — what landed, what is still open

Order taken: formatting first (it touches every file and would otherwise
conflict with everything after it), then the dead-config sweep, then the tooling
(`just fmt`/`fmt-check`/`check-build`/`build-frontend`/dist check), then the
three correctness items that needed new machinery (printer enums, gatus, tests),
then the doc moves. Each item is its own commit, and each was verified by
building rather than by reading: for the Nix changes, the services1 toplevel was
built at the previous commit and in the working tree and the rendered `/etc`
diffed (the printer-state and gatus changes produce a **byte-identical** system,
same store path).

Landed: §1.2, §1.3, §1.4, §1.5, §3.3, §3.6, §3.7 (as an assertion, not
generation — see the item), §3.8, §5's `build-frontend`, and §6 items 5–7.

Still open, deliberately:

- **Consolidating the three monitoring systems** (§3.7). The endpoint list is
  now guarded against the one drift that bites, but Gatus / Prometheus / the
  status dashboard remain three models of the same facts. That is a product
  decision about which one is authoritative, not a refactor.
- **Secrets baked into the store at build time.** §4.1 decided to keep the
  restart token in `ExecStart`, but the wider pattern is untouched and is worth
  quantifying before anyone decides: 13 `builtins.readFile (config.sopsSecretText
  ...)` calls across 10 modules (postgres, mattermost, outline, monitoring,
  gatus, unifi, backup, status, http/nginx-sso, containers) put secret *values*
  into world-readable store paths. Verified example: the gatus config is
  `/nix/store/1i89wjnpr47szvq0q2jx2jmvq1922nnv-gatus.yaml`, `-r--r--r--`, and
  contains the Slack webhook. Fixing it is per-consumer work (`sops.secrets` +
  `EnvironmentFile=`/`LoadCredential=`, dynamic users that cannot read the
  store, or a runtime-rendered config), each needing a deploy to prove the
  service still starts — so it is not a change to make blind.
- **`common-rs/build-spa`'s `mime_of` vs network-status' content-type table**
  (§2.2) — two lists, deliberately unshared until there is a JSON/API decision.
- **`nix flake check` does not force everything.** It evaluates options but does
  not force e.g. `systemd.services.*.serviceConfig.ExecStart` or generated etc
  texts, so a bad path inside those only surfaces in a real build. `just
  check-build` is the mitigation; a stricter check would force the whole config
  and cost evaluation time.
