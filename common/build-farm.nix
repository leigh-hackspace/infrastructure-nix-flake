# MODULE — NixOS module; in a machine's module list or a directory's imports, never read as data.
# Garbage collection and remote building.
#
# Two separate things, both about balance rather than features:
#
#   1. GC on every machine. The crane dependency derivations (common/crane.nix)
#      accumulate in the store and nothing else in this repo pruned them; services1's
#      root is the tightest of the three and is what a three-machine build fills.
#
#   2. aibox as the build machine.  It has 59 GiB of RAM, 6c/12t, and — since
#      strata was parked on 2026-10-17 (machines/aibox/default.nix) — no engine
#      holding ~33 GiB of that RAM.  Its own apps are leaf services that services1
#      proxies, so a build storm costs frigate/immich/whisper rather than the box
#      every public request goes through.  services1 is the box that runs the
#      services and is running out of room; services2's 30 GiB is strata's, so a
#      build there competed with the engine (the reason it was the builder until
#      2026-10-17).  `just build-remote` offloads a build to aibox.
#
# GC deletes old *system* generations, which is safe for auto-rollback: `system-good`
# is a separate profile (`/nix/var/nix/profiles/system-good`) and therefore its own GC
# root, so the generation the rollback timer would switch to cannot be collected by
# `--delete-older-than` on the `system` profile.
#
# Remote building is ON by default and is not merely opt-in: in nix 2.34 the scheduler
# prefers an available external builder for every build ("Default preference is a remote
# build", derivation-building-goal.cc) and only falls back to local when no builder can do
# it. `--max-jobs 0` therefore does not enable delegation — it removes the local fallback.
# So a plain `nix build` / `just switch` on services1 compiles on aibox and copies the
# result back; if aibox is down nix builds locally instead. To go back to local-only,
# set `nix.distributedBuilds = false` (which also makes `--max-jobs 0` fail with "local
# builds are disabled", since nothing is left to delegate to).
#
#   3. the pinned host keys for the hop (`programs.ssh.knownHosts` from
#      ./ssh-known-hosts.nix).  Not optional and not per-box: the hop's ssh is
#      the *daemon's*, run as root out of /root, so the operator account's
#      known_hosts — the file that makes a manual `ssh -i ~/.ssh/agent-hop-key
#      10.3.1.32` work — is a different ssh entirely, and a delegating box whose
#      root has never visited the builder fails the hop before the build starts.
#      Mechanism and receipts: docs/nix-dev-workflow.md, "Remote building: the
#      hop is not the operator's ssh".
#
#   3b. the **identity** for the hop, on the network's own addresses
#       (`programs.ssh.hosts`, generated from ./hosts.nix) — the half that decides
#       what an ssh *offers*, as opposed to what it *checks*.  It is in this module
#       rather than in ./users.nix because this nixpkgs has no per-user home-file
#       option (the header of ./users.nix has the refusal), and because the system
#       file is read by every ssh on the box, root's included.  The symptom it
#       retires is a `git pull` that prints `(leigh-admin@10.3.1.20) Password:`
#       rather than an error.  Measurements and the full argument:
#       docs/nix-dev-workflow.md, "The identity half of that hop is not the host
#       half, and it was missing everywhere".
#
# Per machine (see the two default.nix files):
#
#   services1:  infra.buildFarm.builderHost = HOSTS.aibox; maxJobs = 6;
#   aibox:      infra.buildFarm.serveBuilds = true;
#   gw:         infra.buildFarm.builderHost = HOSTS.aibox, and gc = false — the router
#               delegates its builds but does not collect garbage (machines/gw/
#               default.nix explains the `nix.settings.max-jobs = 0` that goes with it);
#   services2:  neither — served builds until 2026-10-17, now a client with no builder.
{
  config,
  lib,
  ...
}: let
  cfg = config.infra.buildFarm;
  HOSTS = import ./hosts.nix;
in {
  options = {
    infra.buildFarm = {
      gc = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Automatic nix garbage collection (unreachable paths only).";
      };
      serveBuilds = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = "Accept remote builds from the nix daemon on other machines.";
      };
      builderHost = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Host to delegate builds to. Empty unless set.";
      };
      speedFactor = lib.mkOption {
        type = lib.types.int;
        default = 20;
        description = "Builder speed factor. The external builder is preferred over local builds.";
      };
      maxJobs = lib.mkOption {
        type = lib.types.int;
        default = 6;
        description = "Jobs the builder may run at once (aibox's 6 cores; the default was services2's 8).";
      };
    };
  };

  config = {
    nix.gc.automatic = cfg.gc;
    # `nix-collect-garbage` has no `--max-age` (only `--delete-older-than` / `--delete-old`),
    # so the earlier `--max-age 30d` made nix-gc.service fail on every run.
    #
    # Deleting old *system* generations is safe for auto-rollback: `system-good` is a
    # separate profile (`/nix/var/nix/profiles/system-good`), so it is its own GC root
    # and this cannot collect the generation the rollback timer would switch to.
    nix.gc.options = "--delete-older-than 30d";
    nix.optimise.automatic = cfg.gc;

    # NOTE: deliberately *no* `nixbld` membership here, on either end of the hop.
    #
    # An earlier version of this file put leigh-admin in nixbld "so the daemon is
    # used at all" and "so the hop is accepted". Both reasons were wrong: this
    # network runs `allowed-users = *`, which is what accepts a client, and
    # `/nix/var/nix` is root-only, so a non-root nix reaches the store through the
    # daemon socket whether or not its user is in nixbld.
    #
    # What the membership really did was make leigh-admin a *candidate build UID*
    # (nix runs each build as a member of `build-users-group` and SIGKILLs that UID
    # before and after the build), and so every build on services1 and services2
    # killed the admin's entire session. Read common/nixbld.nix before adding any
    # account to that group; the assertion there now rejects this config.

    # Being *accepted* is not the same as being *trusted*. With
    # `require-sigs = true` (the default) the builder's daemon refuses to build
    # input-addressed derivations for a client it does not trust, so every delegated
    # build died with "you are not privileged to build input-addressed derivations"
    # (hit 2026-10-11: `nixos-rebuild` on services1 evaluating the sops-secret
    # derivations, which are built during evaluation and therefore go to the builder).
    # The `sshUser` above has to be a trusted user on the builder too. Appended to the
    # default `["root"]`, so this is `root` plus `leigh-admin` on both ends.
    nix.settings.trusted-users = lib.optionals (cfg.serveBuilds || cfg.builderHost != null) [
      "leigh-admin"
    ];

    # `nix.buildMachines` alone is not enough: nix-remote-build.nix sets
    # `nix.settings.builders = null` unless `nix.distributedBuilds` is true, so the
    # nix.conf line the daemon reads stays empty and `--max-jobs 0` fails with
    # "local builds are disabled" even though /etc/nix/machines is correct. Both are
    # needed — the option list and the switch that turns it into the setting. See the
    # header for what turning it on implies (remote is preferred, not opt-in).
    #
    # This was off from 2026-10-11 to 2026-10-12, because the builder's build-user cleanup
    # was SIGKILLing the very `nix serve --stdio` the delegating machine was talking
    # to (every delegated build died with an empty log, and services2's journal
    # filled up with `user@1234.service: status=9/KILL`). Cause and fix:
    # common/nixbld.nix. With the cleanup UID a throwaway nixbld account again the
    # sweep has nothing of ours to hit, on either end of the hop — verified with a
    # delegated build and a `--max-jobs 0` build, both clean.
    nix.distributedBuilds = cfg.builderHost != null;

    # Let the *builder* fetch its own inputs from the cache, instead of the delegating box
    # copying every one of them over the hop.  This is not a preference, it is the
    # difference between a delegated build being fast and slow, and nobody had ever chosen
    # it: `builders-use-substitutes` defaults to `false`, so from the day remote building
    # went on, every input of every delegated build was substituted *here* and then uploaded
    # to aibox (`nix.conf(5)`: with it on, remote hosts "fetch as many dependencies as
    # possible from their own substituters (e.g, from `cache.nixos.org`) instead of waiting
    # for the local machine to upload them all.  This can drastically reduce build times").
    #
    # A nixpkgs bump is the case that shows it: hundreds of derivations that are already on
    # cache.nixos.org, every one of them downloaded by services1 and shipped to aibox before
    # aibox can start on the first one, with aibox's job slots idle while they arrive.  On
    # the hop, `true` lets aibox pull them itself, in parallel with its own builds; inputs it
    # cannot substitute (a local `runCommand`, a crane dep tree, a sops secret) are uploaded
    # exactly as before, because the ssh-ng builder asks for whatever it is missing.
    #
    # Per-client, not per-builder: it is the delegating box's setting, and it changes no
    # output — only who fetches an input.  It is on whenever this box has a builder at all
    # (`nix.distributedBuilds`' condition), and off on aibox, which delegates nothing.
    # Verify it by delegating a build of something with a large input closure and watching
    # aibox's journal for `substituting`/`copying path … from https://cache.nixos.org` rather
    # than a stream of paths pushed from here.
    nix.settings.builders-use-substitutes = cfg.builderHost != null;

    # Every box that takes part in the hop pins the network's own host keys, in
    # the store, rather than trusting whichever root `known_hosts` happens to
    # exist: when a delegated build fails with `Host key verification failed`
    # there is nothing else in the log, and the box that *works* is the one
    # someone ssh'ed from by hand at some point. See ./ssh-known-hosts.nix for
    # why pinning (rather than accept-new) is the point, and `just known-hosts`
    # for regenerating it after a box regenerates its keys.
    programs.ssh.knownHosts = import ./ssh-known-hosts.nix;

    # The other half of the same hop, and the half that was missing on every box.
    #
    # `knownHosts` says what an ssh *checks*; nothing said what an ssh *offers*.
    # The fleet's key is at `~/.ssh/agent-hop-key`, which is none of the file names ssh
    # tries on its own (`id_rsa`, `id_ed25519`, …), and **no ssh-agent runs on these
    # boxes** — so an ssh that does not carry `-i ~/.ssh/agent-hop-key` offers *nothing*,
    # and sshd's last offer is a **prompt**.  That is exactly what a plain `git pull`
    # printed on services3 on 2026-11-15:
    #
    # (leigh-admin@10.3.1.20) Password:
    #
    # on a box whose sshd is `prohibit-password`, inside a job that had no terminal to
    # answer it.  The job wedges, and the wedge reads as "the network" rather than as
    # "no identity was offered" — the symptom appears far from the cause, the same shape
    # as the `Host key verification failed` receipt above, one file over.  `justfile:148`
    # works around it per call with an inline `GIT_SSH_COMMAND`; aibox and services3 keep
    # hand-written `/root/.ssh/config` stanzas for root's ssh.  Both are the mutable file
    # no deploy can reproduce — this repo's own audit's shape.
    #
    # Why the *system* client config rather than a per-user `~/.ssh/config`: this pinned
    # nixpkgs' `users.users.<name>` submodule has **no file-writing option at all** —
    # `just check` refuses `users.users.leigh-admin.extraFiles`, and this release moved
    # authorized keys to `AuthorizedKeysFile`/`/etc/ssh/authorized_keys.d`
    # (`nixos/modules/services/networking/ssh/sshd.nix`), leaving home files to Home
    # Manager, which this flake does not use.  `programs.ssh.hosts` is the
    # attribute-name-as-`Host`-pattern mechanism the module already has, and its own
    # documented example is this stanza (`IdentityFile` + `IdentitiesOnly`).  `~` in
    # `IdentityFile` is expanded by ssh to the *calling* user's home, so it names the
    # right file on every box without naming a box — and being the system file, it also
    # reaches root's ssh (`HOME=/root`), which the hand-written stanzas used to be the
    # only thing standing in for.
    #
    # What each line is for:
    #
    # * `IdentitiesOnly` is the documented fix for `Too many authentication failures`
    #   (`justfile:148`): a box that offers every identity it has is refused *before* the
    #   right one is tried — services1's `~/.ssh` also holds `id_ed25519`, which no LAN
    #   machine accepts, and measured from services3 a bare `git ls-remote` against the
    #   peer reached exactly that line.  Scoped to these hosts it is why the stanza is
    #   safe: GitHub is not one of them, and the documented leak trap
    #   (`docs/network-upgrade-runbook.md` — an **exported** `GIT_SSH_COMMAND` putting
    #   the hop key on the GitHub remote, `Permission denied (publickey)`) cannot happen
    #   from a generated file, because the host is what the block matches on.
    # * `ConnectTimeout` so a hop to a box that is not answering fails *inside* a tool
    #   call rather than hanging it (rule 11: bound every wait).
    # * deliberately **no** `PasswordAuthentication no`, and no match on a *user*: services3's
    #   owner account keeps a password login as the *spare path* onto a box whose only
    #   uplink is a Wi-Fi dongle (`machines/services3/users.nix`, which warns to check
    #   `sudo sshd -T | grep -i passwordauthentication` after any ssh change), and a
    #   host-keyed `no` would retire that fallback for whoever types it.  Pubkey is tried
    #   first, so this stanza answers a LAN hop while the password path stays available
    #   underneath it.
    # * the core switches are **not** in the list (Telnet only, `docs/procurve-switches.md`);
    #   the NAS **is** (`docs/nas-and-borg.md`: `leigh-admin@10.3.1.6` is keyed with the
    #   machine-hop key), and it is one of `./ssh-known-hosts.nix`'s pins, so the two halves
    #   of the hop stay keyed to one address list — this one, generated from `./hosts.nix`.
    #
    # `order = 100` is safe because nothing else in the pinned nixpkgs declares
    # `programs.ssh.hosts` (verified: no other module names the option) and the module
    # throws on two blocks sharing an order; blocks are written in ascending order, so
    # keep this low for when a catch-all `"*"` block is ever added.  Read the rendered
    # file after any switch: `/etc/ssh/ssh_config` must carry the `Host` line and the
    # stanza below it (rule 21 — read the rendered file, never evaluate).
    programs.ssh.hosts = {
      "${HOSTS.services1} ${HOSTS.services2} ${HOSTS.services3} ${HOSTS.aibox} ${HOSTS.router} ${HOSTS.nas}" = {
        order = 100;
        IdentityFile = "~/.ssh/agent-hop-key";
        IdentitiesOnly = true;
        ConnectTimeout = 5;
      };
    };

    nix.buildMachines = lib.optionals (cfg.builderHost != null) [
      {
        hostName = cfg.builderHost;
        system = "x86_64-linux";
        sshUser = "leigh-admin";
        # sshKey has to be spelled out: the daemon runs as root, so it does not look
        # in leigh-admin's home by itself.
        sshKey = "/home/leigh-admin/.ssh/agent-hop-key";
        # aibox is faster per job than services1 (6c/12t vs 4c/8t) and its RAM is
        # free since strata was parked, so it is preferred when both are available.
        speedFactor = cfg.speedFactor;
        maxJobs = cfg.maxJobs;
      }
    ];
  };
}
