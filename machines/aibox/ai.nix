{
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
        llamaCmdVulkan = "${pkgs.llama-cpp-leigh-vulkan}/bin/llama-server --host 127.0.0.1 --port \${PORT} -t 12";
        modelsPath = "/home/leigh-admin/Projects/llama.cpp.new/models";

        # Native Nix structure representing the YAML config
        llamaConfig = {
          models = {
            "[Reasoning] gemma-4-26B-A4B-it-UD-Q4_K_M" = {
              cmd = "${llamaCmdVulkan} -m ${modelsPath}/gemma-4-26B-A4B-it-UD-Q4_K_M.gguf --mmproj ${modelsPath}/mmproj-F16.gguf -ngl 100 --ctx-size 0 --metrics";
            };

            "[Vision] Qwen2.5-VL-7B-Instruct-Q8_0" = {
              cmd = "${llamaCmdVulkan} -m ${modelsPath}/Qwen2.5-VL-7B-Instruct-Q8_0.gguf --mmproj ${modelsPath}/mmproj-Qwen2.5-VL-7B-Instruct-Q8_0.gguf -ngl 100 --ctx-size 0 --metrics";
            };

            "[General] Qwen3-Next-80B-A3B-Instruct-Q4_K_S" = {
              cmd = "${llamaCmdVulkan} -m ${modelsPath}/Qwen3-Next-80B-A3B-Instruct-Q4_K_S.gguf -ngl 100 --ctx-size 32768 --metrics";
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
