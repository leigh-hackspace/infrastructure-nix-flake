# Strata on aibox: Qwen3.8-Flash-Next (125B total / 6B active MoE) on the
# Radeon 660M iGPU + system RAM + NVMe.  Served on the LAN as
# https://strata2.int.leighhack.org (a row in `common/front-door.nix`, terminated by
# `gw`) and, unproxied, on http://10.3.1.32:8080/v1.
#
# Disabled 2026-10-17, re-enabled 2026-10-28 — deliberately not written as a `# PARKED`
# marker, because this module *is* imported and rule 23 makes that grep the list of what
# is not running.  It was disabled to make this box the
# network's builder, because the expert arena is ~33 GiB of RAM a build cannot have; it
# is back on because the engine is what this box was bought for, and on 2026-11-23 the
# builder duty went away altogether (common/nix-gc.nix's header), so the two no longer
# compete for the same 59 GiB — except when this box builds for itself, which is bounded
# in ./configuration.nix and protected here by `MemoryMin`/`OOMScoreAdjust` below.  Read
# ./STRATA.md before changing expertCache, context or the runtime —
# it is the measurement record, and the two are not independent (the "plateau"
# section: every cache/placement/draft configuration measured lands at
# 225-231 ms/window = 9.4-9.6 tok/s, because both halves are compute-bound).
#
# What the 2026-10-28 un-parking changed, in one paragraph.  The engine now builds
# from the flake's `strata` input instead of its own nine-releases-stale pin
# (./strata-package.nix), so it moves with `nix flake update strata` like services2's
# CUDA engine does; the endpoint is a front-door name rather than a location on this
# box's own nginx (this box has no nginx — `services.nginx.virtualHosts` here would
# have created a second, unguarded door, which is what `common/front-door.nix`
# exists to prevent); the engine has an `api_key` (below); and `services.strata.
# rocm10Runtime` can swap nixpkgs' ROCm 7.2 for the ROCm 10 userspace runtime
# (./rocm10-runtime.nix), which is the only known mitigation for the #879
# poisoned-context class.  What it did NOT change: the flags, which are the measured
# config, and the fact that llama-server and strata cannot be resident at once.
#
# Why this can work on a box with no VRAM:
#   Strata is not an offloader. Its tiers are: experts resident in RAM (the arena),
#   a GPU expert cache for the hot ones, a CPU pool for the cold ones, and the 28.8
#   GB n-gram (PLE) table read unbuffered from the SSD. On an iGPU every "VRAM"
#   allocation is GTT, i.e. the SAME RAM as the arena - so the GPU tier adds
#   compute, not bandwidth. Decode is bandwidth-bound: ~2.7 GB of weights read per
#   token at ~50 GB/s. A dense 27B reads ~21 GB per token, which is exactly why
#   llama.cpp sits at 2 tok/s here; this model reads ~8x less per token.
#
# Card/arch: rocminfo says gfx1035, which Strata's cmake refuses (it accepts
# gfx1100/gfx1101/gfx1200/gfx1201 validated, gfx1101/gfx1200 community,
# gfx1012/gfx1102/gfx1030/gfx1031 unvalidated - and still refuses gfx1035 at 0.1.52).
# gfx1035 is RDNA2 with the same wave32, 64 KiB LDS and v_dot4_i32_i8 ISA as gfx1030,
# so the engine is built for gfx1030 and HSA_OVERRIDE_GFX_VERSION=10.3.0 makes the
# runtime report gfx1030 (the same override this box's ROCm torch setups use for the
# 660M).
#
# ⚠️ llama-server and strata CANNOT be resident at once: llama-server holds ~33 GB of
# GTT for its model, and the IQ2_XS arena alone is 35.5 GiB. This module therefore
# clears llama-server's wantedBy (see displaceLlamaServer) so the box boots into
# Strata instead; `systemctl stop strata && systemctl start llama-server` still works
# for the small models.  It is a real trade, not a free one: `ai.int.leighhack.org`
# and `ai.leighhack.org` (origin vhosts on services1) proxy to 10.3.1.32:8081, i.e. to
# llama-server, and 502 while strata has the box.  ./STRATA.md "Operational notes"
# has the numbers for swapping the two by hand.
#
# Model data is NOT in the store. It is prepared ONCE by hand before this unit will
# start (the unit has ConditionPathExists on the pack index), and it SURVIVED the
# parking - the flake never owned it.  On this box it is already there:
# /home/leigh-admin/Models/Qwen3.8-Flash-Next-GSQ-RCO-GGUF (64 GB) +
# /home/leigh-admin/Strata/{pack/iq2xs,mtp/rt}, prepared 2026-10-05.  The pack and MTP
# runtime do NOT have to be re-made for the 0.1.52 engine: `data/expert-profile.bin`,
# `data/expert-profile-coder.bin` and `data/draft_vocab.bin` are byte-identical at
# 0.1.43.1 and 0.1.52 (sha256 of all three re-compared on 2026-10-28), and the pinned
# llama.cpp - the other thing a pack depends on - is `3cf03257` at both tags.  To do
# it from scratch (or for a different quant):
#
#   # from the checkout, and NEVER through `builtins.getFlake "/abs/path"`: a path
#   # ref copies the whole working tree (7+ GB of cargo/dioxus target dirs + .git, not
#   # just the 26 MB of source) into the store on every call - see
#   # docs/nix-dev-workflow.md.
#   STRATA=$(cd ~/Projects/infrastructure-nix-flake && nix build .#strata --no-link --print-out-paths)
#   mkdir -p /home/leigh-admin/Models/Qwen3.8-Flash-Next-GSQ-RCO-GGUF/IQ2_XS
#   cd /home/leigh-admin/Models/Qwen3.8-Flash-Next-GSQ-RCO-GGUF/IQ2_XS
#   # 1. the two shards (68 GB total; shard 2 is the single PLE table):
#   for i in 1 2; do curl -L -C - -o Qwen3.8-Flash-Next-GSQ-RCO-IQ2_XS-0000$i-of-00002.gguf \
#     "https://huggingface.co/ISTA-DASLab/Qwen3.8-Flash-Next-GSQ-RCO-GGUF/resolve/main/IQ2_XS/Qwen3.8-Flash-Next-GSQ-RCO-IQ2_XS-0000$i-of-00002.gguf"; done
#   # 2. the pack (index + dense tensors + tokenizer, ~1.5 GB):
#   $STRATA/bin/strata-prep iq_pack.py --gguf .../IQ2_XS/...-00001-of-00002.gguf \
#     --out /home/leigh-admin/Strata/pack/iq2xs
#   # 3. the MTP draft runtime (~6 GB; the GSQ-RCO GGUF ships NO MTP head, so
#   #    the 31 mtp.* tensors always come from the BF16 checkpoint):
#   $STRATA/bin/strata-prep mtp_fetch.py fetch --out /home/leigh-admin/Strata/mtp
#   $STRATA/bin/strata-prep mtp_fetch.py verify --out /home/leigh-admin/Strata/mtp
#   $STRATA/bin/strata-prep mtp_pack.py --src /home/leigh-admin/Strata/mtp \
#     --experts q2_0 --out /home/leigh-admin/Strata/mtp/mtp-q2_0.gguf
#   $STRATA/bin/strata-prep mtp_rt.py --gguf /home/leigh-admin/Strata/mtp/mtp-q2_0.gguf \
#     --out /home/leigh-admin/Strata/mtp/rt
#   cp $STRATA/share/strata/data/draft_vocab.bin /home/leigh-admin/Strata/mtp/rt/
#
# Check the card first (before downloading anything), and again after any change to
# the engine or the runtime, because it costs seconds and is the only cheap proof the
# GPU path is alive at all:
#   $STRATA/bin/strata-device --list-devices   # with HSA_OVERRIDE_GFX_VERSION=10.3.0
#   $STRATA/bin/strata-device --selftest
#
# Logs: journalctl -u strata -f and /var/lib/strata/strata.log.  The engine binds
# 10.3.1.32 (its --host), NOT loopback, so a proxy_pass to 127.0.0.1:8080 is a silent
# 502 - and that address is reachable on the LAN because this box's firewall is off
# (machines/aibox/networking.nix), which is half the reason it has an api_key.
{
  config,
  lib,
  pkgs,
  INFRA,
  # The flake's `strata` input (upstream main), which arrives through specialArgs
  # (flakeInputs) exactly as it does for machines/services2/strata.nix. Since
  # upstream's 0.2.16 layout move the input is `flake = false`, so what arrives is
  # the source path itself - the tree both engines build from.
  strata,
  ...
}: let
  cfg = config.services.strata;

  # The engine, built from the flake's `strata` input (upstream main) - the same
  # source services2's CUDA engine builds from, so the two cannot drift by nine
  # releases again.
  strataPkg = pkgs.callPackage ./strata-package.nix {
    src = strata;
  };

  # The ROCm 10.0.0 (TheRock) userspace runtime.  Only reaches the engine when
  # cfg.rocm10Runtime is on; ./rocm10-runtime.nix explains the fetch, the size, and
  # why nixpkgs cannot express this (it has exactly one ROCm).
  rocm10 = pkgs.callPackage ./rocm10-runtime.nix {};

  # Model layout. The GGUFs stay under ~/Models with everything else there;
  # only the hand-prepared pack and MTP runtime live under modelDir.
  ggufDir = "/home/leigh-admin/Models/Qwen3.8-Flash-Next-GSQ-RCO-GGUF";
  modelDir = "/home/leigh-admin/Strata";
  packDir = "${modelDir}/pack/iq2xs";
  nativeGguf = "${ggufDir}/IQ2_XS/Qwen3.8-Flash-Next-GSQ-RCO-IQ2_XS-00001-of-00002.gguf";
  pleGguf = "${ggufDir}/IQ2_XS/Qwen3.8-Flash-Next-GSQ-RCO-IQ2_XS-00002-of-00002.gguf";
  mtpDir = "${modelDir}/mtp/rt";
  expertProfile = "${strataPkg}/share/strata/data/expert-profile.bin";

  # aibox's budget (59.6 GiB RAM, no dedicated VRAM). Measured with
  # cat /sys/class/drm/card0/device/mem_info_gtt_used and `free`:
  #   experts (IQ2_XS arena)  33.02 GiB  always in RAM (engine: "experts loaded:
  #                            33.02 GiB at ~4.9 GiB/s", 12 s)
  #   expert cache            4096 slots ~5.7 GiB GTT (each expert blob is
  #                            1,382,400 B; 2132 slots = 2.88 GiB measured, 8607
  #                            = 11.52 GiB). GTT is the SAME RAM as the arena, so
  #                            this buys GPU compute, not bandwidth - see STRATA.md.
  #                            NEVER "auto": it reads hipMemGetInfo's ~52 GiB of
  #                            free GTT and fills the machine.
  #   dense weights + buffers  ~4 GiB    (GTT)
  #   KV int8 at 131072        ~1.8 GiB  (13.7 KB/token; --kv-resident keeps
  #                            32768 cells/QSA layer on the GPU, the rest in RAM)
  #   PLE table                0         (28.8 GB read from NVMe, --ple-io direct)
  # Measured total: 8.66 GiB GTT + 46 GiB RAM used at expertCache=2048; 17.9 GiB
  # GTT + 54 GiB at 8192 (only 4.7 GiB left - too tight for a box that also runs
  # frigate/immich/whisper). 4096 lands at ~50 GiB used, ~10 GiB headroom - which is the
  # 131072-context figure; measured with this unit's 262144 context loaded on 2026-10-28
  # it is 54 GiB used, 6.9 GiB available, 15.1 GiB of GTT. Size what else shares the box
  # off the 6.9, not the 10.
  # Raise --max-context only with the RAM for it: 262144 costs ~3.6 GiB of KV,
  # 524288 ~7.2 GiB + YaRN 2.
  #
  # That headroom is now shared with the build farm (this box builds for the whole
  # network), which is the honest cost of un-parking: a nix build and the arena want
  # the same DDR5.  expertCache is the dial - 2048 frees ~2.9 GiB for ~10 % decode
  # (./STRATA.md's table) - and `--conversation-cache-mib 0` below is the other one,
  # because leaving it unset lets a future release reserve from what is left.
  context = 262144;
  expertCache = 4096;

  # Explicit chunk, not "auto": auto picks the largest chunk the expert cache can
  # LEND (4096 at 2048 slots). Measured prefill was 44.6 tok/s at 4096 and 46.0 at
  # 8192 - the chunk is not what limits prefill here, but asking for 8192 costs
  # nothing and is strictly better.  Keep it explicit: a bare `auto` may reach 16384
  # on gfx12 (0.1.44), and this engine reports gfx1030, so the only honest spelling
  # of "what chunk do I run" is the number.
  prefillChunk = "8192";

  engineArgs = [
    "--pack"
    packDir
    "--native"
    nativeGguf
    "--ple-gguf"
    pleGguf
    # Explicit, not the default it replaces. 0.1.42's start-up probe (#1425, #1549, #1629)
    # switched the 28.8 GB n-gram table to `mmap` by itself below 15 000 rows/s; 0.1.43 stopped
    # making that choice on any platform (#2040: a SATA SSD tops out at 10-14K rows/s with
    # direct's 16 reads in flight and mapped reads were 7x slower there), so the probe now only
    # warns and the switch is opt-in STRATA_PLE_AUTO_MMAP=1. The flag is therefore no longer a
    # guard against the probe - it is the measured config stated out loud, and it stays for that:
    # mmap would put a 28.8 GB table in the page cache on a box that runs with ~5 GiB available,
    # and STRATA.md's "do not chase the PLE table" still holds (waitA 0.24 ms: no I/O stall in
    # the profile).
    "--ple-io"
    "direct"
    "--expert-profile"
    expertProfile
    "--expert-cache"
    (toString expertCache)
    "--prefill"
    prefillChunk
    "--spec"
    "4"
    "--spec-min-p"
    "0.5"
    "--mtp"
    mtpDir
    "--max-context"
    (toString context)
    "--kv"
    "int8"
    "--kv-resident"
    "32768"
    # Parked conversations, on - but read the measured line before you turn this knob on
    # somewhere else, because it is not what it was bought for.  Deployed and measured on
    # this box 2026-10-28 (./STRATA.md, "Prefill 2.2x, and what the conversation cache
    # actually parks"): a REPEATED prompt costs 1.2 s instead of 158 s
    # (`restored 13064 tokens (checkpoint) in 37 ms`), and a prompt that has GROWN by 30
    # tokens costs the full 157.5 s with `reused 0` - parking matches on prompt identity,
    # not on prefix, so agent-turn traffic (prompt_t + new text every turn) gets nothing
    # from it.  It stays on because it costs nothing until something repeats (`bytes=0`
    # idle), retries and re-issued identical prompts are real here, and it demonstrably
    # did not slow the non-repeat case.  It is NOT the answer to long agent contexts.
    #
    # Sizing, measured: one parked 13K-token conversation is 436 MB
    # (`snapshot_bytes` in the engine's own accounting), so the 1536 MiB cap holds about
    # three of them - eviction at the cap was observed at `parked=3 bytes=1.22 GiB`, which
    # is the LRU doing its job.  1536 is also exactly what 0.1.48.1's own auto rule (#3493:
    # 40% of available - 2560 MiB, 256 MiB steps) computes at this box's measured 6964 MiB
    # available with the arena loaded - the auto default made explicit so a restart cannot
    # silently pick another number when the load changes shape.
    #
    # Why it cannot hurt the build farm: the budget is a CAP and every park is refused
    # unless MemAvailable covers it plus `--conversation-cache-min-free-mib` (default 2560)
    # - `conversation_memory_admit(available, allocation, floor)` in
    # include/strata/core/conversation_memory.hpp - so while a nix build runs, the engine
    # parks nothing and a would-be resume is a fresh read.  Degrades, never fails.
    # `available_host_bytes()` is MemAvailable/cgroup-limited and NEVER counts swap, and
    # the same floor gates parking as well as sizing; that argument, with receipts, is
    # machines/services2/STRATA.md, "The conversation cache cannot be bought with the swap
    # file", and it reads identically on this card.  Slots=3 matches the three-conversation
    # budget; min-tokens 2048 is the guard that stops a one-shot probe evicting a real
    # conversation (its log line is `skip parking (N tokens, below the M minimum)`).
    "--conversation-cache-mib"
    "1536"
    "--conversation-cache-slots"
    "3"
    "--conversation-cache-min-tokens"
    "2048"
    # Plan v0.3 P6 delivers the "PCIe" share (the cache misses handed to the GPU
    # instead of the CPU pool) three ways: `dma` stages them with the copy engine,
    # `direct` lets the grouped kernel read the pinned expert arena through its
    # device alias, and `auto` resolves to a copy kernel inside the graph (mode 2,
    # 16 staging blobs). aibox has no link - the arena and the cache are the same
    # DDR5 - so `direct` is the mode that matches the hardware: it deletes the copy.
    # Measured (strata-tune/run-arms-uma.sh, 2026-10-08): the copy stage (`waitB`)
    # 6.95 -> 0.14 ms/window, the grouped read through the alias +2 ms/window, net
    # 229.7 -> 227.0 ms/window (9.3 -> 9.4 tok/s). Small, and the reason is not the
    # speed: it takes the in-graph copy path out of decode, which is the #884 class
    # trigger. `--pcie-mode dma` measured the same speed (230.4 ms/window) but does
    # its copies with SDMA, i.e. it keeps that trigger, so do not swap them.
    # `--pcie-frac` is deliberately NOT set: the probe's 0.55 is not a link
    # measurement here, yet sweeping it says it is the right answer anyway
    # (0.25 -> 8.1 tok/s, 0 -> 7.5, 1.0 -> 9.3, default -> 9.4).
    "--pcie-mode"
    "direct"
  ];

  # The key clients must present on /v1/* (`Authorization: Bearer <key>`, or
  # `x-api-key`; serve/server.py _authorized).  Upstream's startup warning for not
  # having one is "WARNING: no API key - anyone on your network can use this model",
  # and this box used to earn it: the engine binds 10.3.1.32 and this box's firewall
  # is off, so "LAN-only" was a habit rather than a rule.  Setting a key also switches
  # off the Host check and the browser-Origin guard, both of which exist to protect a
  # keyless server from strangers - so allowed_hosts below stops being the boundary
  # and stays documentation.
  #
  # It is the SAME secret services2's engine uses, because `gw` injects one key:
  # `strataBearer = true` in a front-door row renders
  # `proxy_set_header Authorization "Bearer …strata_api_key…"`, so a second key would
  # be a 401 on the door's path.  Read at build time through config.sopsSecretText
  # (common/sops.nix), which means - like nginx_sso_auth and status_dashboard_lan_token
  # and services2's own copy - it is inside a world-readable store path on every box
  # that builds or substitutes this system; that is what `infra.buildTimeSecrets` gates
  # and why each entry has to say why it cannot be a runtime secret.  Here the answer
  # is "it is interpolated into the JSON run config the unit execs".
  apiKey = builtins.readFile (config.sopsSecretText "strata_api_key");

  # The run config serve/server.py reads: the engine binary, its args, the
  # tokenizer.
  configFile = pkgs.writeText "strata-iq2xs.json" (
    builtins.toJSON {
      exe = "${strataPkg}/bin/strata";
      args = engineArgs;
      cwd = "/var/lib/strata";
      tokenizer = "${packDir}/tokenizer";
      model_name = "qwen3.8-flash-next-iq2xs";
      log = "/var/lib/strata/strata.log";
      # The names this server answers to.  nginx forwards the incoming Host header
      # upstream (NixOS' default `proxy_set_header Host $host`), and host_allowed() in
      # serve/server.py 403s anything unlisted - though with an api_key set that check
      # is skipped, so keep this list honest rather than load-bearing.  The front-door
      # row for strata2.int.leighhack.org is what has to move with it
      # (common/front-door.nix + common/int-dns.nix; the guard in
      # machines/gw/int-dns.nix refuses a door that serves a name DNS does not answer).
      allowed_hosts = [
        "10.3.1.32"
        "strata2.int.leighhack.org"
      ];
      api_key = apiKey;
      backend = "hip";
      # ROCm 10 instead of nixpkgs' ROCm 7.2, or not.  serve/server.py's child_env()
      # PREPENDS these to the engine's LD_LIBRARY_PATH, which is how a binary built
      # against 7.2.4 ends up running on 10.0.0 - and why the directories must exist,
      # which is why they are store paths (./rocm10-runtime.nix).
      lib_dirs = lib.optionals cfg.rocm10Runtime ["${rocm10}/lib"];
      env = {
        # gfx1035 -> gfx1030, so the engine's device check (src/core/device.cu,
        # which compares gcnArchName with the compiled STRATA_HIP_ARCHS) accepts
        # the 660M and rocBLAS/Tensile kernels are selected for gfx1030.
        HSA_OVERRIDE_GFX_VERSION = "10.3.0";
        HIP_VISIBLE_DEVICES = "0";

        # Reliability workarounds for the open gfx1030 `verify: timed out at
        # layer N (#267)` stall. Upstream #884 traces it to a ROCm barrier
        # packet that is never passed when the MMQ prompt path, the adaptive
        # expert swaps and SDMA copies are used together - exactly our stock
        # config. This does the adaptive swaps' H2D copies with a kernel
        # instead of SDMA, so it keeps the MMQ prompt speed (unlike
        # HSA_ENABLE_SDMA=0, which costs 10-24% prompt) and 3-11% decode
        # (unlike disabling swaps with --adapt-every). See STRATA.md.
        STRATA_HIP_ADAPT_KERNEL_COPY = "1";
        # Second, independent mechanism (upstream #750/#920): on this iGPU
        # every GPU allocation is a KFD userptr over the same DDR5 the expert
        # arena uses, and when the kernel reclaims a pinned page the GPU's
        # queues are suspended until the page is restored - which shows up as
        # the same verify timeout. Keeping ROCr off USERPTR for paged host
        # allocations avoids it (measured as the same median upstream).
        HSA_USERPTR_FOR_PAGED_MEM = "0";

        # The watchdog's limit, raised from the 60 s default to 180 s (the same net the
        # off-network R9700 box has run since 2026-11-20).  Not a tolerance of hangs: on
        # this card the *legitimate* pause is the length of the default limit.  The engine
        # counts them per request — "the graphics driver paused the GPU for N ms during
        # this request while Linux moved memory the GPU uses" — and this box's own log
        # holds requests paused for 24 519, 24 636, 51 678, 63 973 and 133 139 ms, against
        # a 60 s limit, with 354 270 ms of such pauses in one engine run.  A request that
        # was going to finish was therefore being failed by the watchdog, and the
        # `no progress` abort of 2026-11-23 13:01:31 (./STRATA.md) is one of those: the
        # GPU had finished what it held (the #267 release path finished it in 1078 ms) and
        # the host had been waiting on a flag across a reclaim.
        # Cost of 180: a genuinely stuck engine holds the single serving slot 3 min
        # instead of 1 before the server restarts it.
        STRATA_WATCHDOG_S = "180";
        # Armed deliberately, and this is the reason it was previously "only ever set
        # while capturing": the #267-class stall came back on 2026-11-23 after a clean
        # run from 2026-10-07, and ./STRATA.md's own instruction on recurrence was to
        # capture this trace block.  It is the only artifact that separates the two
        # candidate causes (#649 memory-starved CPU pool vs a lost GPU wait), and without
        # it the next reader has one journal line and a 39 GiB process to reason from.
        # Costs nothing unless a verify window times out.  Drop it when the class is
        # closed; STRATA_KERNEL_AUDIT is NOT set, because that one adds ~10 nodes per
        # layer to the captured graph and is a measurement cost, not a passive trace.
        STRATA_VERIFY_TRACE = "1";

        # THE PROMPT PATH'S NUMBERS CHANGE WITH THIS ONE - and on this box it pays 2.21x.
        # The engine printed the tip on every start of this card (`strata prefill: HIP
        # device 0 gfx1030 - tip: STRATA_HIP_PROMPT_F16=1 reads long prompts about 2x
        # faster here (FP16 prompt GEMMs, rocBLAS is tuned only for FP16 on gfx103x; the
        # numbers differ slightly, see docs/AMD_HIP.md)`); it is opt-in at
        # src/prefill/gemm.cu:588 (#835) and, when on, replaces the tip with its own
        # line ending "not bit-identical" (gemm.hpp:137 is the 0/1 override).
        # Measured here 2026-10-28 against the same 13,069-token prompt: 349.7 s (37.4
        # tok/s) before, 158.4 s (82.5 tok/s) after, and a second fresh prompt in the
        # after arm read at 81.7 tok/s - so the claim is real on this hardware, and
        # prefill is the wall this box actually has (a 100K-token prompt: ~45 min at the
        # old rate, ~20 min at this one).  Decode is untouched (10.4-12.4 tok/s both
        # arms), as expected for a prompt-path switch.
        # What it costs: "not bit-identical" is upstream's wording, and this box cannot
        # bound it - greedy output at temperature 0 is already not reproducible here run
        # to run (docs/follow-ups.md, "An md5 of the completion is a weak A/B instrument"),
        # so no digest comparison would have meant anything either way.  Revert = delete
        # this line; it is env, so the revert is one restart and no rebuild.
        STRATA_HIP_PROMPT_F16 = "1";
      };
      # Not set, and deliberately so - both are levers with a measurement attached:
      #   * STRATA_ROCBLAS_TUNING (new with 0.1.4x, gfx103x only, and for us):
      #     upstream's own header says rocBLAS's pick for some prompt GEMM shapes on
      #     gfx1030 is up to 6x slower than the best kernel the library holds for the
      #     same shape, and ships tools/hip/gfx1030-rocblas-5.6.0.8d1ae90e.txt plus
      #     tools/hip/tune_rocblas.cpp.  It is keyed to the exact rocBLAS build, and
      #     nixpkgs' reports 5.2.70203, so the shipped table is REFUSED here and
      #     rocBLAS's own kernels run.  Pointing it at $strataPkg/share/strata/tools/hip
      #     is harmless (a mismatch is refused with one log line) and useless until a
      #     table is measured for this rocBLAS with tune_rocblas.cpp - which is the
      #     cheapest real prefill lever this box has, against a 45 tok/s ceiling.
      #   * STRATA_KERNEL_AUDIT: instrumentation for #879.  The audit adds a node per
      #     layer to the captured verify graph
      #     (~10/layer, measured cost on the R9700 box), so it is only ever set while
      #     capturing.  STRATA_DEFAULTS=... is likewise a debugging escape hatch.
    }
  );
in {
  options = {
    services.strata = {
      enable = lib.mkEnableOption "Strata (Qwen3.8-Flash-Next) on aibox's Radeon iGPU";

      displaceLlamaServer = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = ''
          Clear llama-server's wantedBy so the two do not fight for RAM at boot.
          Set false to keep llama-server starting (then strata must be started by
          hand after stopping it).  Note what that costs on the network: the
          `ai.int`/`ai.leighhack.org` vhosts proxy to llama-server's :8081.
        '';
      };

      rocm10Runtime = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = ''
          Load the ROCm 10.0.0 (TheRock) userspace runtime from ./rocm10-runtime.nix
          instead of nixpkgs' ROCm 7.2, by putting it in the run config's lib_dirs.
          The reason is upstream #879 (a non-finite / poisoned verify window), which
          the network's other Strata box reproduces on ROCm 7.2.x and not on 10.0.0
          with the same binary and source.  Set false to go back to the nixpkgs ROCm
          with no rebuild of the engine - one flag, one restart, and
          ./STRATA.md records what each runtime did on this card.
        '';
      };
    };
  };

  config = lib.mkIf cfg.enable {
    # The house "never give up" policy (common/systemd.nix): Restart=always plus
    # startLimitIntervalSec=0, so a startup that fails while a leftover engine
    # still holds the GTT keeps retrying instead of parking in `failed` (the
    # "killed engine keeps its GTT" crash-loop). RestartSec below paces each
    # attempt; the helper leaves it alone.
    systemd.services.strata = INFRA.mkNeverGiveUp {
      description = "Strata (Qwen3.8-Flash-Next across the Radeon iGPU, RAM and NVMe)";
      after = ["network-online.target"];
      wants = ["network-online.target"];
      wantedBy = ["multi-user.target"];

      # Don't crash-loop before the hand-run model prep has produced a pack.
      unitConfig.ConditionPathExists = "${packDir}/index.txt";

      serviceConfig = {
        ExecStart = "${strataPkg}/bin/strata-server --engine strata --config ${configFile} --host 10.3.1.32 --port 8080";
        # A store path is read-only only against **non-root**: 0444 files in 0555
        # directories, and root walks straight past that on ext4.  This unit has no `User=`,
        # so the first time the engine imports its Python control layer (`strata_control`),
        # CPython writes `__pycache__/*.pyc` **into the package's own store path** — and a
        # NAR records the file list and contents but *not mtimes*, so adding files breaks
        # the hash nix recorded.  Measured here 2026-11-16: 5 of 5 strata paths held
        # 14-56 post-registration .pyc files; `nix path-info` and the running engine looked
        # perfect, and the symptom appeared on services1 as `hash mismatch importing path`
        # 898 derivations into a `just check-build`.  Repaired by deleting them (caches are
        # regenerable; the engine never restarted) — `docs/nix-dev-workflow.md`, Gotchas,
        # owns the mechanism, and `common/store-hygiene.nix` is the weekly net that fails a
        # unit when it happens again.
        #
        # Not in the package wrapper (`strata-package.nix` already exports PYTHONPATH
        # there), because that would rebuild the 39 MB package for a runtime habit and
        # would still not cover a `nix-shell`-style root run.  Not a non-root `User=`
        # either, which is the structurally better answer: the engine needs the MEMLOCK
        # limit and the kfd/GTT access it has today, and that is a different experiment
        # than a one-token guard.
        #
        # The same line is in `machines/services2/strata.nix` on purpose: that box is
        # measured clean but has the identical shape (root, same package, no PYTHON* env),
        # and the only thing separating it is that nothing has imported `strata_control`
        # yet.  COST OF APPLYING THIS LINE: systemd restarts the unit, and an engine reload
        # here is tens of GB — it belongs in a window, not on top of an unrelated switch.
        Environment = ["PYTHONDONTWRITEBYTECODE=1"];
        # The engine log (config "log") is written here; StateDirectory creates it.
        StateDirectory = "strata";
        RestartSec = 10;
        # Loading the model is a multi-minute, tens-of-GB operation.
        TimeoutStartSec = "infinity";
        TimeoutStopSec = 120;
        # The resident expert arena is page-locked where it can be.
        LimitMEMLOCK = "infinity";
        # --- the two lines that make "this box's RAM is strata's" true ------------------
        # Measured, both failure modes, both on this box, both because nothing bounded
        # either side of it until 2026-11-23:
        #   * 2026-11-21 17:54-17:57 — the kernel OOM killer ended `strata` and
        #     `strata-host` twice (33-34 GiB anon each) while `mksquashfs` from
        #     netboot-recut.service was running, dying in kfd_ioctl_alloc_memory_of_gpu.
        #     The OOM killer scores by RSS, and the endpoint is the biggest thing here, so
        #     an unbounded build kills the service a person is talking to.
        #   * 2026-11-23 13:01:31 — the engine's own #29 watchdog ended it mid-decode with
        #     844 MiB RAM available, 2.2 GiB in swap and 34 189 major page faults in the
        #     process, after 9 599 ms of "the graphics driver paused the GPU … while Linux
        #     moved memory the GPU uses" in that request.  That pause *is* reclaim, on a
        #     card whose whole "VRAM" is GTT pages of the same DDR5 as the arena, and
        #     upstream's #649 is the same mechanism named from the other side: a
        #     memory-starved box misses the verify handshake.
        #     /proc/vmstat since boot said the rest: 53.9 M pages scanned in *direct*
        #     reclaim (~206 GiB), 46.6 M/61.1 M swap-ins/outs, 9.0 M major faults.
        # So: protect the arena from reclaim, and make the build the thing that dies.
        # `MemoryMin` is cgroup memory protection — the kernel reclaims from every other
        # group before it touches this one — and 40 GiB is the arena plus its caches
        # (measured unit peak 39.3 GiB via systemd, 46.1 GiB on the worst run) sized to
        # leave the 59 GiB box ~19 GiB for ./configuration.nix's capped build cgroup,
        # whisper, immich and the page cache.  It is protection, not a cap: the unit stays
        # unbounded above, because a MemoryMax here would OOM the engine itself.
        # `OOMScoreAdjust` then makes the ranking explicit: if the box does run out, a
        # compiler dies and the endpoint lives.  The negative adjustment is not a
        # privilege claim — leigh-admin has passwordless sudo here anyway — it is the
        # kernel being told which of two failures is the cheap one.
        MemoryMin = "40G";
        OOMScoreAdjust = -700;
        # No core dumps, and this is not hygiene: the engine's hang watchdog
        # (#29/#267) SIGABRTs itself, and on the R9700 box each dump was 32 GB of
        # weights and KV - here it would be more, on the filesystem that holds the
        # models - for a binary with no symbols to put in it.  The journal names the
        # stall (layer + token) and that is all a dump ever told you.
        LimitCORE = "0";
      };
    };

    # The two cannot be resident at once (see the header).
    systemd.services.llama-server.wantedBy = lib.mkForce (
      if cfg.displaceLlamaServer
      then []
      else ["multi-user.target"]
    );

    # No `services.nginx` here, on purpose, and it is a rule rather than an accident:
    # `gw` terminates TLS for every name the network has one of and serves every row
    # of `common/front-door.nix`, and `common/int-dns.nix` + `machines/gw/int-dns.nix`
    # are what keep the two in step.  This box used to expose the engine as
    # `aibox.int.leighhack.org/llm/`; since the front-door move aibox has no nginx,
    # and `strata2.int.leighhack.org` is a row there pointing at 10.3.1.32:8080.
  };
}
