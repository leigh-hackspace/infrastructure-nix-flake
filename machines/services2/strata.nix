{
  config,
  lib,
  pkgs,
  INFRA,
  # The flake's `strata` input: upstream main (bleeding edge). Its `outPath` is
  # the source the CUDA engine is built from.
  strata,
  ...
}:
# Strata on services2: Qwen3.8-Flash-Next Coder (IQ1_M, 256 of 512 experts) on
# the RTX 3060 (CUDA/sm_86) + system RAM + NVMe.
#
# This is the second Strata instance in the network; aibox runs the full 125B
# model on its Radeon iGPU (machines/aibox/strata.nix). This one exists because
# the box has a real 12 GB card and the Coder pack is the smaller, faster,
# coding-tuned variant: its expert arena is ~23.4 GiB instead of the full
# model's ~35.5 GiB, which is what lets it run beside the observability stack on
# a 30 GB box.  Read machines/services2/STRATA.md before changing engine flags.
#
# Model data is NOT in the store. It is prepared ONCE by hand (the unit has
# ConditionPathExists on the pack index, so it will not crash-loop before that):
#
#   # from the checkout, and NEVER through `builtins.getFlake "/abs/path"` — a path
#   # ref copies the whole working tree (build trees and .git included, 6.7 GB+) into
#   # the store on every call; see docs/nix-dev-workflow.md.
#   STRATA=$(cd ~/Projects/infrastructure-nix-flake && nix build .#strata-cuda --no-link --print-out-paths)
#   mkdir -p /home/leigh-admin/Strata/pack/coder-iq1m
#   cd /home/leigh-admin/Models/Qwen3.8-Flash-Next-GSQ-RCO-Coder   # already downloaded
#   # 1. the pack (index + dense tensors + tokenizer, ~1.5 GB):
#   $STRATA/bin/strata-prep iq_pack.py \
#     --gguf /home/leigh-admin/Models/Qwen3.8-Flash-Next-GSQ-RCO-Coder/Qwen3.8-Flash-Next-GSQ-RCO-IQ1_M-00001-of-00002.gguf \
#     --out /home/leigh-admin/Strata/pack/coder-iq1m
#   # 2. the MTP draft runtime (~6 GB; the GSQ-RCO GGUF ships NO MTP head, so the
#   #    31 mtp.* tensors come from the BF16 checkpoint):
#   $STRATA/bin/strata-prep mtp_fetch.py fetch --out /home/leigh-admin/Strata/mtp
#   $STRATA/bin/strata-prep mtp_fetch.py verify --out /home/leigh-admin/Strata/mtp
#   $STRATA/bin/strata-prep mtp_pack.py --src /home/leigh-admin/Strata/mtp \
#     --experts q2_0 --out /home/leigh-admin/Strata/mtp/mtp-q2_0.gguf
#   $STRATA/bin/strata-prep mtp_rt.py --gguf /home/leigh-admin/Strata/mtp/mtp-q2_0.gguf \
#     --out /home/leigh-admin/Strata/mtp/rt
#   cp $STRATA/share/strata/data/draft_vocab.bin /home/leigh-admin/Strata/mtp/rt/
#
# Check the card first, before preparing anything:
#   $STRATA/bin/strata-device --list-devices   # must report sm_86 / RTX 3060
#   $STRATA/bin/strata-device --selftest
#
# Access: http://10.3.1.21:8080/v1 (OpenAI chat/completions, /v1/messages for
# Anthropic-style clients, /v1/models, /health).  services2's firewall is off,
# so this is reachable on the LAN directly; the front door (`gw`) terminates TLS
# for it as https://strata.int.leighhack.org (a row in common/front-door.nix).
# It also answers publicly at https://strata.leighhack.org: `gw`'s nginx
# terminates that name on the network's public addresses and proxies over the LAN
# to 10.3.1.21:8080, because services2 itself has no public IP.  That is only safe
# because the engine now has an api_key (apiKey below): /health says "api_key":
# true and every /v1/* needs it (401 "missing or wrong API key" without one).
#
# Only the LAN names are in allowed_hosts below.  Upstream runs that Host check
# only when it has no key (serve/server.py parse_request: `if svc.api_key or
# host_allowed ...`), so the key is what lets the public name through, and the
# public name staying out of the list is the fail-safe: take the key out and the
# public vhost starts answering 403 instead of serving an unauthenticated engine
# to the whole world.  See the ⚠️ note in common/front-door.nix for why a LAN
# vhost and this list still have to move together.
# Logs: journalctl -u strata -f and /var/lib/strata/strata.log.
let
  cfg = config.services.strata;

  # The CUDA engine, from the bleeding-edge flake input source. Passing the CUDA
  # backendStdenv is what lets nvcc use a gcc it supports (CUDA 13.0 -> gcc 15).
  strataPkg = pkgs.callPackage ./strata-package.nix {
    src = strata.outPath;
    stdenv = pkgs.cudaPackages_13_0.backendStdenv;
    cudaPackages = pkgs.cudaPackages_13_0;
  };

  # Model layout. The GGUFs stay under ~/Models with everything else there; only
  # the hand-prepared pack and MTP runtime live under modelDir.
  modelDir = "/home/leigh-admin/Strata";
  ggufDir = "/home/leigh-admin/Models/Qwen3.8-Flash-Next-GSQ-RCO-Coder";
  packDir = "${modelDir}/pack/coder-iq1m";
  nativeGguf = "${ggufDir}/Qwen3.8-Flash-Next-GSQ-RCO-IQ1_M-00001-of-00002.gguf";
  pleGguf = "${ggufDir}/Qwen3.8-Flash-Next-GSQ-RCO-IQ1_M-00002-of-00002.gguf";
  mtpDir = "${modelDir}/mtp/rt";
  # The Coder pack's own routing profile ships in the source tree.
  expertProfile = "${strataPkg}/share/strata/data/expert-profile-coder.bin";

  # services2's budget (30 GiB usable RAM, 12 GB VRAM, RTX 3060):
  #   experts (IQ1_M Coder arena) 23.4 GiB  the profile's figure; too big beside
  #                               the OS + monitoring on 30 GiB, so this box runs
  #                               the engine's low-RAM mode: the experts the GPU
  #                               does not hold are copied into RAM once
  #                               (--resident-experts, ~16 GiB) instead of the
  #                               full arena.
  #   expert cache                auto      sized from free VRAM. GA106 is a
  #                               discrete 12 GB card, so `auto` reads dedicated
  #                               VRAM (unlike aibox's iGPU, where it reads GTT
  #                               and fills the machine - never copy that trick).
  #   KV                          int8,     13.4 KiB/token (1056 B per token per attention
  #                               streamed  layer x 13), of which only 32768 cells per QSA
  #                                         layer are in VRAM: 0.9 GiB there, 3.09 GiB in
  #                                         pinned host RAM.  setup's own rule would not
  #                                         stream this pack (it wants 34.0-36.6 GB of RAM
  #                                         at these contexts, the box has 30.7) - `--kv-
  #                                         resident` spelled in the args is the override,
  #                                         and the RAM it needs comes out of the resident
  #                                         expert budget, which is the whole trade.
  #
  # The context and the KV format are one decision, not two, because the card ends up
  # with the same ~630 MiB free whatever is asked for: **every token of context is paid
  # for out of the expert cache and the prompt chunk.**  Measured 2026-11-18 in one
  # engine build (0.1.61), one pack, one bench, six arms - the receipts, and why the
  # naive "just raise --max-context" is the most expensive thing you can do here, are in
  # machines/services2/STRATA.md, "Context and the KV cache: what each option costs":
  #
  #   ctx      kv     cache  chunk   8k      28k     ~128k prompt   retrieval
  #                                prompt  prompt   (needle tok/s)   at depth
  #   131072   int8    1781  7680   1064    1193         1118          2/2
  #   196608   q4_0    1878  8192   1111    1097         1038          2/2
  #   196608   k8v4    1558  6400    966    1020          ---          not run
  #   262144   q4_0    1580  6400    990    1055          995          2/2
  #   262144   k8v4    1152  4352    954     933          892          2/2
  #   262144   int8     785  2048    806     785          744          2/2
  #   ... and then the same 262144/int8 with the KV streamed into host RAM:
  #   262144   int8*   2332  8192   562-725  1028        1045          2/2   (headroom 4)
  #   262144   int8*   2332  8192   1208     1200        1081          2/2   **deployed** (headroom 2)
  #   (* = `--kv-resident 32768`: the KV's 3.09 GiB move to pinned host RAM, so the
  #   cache and the chunk come back.  The two rows differ only in how much RAM the
  #   resident expert copy was allowed to keep - see `env.STRATA_RESIDENT_HEADROOM_GIB`
  #   below, and read the `lendable slots` line before believing any number here.)
  #
  # So 262144 (the model's trained length: upstream's `--yarn-orig-ctx` default is
  # 262144, so nothing below it needs rope scaling) is reachable **at int8 precision and
  # at or above this box's old speed**, but only by paying in RAM rather than VRAM: the
  # deployed arm reads an 8k prompt 13 % faster and a 28k one the same as 131072/int8 did,
  # with a bigger expert cache, and it costs the box's last GiB of headroom.  q4_0 remains
  # the arm that costs neither: -7 % on an 8k read and -12 % on a 28k read against 131072,
  # with ~3.9 GiB of RAM still free.  What q4_0 gives up is precision on long documents
  # (upstream: perplexity +7 % at 1K and +13 % at 8K, needles passing; int8 as
  # indistinguishable from fp16) - which is exactly the reason the streamed int8 arm is
  # worth the RAM.  What neither arm can see is that difference in our own instruments: the
  # answer key is unchanged in every arm and the needle is 2/2 in all of them.
  context = 262144;

  engineArgs = [
    # What the engine turns on by itself for THIS card at the current input pin (engine
    # 0.1.61, bumped 2026-11-18 - machines/services2/STRATA.md, "Update: engine 0.1.61 —
    # 2026-11-18").  Read the start line, not this comment: `journalctl -u strata | grep
    # "exact speed switches"`.  It is the one thing this bump moved on this card: **the
    # sm_86 table went from 9 default switches to 8** - STRATA_ADAPT_NOWAIT is no longer
    # set (src/core/nv_defaults.cpp:33, "repeatable output": with it a verify window ran
    # a swapped-in expert on the GPU or the CPU by its copy's timing, so two starts of the
    # same request differed; upstream's measured price of removing it is decode x0.988).
    # ADAPT_LAG=2 stays, so the copies still get two windows to land, and
    # STRATA_ADAPT_NOWAIT=1 puts the old behaviour back.
    #
    # Measured here, removing it did NOT buy repeatability: `strata-answer-check.py 96 3`
    # on the live unit still answers `stable 0/3`, so the greedy-fingerprint gate stays
    # unusable on this box (the expert tier's placement is timing-dependent, and the GPU
    # and CPU round an expert differently - the engine says so itself at start).  Only
    # `strata-answer-score.py`, which scores against an answer key, may carry a
    # before/after claim; the receipts are the 2026-11-14 retraction in
    # machines/services2/STRATA-TUNE-2026-11-14.md.
    #
    # STRATA_PROMPT_ATTN_IMMA is still ON for every CUDA card of compute capability >= 80
    # (+1.1 to +1.8 % prompt, bitwise identical output), so nothing moves there.
    #
    # STRATA_ROUTE_PRIOR is still OFF.  It is the residency-biased expert routing
    # that 0.1.52 defaulted to 1.5 on a discrete card of <=13 GiB, i.e. on this 3060; from
    # 0.1.56 it defaults to 0 on every card ("repetition loops and weaker results in long
    # agent sessions", their agent check 4.5 of 10 with it on against 5.6 of 10 off).  That
    # is the shape wanted here, since this box is the one pi answers from.  To pay it
    # back, put STRATA_ROUTE_PRIOR = "1.5" in extraEnv - no rebuild, and answers move.
    #
    # `--draft` still defaults to `mtp` (generate.cpp:1157 names dflash2 as the opt-in),
    # so the `--mtp` line below still means the MTP drafter, not the new DFlash2 one.
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
    "--spec"
    "4"
    "--spec-min-p"
    "0.5"
    "--mtp"
    mtpDir
    "--max-context"
    (toString context)
    # One of fp16 | int8 | q4_0 | k8v4 (generate.cpp:4719 rejects anything else), and
    # this one is **streamed**.  This box ran `--kv int8` while its context was 131072,
    # where the KV cost 1.9 GiB of cache; at 262144 the same bytes (3.4 GiB) would
    # halve the cache to 785 slots and drop the prompt chunk to 2048 - so 256K at full
    # precision is only affordable if the KV stops living in VRAM.  Moving it costs
    # nothing in answers: upstream's `kv_stream_parity` ctest checks the streamed path
    # against the resident one **bitwise**, with ring restore, and int8 K/V is itself
    # indistinguishable from fp16 (`bench/results/2026-09-27-kv-q4`).  Measured here
    # 2026-11-18 (`machines/services2/STRATA.md`, "256K at full precision: the KV in
    # RAM, and what it bills"): **8k prompt 1208 and 1202 tok/s** against 1064 at
    # 131072/int8, **28k 1200** against 1193, an **183,987-token read at 1081** with
    # 2/2 records back from 2 % and 97 % depth, decode 42.5, answer key 9/9, and the
    # expert cache *up* to 2332 slots with the chunk back at 8192.  q4_0 (the arm this
    # file deployed for an hour: 990/1055, 974 @184K) is the fallback if the RAM
    # arithmetic below ever stops adding up; it is cheaper in RAM and costs precision on
    # long documents (+7 % perplexity at 1K, +13 % at 8K).
    "--kv"
    "int8"
    # KV streaming: keep N cells of each QSA layer in VRAM, the whole K/V in pinned host
    # RAM (3.09 GiB at this context).  A context of N cells or fewer is not streamed at
    # all, so ordinary turns keep the resident path; long ones pay one DMA per array to
    # the host copy (`STRATA_KV_HOST_DMA`, default on NVIDIA since the 0.1.61 bump)
    # instead of VRAM this card does not have.
    #
    # **WHAT TO READ AFTER ANY CHANGE TO THIS START, before anything else:**
    #   sudo grep "lendable slots" /var/lib/strata/strata.log | tail -1
    # It must read `1533 of the prompt path's 1533 lendable slots keep their experts in
    # RAM too`.  When it reads "N of M … (P %) will read their experts from the pack"
    # instead, the prompt path is fetching experts off the NVMe mid-prompt and the read
    # speed collapses - measured, with 69 % uncovered an 8k prompt read at **562-725**
    # tok/s instead of 1200, which is the entire difference between a good arm and a bad
    # one here.  The dial that fixes it is `env.STRATA_RESIDENT_HEADROOM_GIB` below.
    # `--expert-cache` is not: the cache is already bigger than it was.
    # Pinned 2026-10-23 (engine 0.1.48.1): upstream made this default to -1, which
    # means "--serve sizes a host-RAM conversation cache from the RAM still free
    # once the model is loaded": 40 % of (available - 2560 MiB), in 256 MiB steps,
    # at most 8192 MiB, off below 1024 (include/strata/core/conversation_memory.hpp,
    # auto_conversation_cache_mib, upstream #3493; the call is in generate.cpp and
    # only `--serve` takes it).  This box sits right on that threshold - 28.3 GiB
    # RSS of 30 GiB, `free -m` says 3556 available with nothing loaded - so the
    # rule answers 0 today but would start parking 2 GiB the moment the load is
    # sized differently, and 2 GiB more is OOM here beside Prometheus.  0.1.46's
    # default was 0, so this line is the old behaviour spelled, not a change: turn
    # it on only after re-reading the sizing table above.
    "--kv-resident"
    "32768"
    "--conversation-cache-mib"
    "0"
    # Low-RAM mode (the profile needs ~32 GB of experts' RAM; this box has 30):
    # the experts not in the GPU cache are copied into RAM once. The alternative
    # is --mmap-experts, which reads them through the OS file cache - steadier
    # RAM use is the reason setup.py picks the resident variant when it fits.
    "--resident-experts"
  ];

  # The key clients must present on /v1/* (`Authorization: Bearer <key>`, or
  # `x-api-key`; serve/server.py _authorized).  Upstream's own startup warning for
  # not having one is "WARNING: no API key - anyone on your network can use this
  # model", and setting one also switches off the Host check and the browser-Origin
  # guard, both of which exist to protect a keyless server from strangers.
  #
  # Read at build time from sops (config.sopsSecretText, common/sops.nix) and
  # interpolated into the JSON below, so it lands in the world-readable nix store
  # like every other build-time secret here (nginx_sso_auth,
  # status_dashboard_lan_token) — a client has to hold this key to call the
  # endpoint at all.  Anything that must stay out of the store belongs in
  # CONFIG.ENV_FILE (/run/secrets/env_file, root-only at runtime) instead; to move
  # this one there, drop `api_key` from the config and give the unit an
  # EnvironmentFile: serve/server.py also reads STRATA_API_KEY, and refuses to
  # start if that variable is present but empty.  The value is written into the
  # JSON unescaped, so it must carry no quote or backslash; upstream strips
  # spaces around it and reads commas as several keys.
  apiKey = builtins.readFile (config.sopsSecretText "strata_api_key");

  # The run config serve/server.py reads: the engine binary, its args, the
  # tokenizer. The Host check passes IP addresses on trust, so 10.3.1.21 needs
  # no entry; the DNS name a client might use does.
  configFile = pkgs.writeText "strata-coder-iq1m.json" (
    builtins.toJSON {
      exe = "${strataPkg}/bin/strata";
      args = engineArgs;
      cwd = "/var/lib/strata";
      tokenizer = "${packDir}/tokenizer";
      model_name = "qwen3.8-flash-next-coder-iq1m";
      log = "/var/lib/strata/strata.log";
      # The names this server answers to.  Keep in step with the front-door rows
      # in common/front-door.nix that proxy :8080: nginx forwards the incoming
      # Host header upstream, and host_allowed() in serve/server.py 403s anything
      # not listed here (an IP passes on trust).
      allowed_hosts = [
        "10.3.1.21"
        "services2"
        "services2.int.leighhack.org"
        "strata.int.leighhack.org"
      ];
      # What /v1/* needs; see `apiKey` above for why this also turns the Host
      # check off, and the header above for why the public name is not in the
      # list next to it.
      api_key = apiKey;
      backend = "cuda";
      # serve/server.py prepends these to the engine's LD_LIBRARY_PATH. The CUDA
      # toolkit libs are on the binary's RUNPATH, but libcuda.so.1 comes from the
      # driver, which NixOS exposes here (hardware.graphics + hardware.nvidia).
      lib_dirs = ["/run/opengl-driver/lib"];
      env.CUDA_VISIBLE_DEVICES = "0";
      # The fused int8 prompt-expert kernels (#136).  **This is the one switch in the
      # 2026-11-14 sweep (23 arms) that measured faster.**  Upstream lists it among "options
      # that change answers", and the first thing I did with that was measure a change - see
      # the retraction below, because the measurement was invalid.  Numbers on this pack,
      # 8,089-token prompt: control 1,048 / 1,048 / 1,050 / 1,056 / 1,056 tok/s against
      # fused **1,170.5** and **1,099.4** (+4 to +12 %, two reps); 28k: 1,123-1,146 against
      # **1,160.2** and **1,160.7** (+1 to +3 %).  Decode does not move - the fused kernels are
      # the *prompt* path.  Mechanism, read off the engine's own log: the chunk ceiling goes
      # 6,656 -> 8,192, so an 8k prompt is one chunk instead of 6,656 + a tail.
      #
      # What it costs, measured rather than assumed.  Greedy fingerprints APPEARED to move on
      # 2 of 3 prompts and **that observation is retracted**: three runs of ONE config on this
      # live unit gave three distinct hashes for two of the three probe items, so greedy
      # output here is not reproducible run to run and a one-run-per-config hash comparison
      # attributes run noise to whatever you changed.  `strata-answer-check.py` now measures
      # the reproducibility instead of assuming it; the receipts, and the cross-flag
      # "agreement" the same mistake produced, are in
      # machines/services2/STRATA-TUNE-2026-11-14.md.
      #
      # So no answer-level difference from this flag is measurable by hashing, in either
      # direction.  What is measurable is whether it is still *right*, and it is:
      # `strata-answer-score.py` (nine short greedy items with an answer key), run through
      # the front door **before** this line: 8 scored, **8 correct**, `bit` incomplete; and
      # **after** it: 8 scored, **8 correct**, the same `bit` incomplete.  Same score, same
      # incomplete item, and the speed arrived - live endpoint, 8k **1,105.3 / 1,167.0**
      # against control's 1,048-1,056, 28k **1,156.3 / 1,156.2** against 1,123-1,146, decode
      # where it always was, and the engine's own `prompt chunk auto: 8192 tokens` (was
      # 6656) in `/var/lib/strata/strata.log`.
      #
      # So: +5 to +11 % prompt speed with no measurable answer-level difference either way -
      # which is the honest strength of that claim and also its limit.  This endpoint serves
      # pi's coding; if completions ever read worse, the revert is deleting the line below and
      # `just switch`.
      # Re-verify with:
      #   sudo grep "fused int8" /var/lib/strata/strata.log | tail -1
      #   python3 machines/services2/strata-tune/strata-answer-score.py https://strata.int.leighhack.org
      #   python3 machines/services2/strata-tune/strata-bench.py https://strata.int.leighhack.org
      env.STRATA_PF_FUSED = "1";

      # The streaming K/V's other half: nothing else here may grow into host RAM, so
      # `--conversation-cache-mib 0` above and `--prefill auto` (whose chunk the engine
      # sizes against what is spare) both stay exactly as they are.
      #
      # STRATA_RESIDENT_HEADROOM_GIB: the RAM the resident expert copy leaves free.
      # `--resident-experts` defaults it to 4, and on this box 4 cannot hold both the
      # 3.09 GiB the streamed K/V pins *and* the ~21.9 GiB of experts the prompt path
      # lends slots for: at 4 the complement lands at 19.85 GiB and the `lendable slots`
      # line goes to 69 % uncovered (arm A above: 8k at 562-725 tok/s).  2 is what makes
      # it read N of N, at a 21.88 GiB complement.
      #
      # THE PRICE, and it is the only one: this spends the box's RAM.  With 21.88 GiB of
      # experts page-locked and 3.09 GiB of K/V pinned, `free -m` reads ~2100 MiB
      # available at idle and ~1000 MiB during an 184k-token read, with ~2.2 GiB in the
      # 16 GiB swap file - i.e. below the engine's own 2560 MiB guard floor, on a box
      # that also runs Prometheus/Grafana/gatus.  It is fast and it is correct, and
      # there is no room left for a spike.  The thing that removes the risk is the second
      # pair of DIMMs (this board: 2 slots, both filled with 16 GiB DDR4-3200 running at
      # 2933, **maximum 128 GiB**), after which 4 goes back and the margin returns; the
      # purchase is the open item in `docs/follow-ups.md`.  3 was measured too: same
      # 28k (1199) and same long read (1077), but 23 % uncovered, so the first 8k rep
      # after a start costs 917 instead of 1200 - it buys ~1 GiB of margin and pays for
      # it on the cold read, which is why 2 is what is set.
      env.STRATA_RESIDENT_HEADROOM_GIB = "2";
    }
  );
in {
  options.services.strata = {
    enable = lib.mkEnableOption "Strata (Qwen3.8-Flash-Next Coder) on services2's RTX 3060";
  };

  config = lib.mkIf cfg.enable {
    # vm.swappiness 10, not the kernel's default 60.  `--resident-experts` keeps the pack
    # resident as *shared memory* - `/proc/meminfo` reads `Shmem: 25,041,688 kB` (23.9 GiB)
    # on a box with 30 GiB of RAM and a 16 GiB swap file - and shmem is swappable, so at 60
    # the kernel's first reclaim target under page-cache pressure is the expert arena and
    # the CPU pool's next expert read is a swap read.  **Measured, so nobody oversells it:
    # 10 does not make this box swap-free.** Swap use sat near 450 MiB with the engine idle
    # and grew to 1.70 GiB (engine `VmSwap` 1,077,456 kB, `SwapCached` 156 MiB) across three
    # 63,052-token prompts, at swappiness 10.  What it changed is *what the kernel reaches
    # for first*, and decode did not move either way - 31.9 / 32.0 / 32.6 / 31.2 tok/s on
    # control repeats.  So this is a churn dial, not a speed dial; the reason it is in the
    # config at all is that it had been applied with `sysctl -w` and would have died at the
    # next boot.  The whole sweep, and the fact that this box's memory pressure is
    # unmeasured as a speed factor, is machines/services2/STRATA-TUNE-2026-11-14.md.
    boot.kernel.sysctl."vm.swappiness" = 10;

    # A driver bump and a `switch` leave this box running the old NVIDIA module under
    # new user-space, and strata does not fail in that state — it just cannot reach the
    # card, so `strata.int` 502s with a healthy-looking generation. The guard makes the
    # unit say that out loud and keep retrying until the reboot fixes it.
    # common/gpu-driver-match.nix has the mechanism and the 2026-11-15 measurement.
    infra.nvidiaDriverMatchGuard = {
      enable = true;
      units = ["strata"];
    };

    # The house "never give up" policy (common/systemd.nix): Restart=always plus
    # startLimitIntervalSec=0, so a startup that fails while a leftover engine
    # still holds VRAM keeps retrying instead of parking in `failed`.
    # The driver-match guard above depends on that: it is a wait, not a failure.
    systemd.services.strata = INFRA.mkNeverGiveUp {
      description = "Strata (Qwen3.8-Flash-Next Coder across the RTX 3060, RAM and NVMe)";
      after = ["network-online.target"];
      wants = ["network-online.target"];
      wantedBy = ["multi-user.target"];

      # Don't crash-loop before the hand-run model prep has produced a pack.
      unitConfig.ConditionPathExists = "${packDir}/index.txt";

      serviceConfig = {
        ExecStart = "${strataPkg}/bin/strata-server --engine strata --config ${configFile} --host 10.3.1.21 --port 8080";
        # The CUDA sibling of the same guard in `machines/aibox/strata.nix` — read that
        # comment first, it has the mechanism (root + CPython write `__pycache__` into the
        # package's own store path; a NAR hashes the file list, not mtimes, so the path
        # stops matching its recorded hash; the box that has it cannot see that).
        #
        # Measured here 2026-11-17, i.e. **nothing to repair**: every `*-strata*` path
        # verifies clean, zero `__pycache__` anywhere despite running as root since
        # 2026-11-15.  But this unit is root, the package carries the same
        # `strata_control` modules, and there is no `PYTHON*` env — the only difference is
        # that an engine bump or a config that turns the control plane on closes it.
        # A fix that protects one of the two engines is the kind that gets forgotten.
        #
        # COST OF APPLYING: this unit restarts, and that is **55 s to `ready:`** measured
        # here (this file's own header number) with the bot and the voice assistant dialing
        # this engine — so one box at a time, and wait for `/health` before touching the
        # other.
        Environment = ["PYTHONDONTWRITEBYTECODE=1"];
        # The engine log (config "log") is written here; StateDirectory creates it.
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
