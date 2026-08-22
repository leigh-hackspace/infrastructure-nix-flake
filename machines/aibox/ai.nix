{
  pkgs,
  lib,
  ...
}:

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

  # Run multiple models on the GPU (thanks to UMA of the Ryzen 6600H)
  # journalctl -u llama-swap -f
  systemd.services.llama-swap = {
    description = "Llama Swap";
    after = [ "wait-for-network.service" ];
    wants = [ "wait-for-network.service" ];
    wantedBy = [ "multi-user.target" ];

    serviceConfig =
      let
        # Pin the main model to the iGPU: with the GTX 1060 (Vulkan1) present,
        # auto device selection splits layers onto it and model load fails
        # (alloc + compute-pipeline errors; see gtx1060-draft-report.md).
        # Draft models go on Vulkan1 later via -devd, not -dev.
        llamaCmdVulkan = "${pkgs.llama-cpp-leigh-vulkan}/bin/llama-server --host 127.0.0.1 --port \${PORT} -t 12 -dev Vulkan0";
        modelsPath = "/home/leigh-admin/Models";

        # Per-model configuration for the router (INI preset, docs/preset.md).
        # Section names must match the GGUF file names (without .gguf) in
        # ${modelsPath}; keys are CLI args without dashes (long, short or
        # env-var form). [*] holds defaults for every model. NOTE: args on the
        # router's command line override the preset, so keep model-shaping
        # args in this file, not on the CLI. Extra preset-only keys:
        # load-on-startup (preload at boot), stop-timeout (idle unload delay,
        # seconds).
        modelsPresetFile = pkgs.writeText "llama-models-preset.ini" ''
          version = 1

          [*]
          n-gpu-layers = all
          ctx-size     = 131072

          [Qwen3-Next-80B-A3B-Instruct-Q4_K_S]
          ctx-size = 262144

          # 118B-A8B coding/agentic model (laguna arch).
          # UD-IQ3_S 48.4GB: fastest quant that fully fits the ~56G GTT with
          # headroom for KV cache + compute buffers. Alternatives (rename the
          # file and change the section name to switch):
          #   UD-Q3_K_XL 54.1GB - better quality, still fits (3 shards; pass 00001)
          #   UD-IQ2_M 37.3GB   - max tokens/s, noticeably lower quality
          # Thinking is OFF by default in this model's chat template (the
          # prompt ends with a closing think tag but no opening one); the flag
          # below turns it on. Clients can override per request with
          # "chat_template_kwargs": {"enable_thinking": false}.
          [Laguna-S-2.1-UD-IQ3_S]
          ctx-size     = 65536
          flash-attn   = 1
          chat-template-kwargs = {"enable_thinking":true}
        '';

        # Native Nix structure representing the YAML config
        llamaConfig = {
          models = {
            "router" = {
              cmd = "${llamaCmdVulkan} --models-dir ${modelsPath} --models-preset ${modelsPresetFile} --models-max 1 --metrics";
            };
          };
        };

        # Convert native Nix structure to YAML
        configYaml = lib.generators.toYAML { } llamaConfig;
      in
      {
        ExecStart = "${lib.getExe pkgs.llama-swap} -listen 10.3.1.32:8081 -config ${pkgs.writeText "llama-swap-config" configYaml}";
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
  #     ExecStart = "${pkgs.nix}/bin/nix develop .#rocm --command \"./webui.sh\"";
  #     WorkingDirectory = "/home/leigh-admin/Projects/stable-diffusion-webui";
  #     Restart = "always";
  #     User = "leigh-admin";
  #     Group = "users";
  #   };
  # };
}
