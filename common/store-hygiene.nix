# MODULE — NixOS module, imported from `common/base.nix`, so every machine gets it.
# A weekly, idle-priority check that no store path has been written into since nix
# registered it — the fault class that took a failed cross-box build to find.
#
# ## What actually happened, on aibox, 2026-11-16
#
# `nix build` of aibox's toplevel from services1 died with:
#
# ```
# error: hash mismatch importing path '/nix/store/frskrh…-strata-unstable';
#          specified: sha256-x1+VVH0db9tye7gv9slcHM0IrabMZHa8oQj6BM5Chzc=
#          got:       sha256-sdnwqRnuTb8mJh+7xAtzx6nxPmHj6QVaepbhBYGORwc=
# ```
#
# and the same command on aibox itself (`nix store verify`) said the quieter, worse
# thing: `path '…-strata-unstable' was modified!`.  Two boxes, one path, two different
# lies — aibox's own database still called the path valid, so nothing on the builder
# noticed for a day, and the fault only surfaced when another machine tried to *import*
# it.  A `nix path-info` answers happily and `--sigs` even says `ultimate`.
#
# It was not disk rot, and it was not tampering.  Inside that path were 56 files with
# 2026-11-15 timestamps: `share/strata/strata_control/__pycache__/*.cpython-314.pyc`.
# `strata.service` runs **as root** (no `User=`, no `DynamicUser=`), and a NixOS store
# path is read-only only in the DAC sense — files 0444, directories 0555 — which root
# walks straight past, because ext4 lets it.  So the first time the engine imported its
# Python control layer, CPython wrote its bytecode caches **into its own store path**.
#
# The reason that breaks the hash rather than merely untidying the tree: a NAR records
# the file list, the contents, and the executable bit — **not mtimes**.  So an added
# file changes the hash and the recorded nar no longer matches the directory, while a
# touched mtime does not.  That asymmetry is the whole diagnosis, and it is why the
# scan below looks for *entries* newer than the epoch and then confirms with
# `nix store verify` instead of treating a stray timestamp as the finding.
#
# Scope, measured the same day: on aibox **5 of 5 strata store paths** were polluted
# (14–56 files each, every one inside a `__pycache__` directory, zero elsewhere), so
# this is not an incident but the package's standing behaviour — every strata build
# gets dirtied the first time the engine starts.  Nothing else on the box mismatched.
#
# ## The fix, and what this module is for
#
# Prevention belongs in the unit, not here: `machines/aibox/strata.nix` sets
# `PYTHONDONTWRITEBYTECODE=1`, and the general rule is either that or a non-root
# `User=`, at which point the 0555 directories do their job.  See
# `docs/nix-dev-workflow.md`, "A root process can write into a store path, and Python
# does".
#
# This module is the net under it.  The class of fault is *silent on the machine that
# has it*, so the alternative to checking is finding out during a deploy, from the
# other end of a build that has already cost twenty minutes.  It is metadata-only
# (`find` on directory entries, no content read), it runs at `IOSchedulingClass=idle`
# and `Nice=19` because it walks the store, and it **fails the unit** rather than
# logging: the failure then shows up in `systemctl --failed`, which is the same reason
# `nixbld-guard` and `lease-reader-watch` write to the journal instead of being polite.
#
# Repairing a path it names, without restarting anything: delete exactly the entries
# newer than the epoch that live under a `__pycache__` directory and re-run
# `nix store verify --no-trust <path>`.  Bytecode caches are regenerable by
# construction, so this is the whole repair for this class; a mismatch that survives it
# is a *different* fault, and the answer to that one is
# `nix-store --realise <drv> --repair` — which replaces files under a running process,
# so it wants the same window as the service restart.
{
  lib,
  pkgs,
  config,
  ...
}: let
  cfg = config.infra.storeHygiene;
in {
  options.infra.storeHygiene.enable = lib.mkOption {
    type = lib.types.bool;
    default = true;
    description = ''
      Weekly check that no store path has gained entries since nix registered it.
      Cheap enough for every machine (metadata only, idle IO); turn it off on a box
      only if the walk itself is ever the problem, not because the finding is.
    '';
  };

  config = lib.mkIf cfg.enable {
    systemd.services.store-hygiene = {
      description = "Store hygiene: store paths written into after registration";
      serviceConfig = {
        Type = "oneshot";
        # Walking /nix/store's inodes is the whole job, so be the last thing the
        # disk serves: this box builds, serves inference and (on gw) answers DNS.
        Nice = 19;
        IOSchedulingClass = "idle";
        ProtectSystem = "strict";
        ProtectHome = true;
        NoNewPrivileges = true;
        PrivateTmp = true;
      };
      script = ''
        # One pass, no line continuation: a bare backslash before a newline is not a
        # valid escape inside a Nix string, which is a fun way to learn it.
        polluted=$(${pkgs.findutils}/bin/find /nix/store -xdev -type d -name __pycache__ -newermt 2000-01-01 2>/dev/null | ${pkgs.gnused}/bin/sed -E "s#^(/nix/store/[a-z0-9]{32}-[^/]+)/.*#\\1#" | ${pkgs.coreutils}/bin/sort -u)
        if [ -z "$polluted" ]; then
          echo "store-hygiene: clean — no store path has gained entries since registration"
          exit 0
        fi
        echo "store-hygiene: these store paths were WRITTEN INTO after nix registered them,"
        echo "so their contents no longer match the hash in the nix database:"
        printf '  %s\\n' $polluted
        echo ""
        echo "A NAR records the file list and contents but not mtimes, so an added file"
        echo "means this box still calls the path valid while every other box refuses to"
        echo "import it ('hash mismatch importing path').  Confirm each with"
        echo "'nix store verify --no-trust <path>'; the usual cause is a root process"
        echo "importing Python from a store path (fix: PYTHONDONTWRITEBYTECODE=1 or a"
        echo "non-root User=).  Repair by deleting the newer-than-epoch entries under"
        echo "__pycache__ — bytecode caches are regenerable — and re-verify."
        exit 1
      '';
    };

    systemd.timers.store-hygiene = {
      description = "Weekly store-hygiene walk";
      wantedBy = ["timers.target"];
      timerConfig = {
        # Weekly, and `Persistent` so a box that was off for the week catches up
        # rather than running a month stale — the finding is old by nature.
        OnCalendar = "Sun 04:30";
        Persistent = true;
        AccuracySec = "1h";
        RandomizedDelaySec = "3h";
      };
    };
  };
}
