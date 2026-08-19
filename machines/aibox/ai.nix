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
        llamaCmdVulkan = "${pkgs.llama-cpp-leigh-vulkan}/bin/llama-server --host 127.0.0.1 --port \${PORT} -t 12";
        modelsPath = "/home/leigh-admin/Projects/llama.cpp.new/models";

        # Native Nix structure representing the YAML config
        llamaConfig = {
          models = {
            "[Reasoning] gemma-4-26B-A4B-it-UD-Q4_K_M" = {
              cmd = "${llamaCmdVulkan} -m ${modelsPath}/gemma-4-26B-A4B-it-UD-Q4_K_M.gguf --mmproj ${modelsPath}/mmproj-F16.gguf -ngl all --ctx-size 0 --metrics";
            };

            "[Reasoning] ornith-1.0-9b-Q4_K_M " = {
              cmd = "${llamaCmdVulkan} -m ${modelsPath}/ornith-1.0-9b-Q4_K_M.gguf -ngl all --ctx-size 0 --metrics";
            };

            "[Vision] Qwen2.5-VL-7B-Instruct-Q8_0" = {
              cmd = "${llamaCmdVulkan} -m ${modelsPath}/Qwen2.5-VL-7B-Instruct-Q8_0.gguf --mmproj ${modelsPath}/mmproj-Qwen2.5-VL-7B-Instruct-Q8_0.gguf -ngl all --ctx-size 0 --metrics";
            };

            "[General] Qwen3-Next-80B-A3B-Instruct-Q4_K_S" = {
              cmd = "${llamaCmdVulkan} -m ${modelsPath}/Qwen3-Next-80B-A3B-Instruct-Q4_K_S.gguf -ngl all --ctx-size 262144 --metrics";
            };

            # 118B-A8B coding/agentic model. Requires llama.cpp >= b10087 (laguna arch)
            # - the pinned llama-cpp flake input (Aug 2026) already includes it.
            # UD-IQ3_S 48.4GB: fastest quant that fully fits the ~56G GTT with headroom
            # for KV cache + compute buffers. Alternatives below (uncomment to use):
            #   UD-Q3_K_XL 54.1GB - better quality, still fits (3 shards; pass 00001)
            #   UD-IQ2_M 37.3GB   - max tokens/s, noticeably lower quality
            # Thinking is OFF by default in this model's chat template (enable_thinking
            # defaults to false, so the prompt says </think> with no opening tag). The
            # --chat-template-kwargs flag turns it on server-wide; clients can override
            # per request with "chat_template_kwargs": {"enable_thinking": false}.
            "[Coding] Laguna-S-2.1-UD-IQ3_S" = {
              cmd = "${llamaCmdVulkan} -m ${modelsPath}/Laguna-S-2.1-UD-IQ3_S.gguf -ngl all -fa on --ctx-size 65536 --metrics --chat-template-kwargs '{\"enable_thinking\":true}'";
            };
            # "[Coding] Laguna-S-2.1-UD-Q3_K_XL" = {
            #   cmd = "${llamaCmdVulkan} -m ${modelsPath}/Laguna-S-2.1-UD-Q3_K_XL-00001-of-00003.gguf -ngl all -fa on --ctx-size 65536 --metrics --chat-template-kwargs '{\"enable_thinking\":true}'";
            # };
            # "[Coding] Laguna-S-2.1-UD-IQ2_M" = {
            #   cmd = "${llamaCmdVulkan} -m ${modelsPath}/Laguna-S-2.1-UD-IQ2_M.gguf -ngl all -fa on --ctx-size 65536 --metrics --chat-template-kwargs '{\"enable_thinking\":true}'";
            # };
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
