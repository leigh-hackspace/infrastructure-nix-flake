{
  config,
  lib,
  pkgs,
  ...
}:

# Strata on aibox: Qwen3.8-Flash-Next (125B total / 6B active MoE) on the
# Radeon 660M iGPU + system RAM + NVMe, replacing the dense 27B that decodes at
# ~2 tok/s here.
#
# Why this can work on a box with no VRAM:
#   Strata is not an offloader. Its tiers are: experts resident in RAM (the
#   arena), a GPU expert cache for the hot ones, a CPU pool for the cold ones,
#   and the 28.8 GB n-gram (PLE) table read unbuffered from the SSD. On an iGPU
#   every "VRAM" allocation is GTT, i.e. the SAME RAM as the arena - so the GPU
#   tier adds compute, not bandwidth. Decode is bandwidth-bound: ~2.7 GB of
#   weights read per token at ~50 GB/s. A dense 27B reads ~21 GB per token,
#   which is exactly why llama.cpp sits at 2 tok/s here; this model reads ~8x
#   less per token.
#
# Card/arch: rocminfo says gfx1035, which Strata's cmake refuses (it accepts
# gfx1100/gfx1101/gfx1200/gfx1201 validated, gfx1101/gfx1200 community,
# gfx1012/gfx1102/gfx1030/gfx1031 unvalidated). gfx1035 is RDNA2 with the same
# wave32, 64 KiB LDS and v_dot4_i32_i8 ISA as gfx1030, so the engine is built
# for gfx1030 and HSA_OVERRIDE_GFX_VERSION=10.3.0 makes the runtime report
# gfx1030 (the same override this box's ROCm torch setups use for the 660M).
#
# ⚠️ llama-server and strata CANNOT be resident at once: llama-server holds
# ~33 GB of GTT for its model, and the IQ2_XS arena alone is 35.5 GiB. This
# module therefore clears llama-server's wantedBy (see displaceLlamaServer) so
# the box boots into Strata instead; `systemctl start llama-server` still works
# for the small models when strata is stopped.
#
# Model data is NOT in the store. It is prepared ONCE by hand before this unit
# will start (the unit has ConditionPathExists on the pack index):
#
#   STRATA=$(nix build --impure --expr \
#     'let f = builtins.getFlake (toString /home/leigh-admin/Projects/infrastructure-nix-flake);
#        in f.packages.x86_64-linux.strata' --no-link --print-out-paths)
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
# Check the card first (before downloading anything):
#   $STRATA/bin/strata-device --list-devices   # with HSA_OVERRIDE_GFX_VERSION=10.3.0
#   $STRATA/bin/strata-device --selftest
#
# Access: http://10.3.1.32:8080/v1 (OpenAI chat/completions, /v1/messages for
# Anthropic-style clients, /v1/models, /health) and, through the existing
# aibox.int.leighhack.org vhost, http://aibox.int.leighhack.org/llm/v1.
# Logs: journalctl -u strata -f and /var/lib/strata/strata.log.
#
# MEASURED ON THIS BOX (2026-10-05, IQ2_XS, ctx 131072, llama-server stopped).
# Full numbers, the decode-time breakdown and the open suggestions are in
# ./STRATA.md - read it before changing expertCache or context.
#   decode 8.2-11.2 tok/s (llama-server's dense 27B: ~2 tok/s here)
#   prefill ~45 tok/s at ANY chunk size: a 20k-token prompt costs ~7 min, which
#         is the real limit for agent use, and it does not improve with more GTT
#   the expert cache is a GPU-COMPUTE tier, not a memory tier: GTT is the same
#         DRAM the CPU pool already reads, so 2048 -> 8192 slots (hit 63% -> 91%)
#         bought only +11-17% decode and ate the RAM headroom. expertCache below
#         is the middle point that keeps ~11 GiB free.

let
  cfg = config.services.strata;

  strata = pkgs.callPackage ./strata-package.nix { };

  # Model layout. The GGUFs stay under ~/Models with everything else there;
  # only the hand-prepared pack and MTP runtime live under modelDir.
  ggufDir = "/home/leigh-admin/Models/Qwen3.8-Flash-Next-GSQ-RCO-GGUF";
  modelDir = "/home/leigh-admin/Strata";
  packDir = "${modelDir}/pack/iq2xs";
  nativeGguf = "${ggufDir}/IQ2_XS/Qwen3.8-Flash-Next-GSQ-RCO-IQ2_XS-00001-of-00002.gguf";
  pleGguf = "${ggufDir}/IQ2_XS/Qwen3.8-Flash-Next-GSQ-RCO-IQ2_XS-00002-of-00002.gguf";
  mtpDir = "${modelDir}/mtp/rt";
  expertProfile = "${strata}/share/strata/data/expert-profile.bin";

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
  # frigate/immich/whisper). 4096 lands at ~50 GiB used, ~10 GiB headroom.
  # Raise --max-context only with the RAM for it: 262144 costs ~3.6 GiB of KV,
  # 524288 ~7.2 GiB + YaRN 2.
  context = 262144;
  expertCache = 4096;

  # Explicit chunk, not "auto": auto picks the largest chunk the expert cache can
  # LEND (4096 at 2048 slots). Measured prefill was 44.6 tok/s at 4096 and 46.0 at
  # 8192 - the chunk is not what limits prefill here, but asking for 8192 costs
  # nothing and is strictly better.
  prefillChunk = "8192";

  engineArgs = [
    "--pack"
    packDir
    "--native"
    nativeGguf
    "--ple-gguf"
    pleGguf
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
  ];

  # The run config serve/server.py reads: the engine binary, its args, the
  # tokenizer. The Host check passes IP addresses on trust, so 10.3.1.32 needs
  # no entry; the DNS name reached through nginx does.
  configFile = pkgs.writeText "strata-iq2xs.json" (
    builtins.toJSON {
      exe = "${strata}/bin/strata";
      args = engineArgs;
      cwd = "/var/lib/strata";
      tokenizer = "${packDir}/tokenizer";
      model_name = "qwen3.8-flash-next-iq2xs";
      log = "/var/lib/strata/strata.log";
      allowed_hosts = [
        "aibox.int.leighhack.org"
        "10.3.1.32"
      ];
      backend = "hip";
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
      };
    }
  );

  strataLocation = {
    # The engine binds 10.3.1.32 (its --host), NOT loopback: proxy_pass to
    # 127.0.0.1:8080 gives a silent 502 from nginx.
    proxyPass = "http://10.3.1.32:8080";
    proxyWebsockets = true;
    extraConfig = ''
      # First token can take minutes (the engine loads ~40 GB and fills its
      # tiers); SSE must stream un-buffered. The /llm/ prefix is stripped so
      # clients use .../llm/v1/chat/completions.
      rewrite ^/llm/?(.*)$ /$1 break;
      proxy_read_timeout 3600s;
      proxy_send_timeout 3600s;
      proxy_buffering off;
    '';
  };
in
{
  options = {
    services.strata = {
      enable = lib.mkEnableOption "Strata (Qwen3.8-Flash-Next) on aibox's Radeon iGPU";

      displaceLlamaServer = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = ''
          Clear llama-server's wantedBy so the two do not fight for RAM at boot.
          Set false to keep llama-server starting (then strata must be started by
          hand after stopping it).
        '';
      };
    };
  };

  config = lib.mkIf cfg.enable {
    systemd.services.strata = {
      description = "Strata (Qwen3.8-Flash-Next across the Radeon iGPU, RAM and NVMe)";
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      wantedBy = [ "multi-user.target" ];

      # Never rate-limit restarts: a startup that fails because a leftover
      # engine still holds the GTT must keep retrying until it can come up,
      # rather than parking in `failed` (the "killed engine keeps its GTT"
      # crash-loop). RestartSec below paces each attempt.
      startLimitIntervalSec = 0;

      # Don't crash-loop before the hand-run model prep has produced a pack.
      unitConfig.ConditionPathExists = "${packDir}/index.txt";

      serviceConfig = {
        ExecStart = "${strata}/bin/strata-server --engine strata --config ${configFile} --host 10.3.1.32 --port 8080";
        # The engine log (config "log") is written here; StateDirectory creates it.
        StateDirectory = "strata";
        Restart = "on-failure";
        RestartSec = 10;
        # Loading the model is a multi-minute, tens-of-GB operation.
        TimeoutStartSec = "infinity";
        TimeoutStopSec = 120;
        # The resident expert arena is page-locked where it can be.
        LimitMEMLOCK = "infinity";
      };
    };

    # The two cannot be resident at once (see the header).
    systemd.services.llama-server.wantedBy = lib.mkForce (
      if cfg.displaceLlamaServer then [ ] else [ "multi-user.target" ]
    );

    services.nginx.virtualHosts."aibox.int.leighhack.org".locations."/llm/" = strataLocation;
  };
}
