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
# GC deliberately does NOT delete generations. `system.autoRollback` (common/base.nix)
# rolls back to the last *confirmed* generation, so the `system-good` generation has to
# survive whatever gc does — `--delete-generations old` would eat it. Only unreachable
# store paths are collected.
#
# Remote building is opt-in per invocation (`--max-jobs 0`), not global: with
# `nix.maxJobs > 0` the client builds locally anyway, and forcing it to 0 would make
# every deploy depend on services2 being reachable. A build that falls back to local is
# worth more than a build that fails because the builder is down.
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
    };
  };

  config = {
    nix.gc.automatic = cfg.gc;
    # No --delete-generations: see the note at the top.
    nix.gc.options = "--max-age 30d";
    nix.optimise.automatic = cfg.gc;

    # The nix daemon serves builds for `allowed-users`, which defaults to
    # `root @nixbld`, and `builders` is a restricted setting — a non-trusted user's
    # `nix build` ignores it. So leigh-admin has to be in nixbld on *both* ends: the
    # machine that delegates (so the daemon is used at all) and the machine that
    # accepts (so the hop is accepted). It is not in nixbld by default on any machine
    # (common/users.nix), and there is no `nix.serveUsers` option in current nixpkgs —
    # that was the old nix-serve knob.
    users.users.leigh-admin.extraGroups =
      lib.optionals (cfg.serveBuilds || cfg.builderHost != null) ["nixbld"];

    nix.buildMachines = lib.optionals (cfg.builderHost != null) [
      {
        hostName = cfg.builderHost;
        system = "x86_64-linux";
        sshUser = "leigh-admin";
        sshKey = "/home/leigh-admin/.ssh/agent-hop-key";
        # services2 is faster per job than services1 (8c/16t) and has the disk, so
        # it is preferred when both are available.
        speedFactor = 20;
        maxJobs = 8;
      }
    ];
  };
}
