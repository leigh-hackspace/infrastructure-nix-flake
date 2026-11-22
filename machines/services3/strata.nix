# Strata on services3 — Qwen3.8-Flash-Next **Coder** (IQ1_M) on the GTX 1080 Ti.
#
# Third Strata instance in the network and the one on the oldest card: services2
# runs the same pack on an RTX 3060 (CUDA 13 / sm_86) and aibox runs the full 125B
# model on a Radeon iGPU (HIP / gfx1030). This one is **CUDA 12.8 / sm_61
# (Pascal)** — CUDA 13 cannot compile for GP102 at all — and ./strata-package.nix
# is where that decision lives and is explained. Read that header and
# machines/services2/STRATA.md before changing an engine flag here; the flags
# below are services2's, re-sized for this box, and every *variance* from them is
# called out.
#
# ## Why the Coder pack, on this box
#
# The GPU is 11 GB of VRAM and the box has 40 GB of RAM (measured 2026-11-21:
# `MemTotal: 41999736 kB`; ./README.md's "16 GiB" row was the box as it arrived
# and is stale). The Coder pack's expert arena is ~23.4 GiB, which is the only
# pack that leaves ~10 GB for the OS on this machine — the full model's Q2_0
# arena is 34 GiB and does not fit beside anything. It is also the same pack
# services2 serves, so the two endpoints answer the same model and a client can
# be pointed at either.
#
# ## Where the data lives (and why not in the store)
#
# The 476 GB NVMe (`nvme0n1`, SK hynix PC601, wiped 2026-11-21 — it still held a
# previous owner's ZFS pool, and the whole disk is now one ext4 filesystem,
# label `STRATA`, mounted at `/srv/strata`: ./hardware-configuration.nix). The
# GGUF shards, the prepared pack and the engine log are all there:
#
#   /srv/strata/models/Qwen3.8-Flash-Next-GSQ-RCO-Coder/   54.4 GB of GGUF
#   /srv/strata/pack/coder-iq1m                            the pack (~1.5 GB)
#   /srv/strata/mtp                                        the MTP draft (~6 GB)
#
# None of it is in the store, and none of it is deployed: it is prepared ONCE by
# hand, and the unit carries `ConditionPathExists` on the pack index so it will
# not crash-loop before that exists. The hand-run recipe, in full:
#
#   STRATA=$(cd ~/Projects/infrastructure-nix-flake && nix build .#strata-cuda12 --no-link --print-out-paths)
#
#   # 0. is the card what this build thinks it is? (must print sm_61 / 1080 Ti)
#   $STRATA/bin/strata-device --list-devices
#   $STRATA/bin/strata-device --selftest
#
#   # 1. the pack (index + dense tensors + tokenizer, ~1.5 GB):
#   mkdir -p /srv/strata/pack/coder-iq1m
#   $STRATA/bin/strata-prep iq_pack.py \
#     --gguf /srv/strata/models/Qwen3.8-Flash-Next-GSQ-RCO-Coder/Qwen3.8-Flash-Next-GSQ-RCO-IQ1_M-00001-of-00002.gguf \
#     --out /srv/strata/pack/coder-iq1m
#
#   # 2. the MTP draft runtime (OPTIONAL, and off below — see `useMtp`):
#   $STRATA/bin/strata-prep mtp_fetch.py fetch --out /srv/strata/mtp
#   $STRATA/bin/strata-prep mtp_fetch.py verify --out /srv/strata/mtp
#   $STRATA/bin/strata-prep mtp_pack.py --src /srv/strata/mtp \
#     --experts q2_0 --out /srv/strata/mtp/mtp-q2_0.gguf
#   $STRATA/bin/strata-prep mtp_rt.py --gguf /srv/strata/mtp/mtp-q2_0.gguf --out /srv/strata/mtp/rt
#   cp $STRATA/share/strata/data/draft_vocab.bin /srv/strata/mtp/rt/
#
# ## Access
#
# `http://10.3.2.213:8080/v1` (this box's own address from common/hosts.nix — it
# is the Wi-Fi dongle's, VLAN 226), with `Authorization: Bearer <strata_api_key>`
# on every `/v1/*` call; `/health` needs none. The door row is
# `strata3.int.leighhack.org` (common/front-door.nix, `strataBearer = true`), which
# is how every client should reach it: the engine's own Host check and the front
# door's key injection are both satisfied by that name, and the box binds its LAN
# address rather than loopback because the door is another box on another VLAN
# (the argument ./llama.nix makes at length, and `services3` has no firewall).
#
# One caveat about `allowed_hosts` below, measured from `gw` on 2026-11-22 rather than
# read off the code: `curl` to this port answers **200 on `/health` with any `Host`,
# including `Host: example.com`**. That is the behaviour services2's module already
# records — upstream runs the Host check only when the engine has *no* key, and this
# engine has one — so listing the name here is documentation of intent, not the gate.
# The gate is the LAN ACL on the door row plus the key the door injects; do not cite
# `allowed_hosts` as a control in anything that needs one.
{
  config,
  lib,
  pkgs,
  INFRA,
  strata,
  ...
}: let
  HOSTS = import ../../common/hosts.nix;
  cfg = config.services.strata3;

  # The CUDA 12.8 / sm_61 engine. Same source as services2's and aibox's (one
  # `strata` input, so the three cannot drift), a different toolkit and arch. The
  # input is `flake = false` since upstream deleted its root flake.nix, so it *is*
  # the source tree — there is no `.outPath` to take (machines/services2/strata.nix
  # passes it the same way).
  strataPkg = pkgs.callPackage ./strata-package.nix {
    src = strata;
    stdenv = pkgs.cudaPackages_12_8.backendStdenv;
    cudaPackages = pkgs.cudaPackages_12_8;
  };

  # Where the hand-prepared data lives. One root, on the NVMe.
  dataRoot = "/srv/strata";
  ggufDir = "${dataRoot}/models/Qwen3.8-Flash-Next-GSQ-RCO-Coder";
  packDir = "${dataRoot}/pack/coder-iq1m";
  nativeGguf = "${ggufDir}/Qwen3.8-Flash-Next-GSQ-RCO-IQ1_M-00001-of-00002.gguf";
  pleGguf = "${ggufDir}/Qwen3.8-Flash-Next-GSQ-RCO-IQ1_M-00002-of-00002.gguf";
  mtpDir = "${dataRoot}/mtp/rt";
  # The Coder pack's own routing profile ships in the source tree.
  expertProfile = "${strataPkg}/share/strata/data/expert-profile-coder.bin";

  # The MTP draft head. **Off on the first bring-up on purpose**: it costs
  # roughly a gigabyte of the 11 GB of VRAM that the expert cache would
  # otherwise have, and this box has ~4x less VRAM than the card Strata's own
  # numbers come from. Turn it on only after a baseline decode rate exists to
  # beat — `--spec 4 --spec-min-p 0.5` — and measure it; on a card this slow it
  # is a real candidate for a win, but an unmeasured one.
  useMtp = false;

  # VRAM budget, written down because every number here is a choice:
  #   dense weights + buffers   ~5 GiB   (services2's measured figure for this pack)
  #   KV int8 @ 131072          ~1.8 GiB (services2's measured slope for this pack)
  #   expert cache              auto     = whatever is left (~3.6 GiB ⇒ ~2,700 slots)
  #   free with everything up   see the log's "VRAM free with everything loaded" line
  #
  # **131072 since 2026-11-21, raised from the 32768 it first booted at.** The first
  # boot measured 10,447 MiB of the card's 11,264 used with 716 MiB free at 32K, and
  # `--expert-cache auto` re-sizes from what is left rather than refusing, so the
  # extra ~1.35 GiB of KV is taken out of the cache: ~4,100 slots become ~2,700, which
  # is where a token's missed expert goes from a VRAM read to a RAM or SSD read. That
  # is the price, and it is paid in decode rate, not in correctness. The model is
  # trained well past this (services2 serves the same pack at 131072), and the RAM
  # side does not move — the KV stays in VRAM, which is what `setup.py`'s
  # `kv_streaming_wanted` rule says once the arena is resident.
  #
  # Watch the engine's own warning after the load: a `LOW: requests may stall; add
  # --vram-reserve-mib` line means the cache took too much of the card and this is one
  # line to walk back (65536 halves the KV and gives back ~0.9 GiB).
  context = 131072;

  engineArgs =
    [
      "--pack"
      packDir
      "--native"
      nativeGguf
      "--ple-gguf"
      pleGguf
      "--expert-profile"
      expertProfile
      "--expert-cache"
      "auto"
      "--prefill"
      "auto"
      "--max-context"
      (toString context)
      "--kv"
      "int8"
      # **The K/V's *tier* — and why nothing is spelled for it here.** On 2026-11-22
      # someone read the live start and thought the K/V should stop living in VRAM,
      # so `--kv-resident 32768` was written into this list; it was **reverted the
      # same day**, unrewarded, before any switch of this box carried it. What the
      # live log said then, and says still, is that this box is already at the top of
      # its own ladder: `FileExpertSource: 1690 of the prompt path's 1690 lendable
      # slots keep their experts in RAM too` and `resident RAM mode: … (no file
      # reads)` — see `machines/services3/README.md` ("The RAM tier, measured") for
      # the whole ladder and both re-verify blocks. `git log -S --kv-unloaded --
      # machines/services3/` answers nothing: the `--kv-unloaded 16384 16384 0 0`
      # the README quotes was applied by hand at a console, never by this module,
      # so it is not in this start and cannot be assumed. If the K/V tier is wanted
      # here, measure it: read the `expert cache auto: N GiB free … -> N slots` line
      # before and the `lendable slots` line after, and land it as its own commit —
      # never inside one whose subject is something else.
      # Low-RAM mode: the arena (23.4 GiB) is larger than this card, so the
      # experts the GPU cache does not hold are copied into RAM once
      # (--resident-experts) instead of being read back through the file cache
      # (--mmap-experts). 40 GB of RAM is what makes that the right trade here,
      # and it is the mode upstream's own docs name for "a 32 GB PC with a
      # 12 to 16 GB GPU runs the Coder".
      "--resident-experts"
      # **The verify window, and it is not optional on a native (IQ) pack.** The engine
      # refuses one without `--spec T (T >= 2)` — measured 2026-11-21: `strata
      # generate: ... is a native (IQ) pack: it needs --native SHARD1, --spec T (T >= 2)
      # and --prefill CHUNK`, with `--prefill auto` resolving later and `--native`
      # already given, so `--spec` was the only thing missing. It is the speculative
      # window width, and it drives the **prompt-lookup (suffix) drafter**, which is on
      # by default and needs no MTP weights — so a window is useful here even with
      # `useMtp = false`. `--spec-min-p` is the acceptance floor the draft policy uses
      # before it will spend a verify row.
      "--spec"
      "4"
      "--spec-min-p"
      "0.5"
    ]
    ++ lib.optionals useMtp [
      "--mtp"
      mtpDir
    ];

  apiKey = builtins.readFile (config.sopsSecretText "strata_api_key");

  configFile = pkgs.writeText "strata-coder-iq1m-sm61.json" (
    builtins.toJSON {
      exe = "${strataPkg}/bin/strata";
      args = engineArgs;
      cwd = "/var/lib/strata";
      tokenizer = "${packDir}/tokenizer";
      model_name = "qwen3.8-flash-next-coder-iq1m";
      log = "/var/lib/strata/strata.log";
      allowed_hosts = [
        "10.3.2.213"
        "services3"
        "strata3.int.leighhack.org"
      ];
      api_key = apiKey;
      backend = "cuda";
      # libcuda.so.1 is the driver's, which NixOS exposes here; the toolkit's own
      # libraries are on the binary's RUNPATH.
      lib_dirs = ["/run/opengl-driver/lib"];
      env.CUDA_VISIBLE_DEVICES = "0";
      # **The one card-specific switch that matters on Pascal.** A consumer
      # Pascal GPU (GP102 included) runs FP16 arithmetic at 1/64 of its FP32
      # rate, so cuBLAS's FP16-operand GEMMs are the slowest thing on the card
      # — this makes the large dense projections `cublasSgemm` on FP32 copies
      # instead, which is the same route upstream measured as a large prompt win
      # on a Tesla P40 (the other sm_61 card: 430-474 tok/s from 360-432).
      # Upstream says to run `gemm_f32_parity` (a `-DSTRATA_BUILD_TESTS=ON`
      # build) against your toolkit's cuBLAS before setting it, because the
      # equality is a property of the cuBLAS build; that check has NOT been run
      # here, and the honest statement is "faster, rounding-equivalent by
      # upstream's measurement on another sm_61 card, unverified on ours".
      # Revert is deleting the line.
      env.STRATA_DENSE_F32 = "1";
    }
  );
in {
  options.services.strata3 = {
    enable = lib.mkEnableOption "Strata (Qwen3.8-Flash-Next Coder) on services3's GTX 1080 Ti (CUDA 12.8 / sm_61)";
  };

  config = lib.mkIf cfg.enable {
    # The engine's arena is shared memory by `--resident-experts`, and shmem is
    # swappable; with no swap on this box the setting only decides what the
    # kernel reaches for under page-cache pressure. It is services2's dial for
    # the same reason (that file measured it) and it costs nothing to agree.
    boot.kernel.sysctl."vm.swappiness" = 10;

    # A driver bump + a `switch` leaves the old NVIDIA module under new
    # user-space; strata then cannot reach the card and `strata3.int` 502s with
    # every unit healthy. The guard makes the unit say so, and mkNeverGiveUp
    # below keeps it retrying until the reboot. common/gpu-driver-match.nix.
    infra.nvidiaDriverMatchGuard = {
      enable = true;
      units = ["strata"];
    };

    systemd.services.strata = INFRA.mkNeverGiveUp {
      description = "Strata (Qwen3.8-Flash-Next Coder across the GTX 1080 Ti, RAM and NVMe)";
      after = ["network-online.target"];
      wants = ["network-online.target"];
      wantedBy = ["multi-user.target"];

      # Don't crash-loop before the hand-run model prep has produced a pack.
      unitConfig.ConditionPathExists = "${packDir}/index.txt";
      # The pack is on a separate filesystem, so the condition and the engine both have
      # to come after it is mounted. Without this, a boot can evaluate
      # ConditionPathExists against the still-empty mountpoint, skip the unit as "not
      # met" and leave an engine that never starts and nothing failed.
      unitConfig.RequiresMountsFor = dataRoot;

      serviceConfig = {
        ExecStart = "${strataPkg}/bin/strata-server --engine strata --config ${configFile} --host ${HOSTS.services3} --port 8080";
        StateDirectory = "strata";
        RestartSec = 10;
        # Loading the model is a multi-minute, tens-of-GB operation.
        TimeoutStartSec = "infinity";
        TimeoutStopSec = 120;
        # The resident expert arena is page-locked where it can be.
        LimitMEMLOCK = "infinity";
      };
    };
  };
}
