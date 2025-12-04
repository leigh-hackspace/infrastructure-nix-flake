## NOTES: Get the container image...
# cd ../llama.cpp.new
# podman build -t llama-cpp-vulkan --target server -f .devops/vulkan.Dockerfile .
# podman save llama-cpp-vulkan -o llama-cpp-vulkan.tar
# sudo podman load -i llama-cpp-vulkan.tar

{
  config,
  pkgs,
  lib,
  ...
}:

{
  environment.systemPackages = with pkgs; [
    rocmPackages.rocminfo
  ];

  # I think stable-diffusion-webui needs this
  systemd.tmpfiles.rules = [
    "L+    /opt/rocm   -    -    -     -    ${pkgs.rocmPackages.clr}"
  ];

  # Run multiple models on the GPU (thanks to UMA of the Ryzen 6600H)
  # journalctl -u llama-swap -f
  systemd.services.llama-swap = {
    description = "Llama Swap";
    after = [ "wait-for-network.service" ];
    wants = [ "wait-for-network.service" ];
    wantedBy = [ "multi-user.target" ];

    serviceConfig =
      let
        llamaCmdRocm = "${pkgs.llama-cpp-leigh-rocm}/bin/llama-server --host 127.0.0.1 --port \${PORT} -t 12";
        # llamaCmdVulkan = "${pkgs.llama-cpp-leigh-vulkan}/bin/llama-server --host 127.0.0.1 --port \${PORT} -t 12";
        llamaCmdPodman = "${lib.getExe pkgs.podman} run --rm -v /home/leigh-admin/Projects/llama.cpp.new/models:/models:Z -p \${PORT}:8080 --device=/dev/dri/renderD128 --device=/dev/dri/card0 localhost/llama-cpp-vulkan -t 12";
        modelsPath = "/home/leigh-admin/Projects/llama.cpp.new/models";
        modelsPathPodman = "/models";

        # Native Nix structure representing the YAML config
        llamaConfig = {
          models = {
            "nvidia_Llama-3.1-8B-UltraLong-4M-Instruct-Q6_K_L" = {
              cmd = "${llamaCmdRocm} -m ${modelsPath}/nvidia_Llama-3.1-8B-UltraLong-4M-Instruct-Q6_K_L.gguf -ngl 100 --ctx-size 0 --metrics";
            };
            "Qwen2.5-VL-7B-Instruct-Q8_0" = {
              cmd = "${llamaCmdRocm} -m ${modelsPath}/Qwen2.5-VL-7B-Instruct-Q8_0.gguf --mmproj ${modelsPath}/mmproj-Qwen2.5-VL-7B-Instruct-Q8_0.gguf -ngl 100 --ctx-size 0 --metrics";
            };
            "Qwen3VL-8B-Instruct-Q8_0" = {
              cmd = "${llamaCmdRocm} -m ${modelsPath}/Qwen3VL-8B-Instruct-Q8_0.gguf --mmproj ${modelsPath}/mmproj-Qwen3VL-8B-Instruct-Q8_0.gguf -ngl 100 --ctx-size 0 --metrics";
            };
            "DeepSeek-R1-Distill-Qwen-32B-Q6_K_L" = {
              cmd = "${llamaCmdRocm} -m ${modelsPath}/DeepSeek-R1-Distill-Qwen-32B-Q6_K_L.gguf -ngl 100 --ctx-size 0 --metrics";
            };
            "gemma-3-27b-it-Q8_0" = {
              cmd = "${llamaCmdRocm} -m ${modelsPath}/gemma-3-27b-it-Q8_0.gguf --mmproj ${modelsPath}/mmproj-model-f16.gguf -ngl 100 --ctx-size 0 --metrics";
            };
            "Qwen3-Next-80B-A3B-Instruct-Q4_K_M" = {
              # Crashes in ROCm
              # No GPU in Vulkan
              # Podman works for some reason...
              cmd = "${llamaCmdPodman} -m ${modelsPathPodman}/Qwen3-Next-80B-A3B-Instruct-Q4_K_M.gguf -ngl 100 --ctx-size 16384 --metrics";
              # cmd = "${llamaCmdVulkan} -m ${modelsPath}/Qwen3-Next-80B-A3B-Instruct-Q4_K_M.gguf -ngl 100 --ctx-size 16384 --metrics";
            };
            "Qwen3-Coder-30B-A3B-Instruct-Q4_K_M" = {
              cmd = "${llamaCmdRocm} -m ${modelsPath}/Qwen3-Coder-30B-A3B-Instruct-Q4_K_M.gguf -ngl 100 --ctx-size 0 --metrics";
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

  # # View logs with: journalctl -u podman-llama-server-8b -f
  # virtualisation.oci-containers.containers = {
  #   llama-server-8b = {
  #     hostname = "llama-server-8b";
  #     image = "llama-cpp-vulkan";
  #     cmd = [
  #       "-m"
  #       "/models/nvidia_Llama-3.1-8B-UltraLong-4M-Instruct-Q6_K_L.gguf"
  #       "-ngl"
  #       "65"
  #       # "--ctx-size"
  #       # "0"
  #       "--port"
  #       "8080"
  #       "--parallel"
  #       "2"
  #       "--slots"
  #       "--metrics"
  #     ];
  #     autoStart = true;
  #     ports = [
  #       "8081:8080"
  #     ];
  #     volumes = [
  #       "/home/leigh-admin/Projects/llama.cpp.new/models:/models:Z"
  #     ];
  #     environment = {
  #       TZ = "Europe/London";
  #     };
  #     extraOptions = [
  #       "--device=/dev/dri/renderD128"
  #       "--device=/dev/dri/card0"
  #     ];
  #   };
  # };

  # View logs with: journalctl -u llama-server-8b -f
  # systemd.services.llama-server-8b = {
  #   description = "LLaMa Server ROCm";
  #   after = [ "network.target" ];

  #   # Ensure the service is started at boot
  #   wantedBy = [ "multi-user.target" ];

  #   serviceConfig = {
  #     ExecStart = "${pkgs.llama-cpp-rocm}/bin/llama-server -m /home/leigh-admin/Projects/llama.cpp.new/models/nvidia_Llama-3.1-8B-UltraLong-4M-Instruct-Q6_K_L.gguf --host 0.0.0.0 --port 8082 -ngl 100";
  #     Restart = "always";
  #   };
  # };

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
