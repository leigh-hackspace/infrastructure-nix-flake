{ lib, pkgs, ... }:

let
  # Pin the main model to the iGPU: with the GTX 1060 (Vulkan1) present,
  # auto device selection splits layers onto it and model load fails
  # (alloc + compute-pipeline errors; see gtx1060-draft-report.md).
  llamaServer = "${pkgs.llama-cpp-leigh-vulkan}/bin/llama-server";
  modelsPath = "/home/leigh-admin/Models";
  # Per-model speculative decoding presets for the inner llama.cpp routers.
  # `draft-mtp` reuses the model's own MTP module (`blk.N.nextn.*` tensors) as
  # the draft — no separate draft model, no extra VRAM. Only MTP-capable models
  # are listed; everything else loads without speculation.
  mtpPresets = pkgs.writeText "llama-mtp-presets" ''
    version = 1

    [Ornith-1.5-35B-A3B-GGUF]
    spec-type = draft-mtp
    [Tiel-Coder-35B-A3B-GGUF-MTP]
    spec-type = draft-mtp
  '';

  # Tiel-Coder-35B-A3B-GGUF-MTP
in
{
  environment.systemPackages = with pkgs; [
    rocmPackages.rocminfo
    python3Packages.huggingface-hub # provides the huggingface-cli model downloader
  ];

  # I think stable-diffusion-webui needs this
  systemd.tmpfiles.rules = [
    "L+    /opt/rocm   -    -    -     -    ${pkgs.rocmPackages.clr}"
  ];

  # Stop crashes for large context sizes
  boot.kernelParams = [ "amdgpu.lockup_timeout=10000" ];

  # Serve the single model directly (llama-swap's router was redundant with
  # only one model in the list).
  # journalctl -u llama-server -f
  systemd.services.llama-server = {
    description = "Llama.cpp server";
    after = [ "wait-for-network.service" ];
    wants = [ "wait-for-network.service" ];
    wantedBy = [ "multi-user.target" ];

    serviceConfig =
      {
        ExecStart = ''
          ${llamaServer} \
            --host 10.3.1.32 \
            --port 8081 \
            --models-dir ${modelsPath} \
            --models-preset ${mtpPresets} \
            --models-max 1 \
            -t 12 \
            -dev Vulkan0 \
            -ngl all \
            --ctx-size 262144 \
            --flash-attn on
        '';
        WorkingDirectory = "/home/leigh-admin/Projects/infrastructure-nix-flake";
        Restart = "always";
      };
  };

  # sudo podman build .
  # sudo podman tag b9856843437b diamcp:latest
  virtualisation.oci-containers.containers.diamcp = {
    hostname = "diamcp";
    image = "localhost/diamcp";
    autoStart = true;
    ports = [ "8000:8000" ];
    volumes = [
      # "/home/leigh-admin/workspace:/workspace"
      "/mnt/filestore/ai-workspace:/workspace"
    ];
    extraOptions = [
      "--user=3002:100"
    ];
  };

  systemd.services.podman-diamcp = {
    requires = [ "wait-for-network.service" ];
    after = [ "wait-for-network.service" ];
  };

  # # View logs with: journalctl -u stable-diffusion -f
  # systemd.services.stable-diffusion = {
  #   description = "Stable Diffusion";
  #   after = [ "network.target" ];

  #   # Ensure the service is started at boot
  #   wantedBy = [ "multi-user.target" ];

  #   serviceConfig = {
  #     ExecStart = "${pkgs.nix}/bin/nix develop .#rocm --command "./webui.sh\"";
  #     WorkingDirectory = "/home/leigh-admin/Projects/stable-diffusion-webui";
  #     Restart = "always";
  #     User = "leigh-admin";
  #     Group = "users";
  #   };
  # };
}
