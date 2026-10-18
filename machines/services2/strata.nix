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
# so this is reachable on the LAN directly; services1 also terminates TLS for it
# as https://strata.int.leighhack.org (machines/services1/services/strata-proxy.nix).
# Since 2026-10-18 the engine also answers publicly at
# https://strata.leighhack.org: services1's nginx terminates that name on its own
# public IPs (81.187.195.17 / 2001:8b0:1d14:225::20) and proxies over the LAN to
# 10.3.1.21:8080, because services2 itself has no public IP.  That is only safe
# because the engine now has an api_key (apiKey below): /health says "api_key":
# true and every /v1/* needs it (401 "missing or wrong API key" without one).
#
# Only the LAN names are in allowed_hosts below.  Upstream runs that Host check
# only when it has no key (serve/server.py parse_request: `if svc.api_key or
# host_allowed ...`), so the key is what lets the public name through, and the
# public name staying out of the list is the fail-safe: take the key out and the
# public vhost starts answering 403 instead of serving an unauthenticated engine
# to the whole world.  See the ⚠️ note in strata-proxy.nix for why a LAN vhost
# and this list still have to move together.
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
  #   KV int8 at 131072           ~1.8 GiB  stays in VRAM (setup.py's rule: KV
  #                               streaming to RAM needs the model's ~32 GB plus
  #                               the cache; this box is short of that).
  context = 131072;

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
    "--kv"
    "int8"
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
      # The names this server answers to.  Keep in step with the vhosts on
      # services1 that proxy :8080 (machines/services1/services/strata-proxy.nix):
      # nginx forwards the incoming Host header upstream, and host_allowed() in
      # serve/server.py 403s anything not listed here (an IP passes on trust).
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
    }
  );
in {
  options.services.strata = {
    enable = lib.mkEnableOption "Strata (Qwen3.8-Flash-Next Coder) on services2's RTX 3060";
  };

  config = lib.mkIf cfg.enable {
    # The house "never give up" policy (common/systemd.nix): Restart=always plus
    # startLimitIntervalSec=0, so a startup that fails while a leftover engine
    # still holds VRAM keeps retrying instead of parking in `failed`.
    systemd.services.strata = INFRA.mkNeverGiveUp {
      description = "Strata (Qwen3.8-Flash-Next Coder across the RTX 3060, RAM and NVMe)";
      after = ["network-online.target"];
      wants = ["network-online.target"];
      wantedBy = ["multi-user.target"];

      # Don't crash-loop before the hand-run model prep has produced a pack.
      unitConfig.ConditionPathExists = "${packDir}/index.txt";

      serviceConfig = {
        ExecStart = "${strataPkg}/bin/strata-server --engine strata --config ${configFile} --host 10.3.1.21 --port 8080";
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
