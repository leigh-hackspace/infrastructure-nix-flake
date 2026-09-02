# Alexandria audiobook generator (github.com/Finrandojin/alexandria-audiobook).
# Cloned to ~/Projects/alexandria-audiobook; WebUI on port 4200.
#
# It turns books into voiced audiobooks: an OpenAI-compatible LLM (point it at
# llama-server on this box, http://10.3.1.32:8081/v1) annotates the script and
# a built-in Qwen3-TTS engine voices it (or use an external Qwen3-TTS server).
#
# The upstream Docker image is CUDA-tagged, but aibox has no NVIDIA GPU: the
# only GPU is the AMD Radeon 660M iGPU (gfx1035). The container therefore runs
# the ROCm build of PyTorch (see Dockerfile.rocm) and gets the iGPU via
# /dev/kfd + /dev/dri, so Qwen3-TTS sees it as a HIP "cuda" device. The 660M
# is also llama-server's (Vulkan) — both share the same 6 CUs and UMA RAM, so
# expect contention if the LLM is loaded during a long TTS batch.
#
# Images are built manually from the clone (docker.io base, ~9 GB; :rocm adds
# the self-contained ROCm 6.4 torch wheel, ~22 GB total):
#   cd ~/Projects/alexandria-audiobook
#   sudo podman build -t localhost/alexandria:latest .
#   sudo podman build -f Dockerfile.rocm -t localhost/alexandria:rocm .
# (rebuild :rocm whenever :latest changes; restart podman-alexandria to pick
# the new image)
#
# journalctl -u podman-alexandria -f
# sudo podman exec -ti alexandria sh
{
  pkgs,
  ...
}:

let
  dataDir = "/home/leigh-admin/Projects/alexandria-audiobook/data";
  dataSubdirs = [
    "config"
    "uploads"
    "designed_voices"
    "clone_voices"
    "lora_models"
    "lora_datasets"
    "dataset_builder"
    "scripts"
    "output"
    "hf-cache"
  ];
in
{
  virtualisation.oci-containers.containers.alexandria = {
    hostname = "alexandria";
    image = "localhost/alexandria:rocm";
    autoStart = true;
    ports = [
      "4200:4200"
    ];
    volumes = [
      # WebUI settings (OpenAI endpoint, prompts, TTS mode, ...)
      "${dataDir}/config:/alexandria/config"
      # User uploads (source books)
      "${dataDir}/uploads:/alexandria/app/uploads"
      # User-generated assets / state
      "${dataDir}/designed_voices:/alexandria/designed_voices"
      "${dataDir}/clone_voices:/alexandria/clone_voices"
      "${dataDir}/lora_models:/alexandria/lora_models"
      "${dataDir}/lora_datasets:/alexandria/lora_datasets"
      "${dataDir}/dataset_builder:/alexandria/dataset_builder"
      "${dataDir}/scripts:/alexandria/scripts"
      # Audiobook output
      "${dataDir}/output:/alexandria/voicelines"
      # HuggingFace model cache (~3.5 GB per model, downloaded on first use)
      "${dataDir}/hf-cache:/root/.cache/huggingface"
      "/etc/localtime:/etc/localtime:ro"
    ];
    environment = {
      # Mirrors docker-compose.yml
      ALEXANDRIA_CONFIG_PATH = "/alexandria/config/config.json";
      # gfx1035 (Rembrandt iGPU) is not a torch wheel target: without this,
      # HIP launches fail with "invalid device function". 10.3.0 = gfx1030
      # code objects, which run fine on the 660M (verified 2026-09-02).
      HSA_OVERRIDE_GFX_VERSION = "10.3.0";
    };
    # AMD iGPU passthrough for the ROCm PyTorch build (KFD + render node).
    extraOptions = [
      "--device=/dev/kfd"
      "--device=/dev/dri/renderD128"
    ];
  };

  # Podman refuses bind mounts whose host dir is missing ("statfs: no such
  # file or directory"), so create the data dirs before every start. The app
  # state (WebUI settings, uploads, voices, LoRAs, output) lives here so it
  # survives container recreations (the generated unit `podman rm -f`s + re-
  # runs on every start).
  systemd.services.podman-alexandria.serviceConfig.ExecStartPre = [
    (pkgs.writeShellScript "alexandria-data-dirs" ''
      mkdir -p ${dataDir} ${builtins.concatStringsSep " " (map (d: "${dataDir}/${d}") dataSubdirs)}
      chown leigh-admin:users ${dataDir}
      chmod 0775 ${dataDir} ${builtins.concatStringsSep " " (map (d: "${dataDir}/${d}") dataSubdirs)}
    '')
  ];
}
