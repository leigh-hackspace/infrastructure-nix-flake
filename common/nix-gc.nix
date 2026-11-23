# MODULE — NixOS module; in a machine's module list or a directory's imports, never read as data.
# Garbage collection and store optimisation, on every box that asks for it.
#
# **What this file used to be part of.** Until 2026-11-23 this was the live half of
# `common/build-farm.nix`, which also carried the fleet's remote-building config (aibox
# as the network's builder) and the ssh pins that hop needed. The builder half is
# **withdrawn** — every box builds for itself again, because the one thing that made a
# central builder attractive (evaluation building) was closed by `common/crane.nix`'s
# content-keyed `Cargo.lock`, and the RAM that made aibox a good builder is strata's
# (machines/aibox/STRATA.md). `./ssh-hop.nix`'s footer is what it would take to want
# delegation back. What is left here is the half that was never about builders.
#
# Why it is still a module of its own: the crane dependency derivations
# (`common/crane.nix`) accumulate in every store and nothing else in this repo pruned
# them; services1's root is the tightest on the network and it is the one a
# `just check-build` fills (docs/nix-dev-workflow.md, "disk headroom").
#
# GC deletes old *system* generations, which is safe for auto-rollback: `system-good` is
# a separate profile (`/nix/var/nix/profiles/system-good`) and therefore its own GC root,
# so the generation the rollback timer would switch to cannot be collected by
# `--delete-older-than` on the `system` profile.
#
# Nothing here touches `nixbld`: `common/nixbld.nix` owns that invariant and refuses any
# account added to the build-user pool, and this file has no opinion about it.
{
  config,
  lib,
  ...
}: let
  cfg = config.infra.nixGc;
in {
  options = {
    infra.nixGc.enable = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Automatic nix garbage collection and store optimisation (unreachable paths only).";
    };
  };

  config = {
    nix.gc.automatic = cfg.enable;
    # `nix-collect-garbage` has no `--max-age` (only `--delete-older-than` / `--delete-old`),
    # so the earlier `--max-age 30d` made nix-gc.service fail on every run.
    #
    # Deleting old *system* generations is safe for auto-rollback: `system-good` is a
    # separate profile, so it is its own GC root and this cannot collect the generation the
    # rollback timer would switch to.
    nix.gc.options = "--delete-older-than 30d";
    nix.optimise.automatic = cfg.enable;
  };
}
