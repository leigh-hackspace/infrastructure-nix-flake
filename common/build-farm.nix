# Garbage collection and remote building.
#
# Two separate things, both about balance rather than features:
#
#   1. GC on every machine. The crane dependency derivations (common/crane.nix)
#      accumulate in the store and nothing else in this repo pruned them; services1
#      is at 84% of its root partition (116 GB of store on 216 GB).
#
#   2. services2 as the build machine. It has 211 GB free and no services worth
#      protecting from a build, while services1 is the box that runs the services and
#      is running out of room. `just build-remote` offloads a build there.
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
# So a plain `nix build` / `just switch` on services1 compiles on services2 and copies the
# result back; if services2 is down nix builds locally instead. To go back to local-only,
# set `nix.distributedBuilds = false` (which also makes `--max-jobs 0` fail with "local
# builds are disabled", since nothing is left to delegate to).
#
# Per machine (see the two default.nix files):
#
#   services1:  infra.buildFarm.builderHost = HOSTS.services2;
#   services2:  infra.buildFarm.serveBuilds = true;
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
        description = "Builder speed factor. services2 is preferred over local builds.";
      };
      maxJobs = lib.mkOption {
        type = lib.types.int;
        default = 8;
        description = "Jobs the builder may run at once (its core count).";
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

    nix.buildMachines = lib.optionals (cfg.builderHost != null) [
      {
        hostName = cfg.builderHost;
        system = "x86_64-linux";
        sshUser = "leigh-admin";
        # sshKey has to be spelled out: the daemon runs as root, so it does not look
        # in leigh-admin's home by itself.
        sshKey = "/home/leigh-admin/.ssh/agent-hop-key";
        # services2 is faster per job than services1 (8c/16t) and has the disk, so it
        # is preferred when both are available.
        speedFactor = cfg.speedFactor;
        maxJobs = cfg.maxJobs;
      }
    ];
  };
}
