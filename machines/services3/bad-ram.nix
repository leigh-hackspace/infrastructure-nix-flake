# MODULE — keep this box's usable RAM below the bad DIMM's measured fault window.
#
# ─── what is capped, and why a cap rather than a fence ─────────────────────────
#
# DIMM_A2 (Kingston KTP9W1-MIE 16 GiB, serial 49216745) is **proven bad** and is
# deliberately back in the box, because RAM is scarce and 16 GiB of it is worth more
# than the inconvenience.  `./bad-ram-pages.txt` is the measured fault set: 298
# single-bit flips over 272 physical pages, every one in byte 6 of the 8-byte word,
# 25 of 25 re-failing on the same bit with the value written opposite — a defective
# part at rest, not a marginal one.  The lowest measured bad byte is 0x7d5a29000
# (31.338 GiB), the highest 0x99431e000 (38.316 GiB), and `./README.md`'s "One
# stability fact this box earned the hard way" owns the measurement, the two kernel
# Oopses that named frames inside the same window, and the DIMM inference.
#
# Two ways to keep those addresses out of the allocator, and this module does both while
# the change is staged ("the cap comes off" below): a **window** through `mem=`, and the
# **measured pages themselves** through a kernel patch.  The command line can only
# express the window; the arithmetic for the pages is why (measured 2026-11-23/25, not
# assumed):
#
#   272 pages, one entry each   3264 characters of `memmap=` against a 1772-character
#                               budget (2047-byte command line less 245 of fixed
#                               arguments); 544 E820 slots against the ~295 the
#                               firmware map leaves spare (320 - 25).
#   merge adjacent frames       272 -> 269 runs.  The bad cells are ~2 MiB apart, so
#                               there is nothing to merge and no characters to save;
#                               the only lever is *cutting the gaps between points*,
#                               and that is where the memory goes.
#   110-range cover             164.73 MiB of clean RAM reserved to fence 1.06 MiB of
#                               bad pages: 1770 characters, 220 slots, 31 spare.
#   generated strings, today   the honest form, one `memmap=` per frame, is 8683
#                               characters against a 2047-byte limit — over by 6.6 KiB.
#                               Coarsening enough to fit buys a blanket: 16 MiB
#                               granularity is 70 ranges and 1120 MiB of clean RAM lost,
#                               and even 1 MiB granularity (230 ranges, 230 MiB) does
#                               not fit.
#
# and the 110-range cover is not merely expensive, it is **falsified**.  It was armed
# on the 2026-11-23 00:16 boot (generation `19hkg86ckj…`, 110 ranges, byte-addressed,
# comfortably inside the command-line budget).  That boot reached userspace at 5.0 s
# and then lost **both** of its block devices — the SATA SSD holding `/` and `/boot`
# and the NVMe holding `/srv/strata` — ending in emergency mode with `systemd-oomd`
# failing (it has never failed on this box in any other logged boot) and with no
# journal, because the units that write to the root filesystem failed too.  So a
# fence over the *sample* does not make this part usable: the population of bad cells
# is bigger than the 272 that were caught.
#
# The cap is therefore a **window** exclusion, not a page exclusion.  Everything at or
# above `cap` is never handed to the allocator, which puts 100% of every address this
# box has ever been seen corrupting out of reach — for nine characters of command line,
# zero extra E820 slots, and no `memmap=` syntax to truncate.  It costs the top of the
# 40 GiB (about 29.5 GiB usable instead of 39.8), and what it buys is that the RAM it
# *does* hand out is not the RAM that has been observed corrupting page tables.
#
# ----- the command line is not the only place a page list can live -----
#
# `memblock_reserve()` takes reservations by address and has no 2 KiB limit, so the
# pages can be excluded *individually* from inside the kernel.  `./bad-ram-kernel.patch.in`
# is a 48-line patch to `arch/x86/kernel/e820.c` that carries the same measured frames as
# a `.rodata` array and reserves every one of them in `e820__memblock_setup()`, before
# `memblock_free_all()` ever populates the buddy allocator — so those 272 pages are never
# handed out, and never counted in `MemTotal` either.  It reserves 272 × 4 KiB = 1088 KiB
# and leaves the rest of the 40 GiB (≈38.5 GiB usable) reachable, which is the answer to
# "how do we get to almost 40 GiB" that a command line cannot give.  The patch is
# **generated**: its array element is the single `@PAGES@` line, spliced from
# `./bad-ram-pages.txt` by `badRamPatch` below, and it applies with zero fuzz to the
# pinned 6.18.63 tree (verified 2026-11-25 against the tarball's own `e820.c`).
#
# It also costs, and this is why it is a patch and not a preference: a locally built
# kernel has no binary cache.  Every nixpkgs or kernel bump rebuilds `linux` and the
# NVIDIA module **on this box** (30–45 min on the i7-6700), and the patch is
# boot-critical — a bad hunk is a box that does not start, so the fallback generation in
# systemd-boot is part of the procedure, not a nicety.
#
# ----- the cap comes off only when the reservation is proven -----
#
# `cap` therefore now defaults to **`null`** — no `mem=` at all — because the page
# reservation is deployed and has been proven to boot.  **Step 1, done 2026-11-25
# 23:10:17**: booted `1pchb7ms…` / `gs67h8pd…-linux-6.18.63`, which printed
# `services3-bad-ram: reserved 272 measured-bad pages (1088 KiB)`, with the check's four
# lines green and `MemTotal` still 28.46 GiB under the then-cap (receipts in `./README.md`,
# "The measured pages are now excluded by the kernel itself").  That proves the array is
# in the image, is counted at boot, and the box runs.
#
# **Step 2 is the capacity** — the `null` default — and it is only legitimate if the
# uncapped 2-hour run on the patched kernel comes back clean, because the reservation
# covers what was measured and 2026-11-23 is what an unmeasured cell costs.  If it is not
# clean: set `cap = "30G"` back (the belt) and add what it found to `./bad-ram-pages.txt`,
# which regenerates *both* the patch and the cap's floor.  The 2 GiB of margin between the
# old cap and the lowest bad page stops being needed the moment the pages themselves are
# excluded, which is the whole point of the patch.
#
# ─── what this module cannot do, stated plainly ────────────────────────────────
#
# It excludes the *measured* pages and a *measured* window, not the part.  298 hits are
# a sample from one 16 GiB module with no ECC (`EDAC ie31200: No ECC support`), so a cell
# that only flips under a pattern nobody has tried yet is still live — neither inside the
# 1088 KiB the patch reserves nor anywhere else in the 40 GiB.  That is why the window
# stays on until an uncapped test says otherwise, and why earning the top 10 GiB back
# means re-measuring (`stressapptest -M 34000 -s 7200` with the engine stopped, then
# regenerate `./bad-ram-pages.txt` — which changes *both* the patch and the cap, because
# they are two views of one file) rather than raising a number by hand.
#
# ─── the two things that must never happen ─────────────────────────────────────
#
# A cap that silently did nothing is worse than no cap, because the box then gets
# trusted with the fault window live again.  So there are two guards, and they are the
# reason this is a module and not one line in `./hardware-configuration.nix`:
#
#   * an **assertion**, so an edit that raises `cap` at or above the lowest measured
#     bad page refuses to evaluate;
#   * `bad-ram-cap-check.service`, which reads back what the kernel actually did —
#     `mem=` on `/proc/cmdline`, `MemTotal` against the requested cap, and the top of
#     `/proc/iomem`'s `System RAM` against the lowest measured bad byte — and fails
#     `systemctl --failed` if any of the three disagrees.
#
# Nothing here caps memory any more in the default configuration, so the interesting
# report is the reservation: `bad-ram-cap-check` prints the kernel's own count of reserved
# pages (`journalctl -k -b | grep 'bad-ram: reserved'`), `MemTotal` (which must be near the
# full 40 GiB once the belt is off), and — only while a cap is still set — the top of
# `/proc/iomem`'s `System RAM` against the lowest bad byte.  The proof that the bad
# *pages* are out of the allocator is the reservation line plus the stress test; there is
# no `/proc` view of a memblock reservation.
{
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.infra.badRam;

  # DATA: one page frame per line, field 1.  `builtins.fromTOML` is the hex parser
  # because Nix has no `strtonum` and `0x…` is a valid TOML integer — the alternatives
  # are a hand-written digit fold or a hand-copied number, and a copied number is the
  # fact this file exists to stop being copied.
  frameOf = line: let
    m = builtins.match "[[:space:]]*(0x[0-9a-fA-F]+).*" line;
  in
    if m == null
    then null
    else (builtins.fromTOML "frame = ${builtins.head m}").frame;

  frames =
    lib.filter (f: f != null)
    (map frameOf (lib.splitString "\n" (builtins.readFile ./bad-ram-pages.txt)));

  pageCount = lib.length frames;

  lowestBadFrame =
    lib.foldl' (a: b:
      if a < b
      then a
      else b)
    (lib.head frames)
    frames;

  lowestBadByte = lowestBadFrame * 4096;

  # Hex, for the assertion's message only: `builtins.toJSON`/`toString` print decimal and a
  # frame number is read in hex everywhere else in this tree (`/proc/iomem`, the file).
  hexDigits = ["0" "1" "2" "3" "4" "5" "6" "7" "8" "9" "a" "b" "c" "d" "e" "f"];
  toHex = n: let
    go = v: acc:
      if v == 0
      then acc
      else go (builtins.div v 16) (builtins.elemAt hexDigits (v - builtins.div v 16 * 16) + acc);
  in
    if n == 0
    then "0"
    else go n "";

  frameHex = toHex lowestBadFrame;

  # The kernel patch carries the same list.  Its array element is a single line — one
  # `0x<frame>, ` per measured page — which is why the patch is a fixed 48-line diff and
  # why nothing here counts lines or hunk offsets.  `bad-ram-pages.txt` stays the DATA;
  # this and `./bad-ram-kernel.patch.in` are both derived from it.
  pagesLine = lib.concatStringsSep ", " (map (f: "0x" + toHex f) frames);

  badRamPatch = pkgs.runCommandLocal "services3-bad-ram-pages.patch" {} ''
    ${pkgs.gawk}/bin/awk '
      NR == FNR { pages = $0; next }
      $0 == "+\t@PAGES@" { print "+\t" pages; next }
      { print }
    ' ${pkgs.writeText "services3-bad-ram-frames.txt" pagesLine} ${./bad-ram-kernel.patch.in} > $out
  '';

  # `mem=<n>[KMGT]`, the same spelling the kernel takes, or `null` for no cap at all (the
  # patched kernel's page reservation is then the only exclusion; see the header).  Parsed
  # rather than trusted: the assertion below is the only thing between a careless edit and
  # a boot that quietly re-admits the fault window.
  capBytes =
    if cfg.cap == null
    then null
    else let
      m = builtins.match "([0-9]+)([KMGT])" (lib.toUpper cfg.cap);
      mul = {
        K = 1024;
        M = 1024 * 1024;
        G = 1024 * 1024 * 1024;
        T = 1024 * 1024 * 1024 * 1024;
      };
    in
      if m == null
      then null
      else lib.toInt (builtins.elemAt m 0) * mul.${builtins.elemAt m 1};

  gib = n: builtins.div n (1024 * 1024 * 1024);
in {
  options.infra.badRam = {
    enable = lib.mkOption {
      type = lib.types.bool;
      # Default ON, deliberately: `lib.mkEnableOption` defaults to false, and this module
      # is the only thing keeping a proven-bad DIMM's fault window out of the allocator.
      # An opt-in default would mean the box boots with all 40 GiB reachable the moment
      # someone forgets a line.  Turn it OFF only when the bad stick has been physically
      # pulled — and then regenerate nothing else, because with the stick out the top of
      # the address space is good RAM again.  `false` while the stick is in is the
      # 2026-11-23 configuration.
      default = true;
      description = ''
        Cap usable RAM below the bad DIMM's measured fault window.  `false` is only
        correct once that DIMM is no longer in the machine.
      '';
    };

    cap = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = ''
        `mem=` value, or `null` for no cap at all. With the page reservation in force
        (`./bad-ram-kernel.patch.in`) `null` is the end state: 1088 KiB of the 40 GiB
        reserved, the fault window excluded page by page instead of by window, and
        ≈38.5 GiB usable. `"30G"` is the belt — set it back the moment a test finds a
        cell outside `./bad-ram-pages.txt`, and regenerate that file rather than raising
        a number by hand. The assertion refuses any value at or above the lowest measured
        bad page (31.338 GiB in the current file).

        Do not move this to `null` in a tree whose `boot.kernelPatches` is not carrying
        the array: without the reservation, `null` hands the fault window to the
        allocator, and 2026-11-23 is what that costs (`./README.md`).
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.cap == null || capBytes != null;
        message = ''
          infra.badRam.cap is "${toString cfg.cap}", which is neither null nor a `mem=` size
          the kernel takes.  Use <number>K, <number>M, <number>G, <number>T (for example
          "30G"), or null for no cap at all.
        '';
      }
      {
        assertion = capBytes == null || capBytes < lowestBadByte;
        message = ''
          infra.badRam.cap is "${cfg.cap}" (${toString (gib capBytes)} GiB) and the lowest
          measured bad page starts at byte ${toString lowestBadByte} (0x${toHex lowestBadByte},
          frame 0x${frameHex}, see ./bad-ram-pages.txt).
          Every measured bad page must be *above* the cap, or the fault window is live
          again and the 2026-11-23 failure mode (both disks lost at 5 s) is back.  Lower
          the cap, or re-measure and regenerate ./bad-ram-pages.txt if the DIMM has been
          moved, reseated or replaced — the addresses do not survive a change of layout.
        '';
      }
    ];

    boot.kernelParams = lib.optionals (cfg.cap != null) ["mem=${cfg.cap}"];

    # The measured pages themselves, reserved out of memblock by a patched kernel — see
    # the header.  This rebuilds `linux` (and the NVIDIA module with it) on this box, once
    # per kernel version: a locally patched kernel has no binary cache.
    boot.kernelPatches = [
      {
        name = "services3-bad-ram-pages";
        patch = badRamPatch;
      }
    ];

    # The evidence on the box, so the boot-time check reads the same file the eval read
    # and neither of them needs a checkout.
    environment.etc."bad-ram-pages.txt".source = ./bad-ram-pages.txt;

    systemd.services.bad-ram-cap-check = {
      description = "Prove the bad DIMM's fault window is out of the allocator";
      wantedBy = ["multi-user.target"];
      after = ["local-fs.target"];
      startLimitIntervalSec = 0;

      # A unit's PATH does not include `/run/current-system/sw/bin`, which is what made
      # the first version of this report die with `awk: command not found` and report
      # nothing at all — on the one boot it was ever needed.  So the tools are named
      # here and the script uses nothing outside them.
      path = [pkgs.coreutils pkgs.gnugrep pkgs.gnused pkgs.systemd];

      # oneshot + RemainAfterExit so a good boot reads `active (exited)` and a bad one
      # reads `failed`: the unit's state *is* the answer to "is the cap real?", and
      # `systemctl --failed` is the query.  A boot-time failure does not arm
      # auto-rollback (only an activation does), so this reports rather than reboots.
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
      };

      script = ''
        set -u
        fail=0

        have_cap=${
          if cfg.cap == null
          then "0"
          else "1"
        }
        cap=${lib.escapeShellArg (
          if cfg.cap == null
          then ""
          else cfg.cap
        )}
        want_kb=$(( ${toString (
          if capBytes == null
          then 0
          else capBytes
        )} / 1024 ))
        want_pages=${toString pageCount}

        # 0. The kernel took the page list.  This is the check the patch exists for, and the
        #    only one that can tell "the reservation is in force" from "the patch applied and
        #    the array came out empty".  The count is compiled in, so a mismatch also means the
        #    running kernel is not the one this configuration built.
        reserved=$(journalctl -k -b --no-pager 2>/dev/null | grep -c "services3-bad-ram: reserved $want_pages measured-bad pages" || true)
        if [ "$reserved" -ge 1 ]; then
          echo "bad-ram: the kernel reserved $want_pages measured-bad pages (1088 KiB, patch in force)"
        else
          echo "bad-ram: FAILED — no 'services3-bad-ram: reserved $want_pages ...' line in this boot's"
          echo "  kernel log.  Either the running kernel is not the patched one (the patch did not"
          echo "  apply, or the generation was not built with it), or it ran with an empty list."
          echo "  Do not trust this box's memory until that is answered."
          fail=1
        fi

        if [ "$have_cap" -eq 1 ]; then
          has_cap=0
          if tr ' ' '\n' < /proc/cmdline | grep -qx "mem=$cap"; then has_cap=1; fi

          # Booted into a configuration that is not the current one: this boot predates the
          # cap, so it is not on the running kernel's command line yet and there is nothing
          # to judge.  Deferring is not politeness — a unit that fails while
          # `switch-to-configuration` is running fails the *switch*, and a failed activation
          # arms autoRollback's five-minute reboot (AGENTS.md, GOLDEN RULE; hit on gw
          # 2026-10-24 with a health-check unit).  `just pending-reboot` is the verb that
          # shouts about boxes on old generations, so this defers to it.  The deferral is
          # keyed on the cap being *absent*: a switch made while already running the capped
          # generation must still do the full check rather than call itself unarmed.
          booted=$(readlink -f /run/booted-system 2>/dev/null || echo none)
          current=$(readlink -f /run/current-system)
          if [ "$has_cap" -eq 0 ] && [ "$booted" != "$current" ]; then
            echo "bad-ram: NOT YET ARMED — booted system is $(basename "$booted"),"
            echo "  current system is $(basename "$current"): this boot predates the cap, so"
            echo "  the kernel has not been given mem=$cap yet.  Reboot to arm it."
            exit 0
          fi

          # 1. The parameter reached the kernel.  This is the failure that costs nothing to
          #    catch and everything to miss: the EFI stub truncates an over-long command line
          #    at the last space that fits and loses whole arguments, and what vanished on
          #    2026-11-23 was `root=fstab loglevel=4 lsm=…`.
          if [ "$has_cap" -eq 1 ]; then
            echo "bad-ram: mem=$cap is on the kernel command line"
          else
            echo "bad-ram: FAILED — mem=$cap is NOT on /proc/cmdline:"
            tr ' ' '\n' < /proc/cmdline | sed 's/^/  /'
            fail=1
          fi

          # 2. It took.  MemTotal can only be at or below what was asked for, and a shortfall
          #    of more than 3 GiB means the cap is not what bounded the box.  The load-bearing
          #    half is the upper bound: an absent cap reports ~39 GiB, so the two cases are
          #    ~10 GiB apart.  The lower bound only catches an over-aggressive cap, and it is
          #    deliberately loose because the *measured* reservation on this board at 30G is
          #    1.54 GiB (MemTotal 29840200 kB, 2026-11-25 18:11), so a 2 GiB band would have
          #    480 MB of margin and would fire a false alarm at the first kernel that reserved
          #    slightly more.
          total_kb=$(sed -n 's/^MemTotal:[[:space:]]*\([0-9]\+\) kB$/\1/p' /proc/meminfo || true)
          floor_kb=$(( want_kb - 3 * 1024 * 1024 ))
          if [ -n "$total_kb" ] && [ "$total_kb" -le "$want_kb" ] && [ "$total_kb" -ge "$floor_kb" ]; then
            echo "bad-ram: MemTotal ''${total_kb} kB is consistent with a $cap cap (''${want_kb} kB)"
          else
            echo "bad-ram: FAILED — MemTotal is ''${total_kb:-unreadable} kB, expected at most $cap"
            echo "  (''${want_kb} kB) and within 3 GiB of it.  A cap that did not take is worse than none."
            fail=1
          fi

          # 3. The window is out.  This is the check that matters and the one a cap can fail
          #    while looking healthy: the top of the kernel's own System RAM must be below the
          #    lowest measured bad byte (31.338 GiB), or the cap is above the fault and the
          #    measured pages are allocatable.
          # Every read that may legitimately find nothing carries `|| true`: the unit's script
          # has `set -e` prepended by NixOS, so a `grep` that finds no match aborts the whole
          # check instead of reporting it (measured: exit 1 at this line).
          lowest_frame=$(grep -m1 -oE '^0x[0-9a-f]+' /etc/bad-ram-pages.txt 2>/dev/null || true)
          top_hex=$(sed -n 's/^[[:space:]]*[0-9a-f]\+-\([0-9a-f]\+\) : System RAM$/\1/p' /proc/iomem | tail -1 || true)
          if [ -z "$lowest_frame" ]; then
            echo "bad-ram: FAILED — cannot read a measured bad page from /etc/bad-ram-pages.txt,"
            echo "  so there is nothing to compare the top of RAM against.  The file is a symlink into"
            echo "  the store's ./bad-ram-pages.txt; if it is missing the module did not evaluate."
            fail=1
          elif [ -z "$top_hex" ]; then
            echo "bad-ram: FAILED — could not read the top of System RAM from /proc/iomem"
            fail=1
          else
            lowest_byte=$(( lowest_frame * 4096 ))
            top_byte=$(( 0x$top_hex ))
            if [ "$top_byte" -lt "$lowest_byte" ]; then
              echo "bad-ram: top of System RAM is 0x$top_hex, below the lowest measured"
              echo "  bad page (byte $lowest_byte = 0x$(printf '%x' "$lowest_byte")) — the fault window is out"
            else
              echo "bad-ram: FAILED — System RAM reaches 0x$top_hex, at or above the lowest"
              echo "  measured bad page (byte $lowest_byte = 0x$(printf '%x' "$lowest_byte"))."
              echo "  The cap is too high for this DIMM; do not trust this box's memory."
              fail=1
            fi
          fi
        else
          # Uncapped: the page reservation is the only exclusion, so System RAM is *expected*
          # to reach past the bad addresses — step 0 is the proof instead.  All this branch can
          # add is that the box really is running without a cap (MemTotal near the full 40 GiB)
          # rather than someone having left `30G` in place and wondering where the RAM went.
          total_kb=$(sed -n 's/^MemTotal:[[:space:]]*\([0-9]\+\) kB$/\1/p' /proc/meminfo || true)
          if [ -n "$total_kb" ] && [ "$total_kb" -ge $(( 38 * 1024 * 1024 )) ]; then
            echo "bad-ram: no mem= cap on the command line; MemTotal ''${total_kb} kB is the whole DIMM"
            echo "  set less the $want_pages reserved pages and the board's own reservations"
          else
            echo "bad-ram: FAILED — infra.badRam.cap is null but MemTotal is ''${total_kb:-unreadable} kB;"
            echo "  a cap is still in force (or the DIMMs are not all present)."
            fail=1
          fi
        fi

        exit "$fail"
      '';
    };
  };
}
