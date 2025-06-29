## NOTES: Get the container image...
# podman build -t llama-cpp-vulkan --target server -f .devops/vulkan.Dockerfile .
# podman save llama-cpp-vulkan -o llama-cpp-vulkan.tar
# sudo podman load -i llama-cpp-vulkan.tar

{ config, pkgs, ... }:

{
  environment.systemPackages = with pkgs; [
    llama-cpp-leigh-rocm
    rocmPackages.rocminfo
  ];

  # I think stable-diffusion-webui needs this
  systemd.tmpfiles.rules = [
    "L+    /opt/rocm   -    -    -     -    ${pkgs.rocmPackages.clr}"
  ];

  # Run a 32B DeepSeek model on the GPU (thanks to UMA of the Ryzen 6600H)
  # View logs with: journalctl -u podman-llama-server-deepseek -f
  virtualisation.oci-containers.containers = {
    llama-server-32b = {
      hostname = "llama-server-32b";
      image = "llama-cpp-vulkan";
      cmd = [
        "-m"
        "/llama.cpp/models/DeepSeek-R1-Distill-Qwen-32B-Q6_K_L.gguf"
        "-ngl"
        "65"
        "--ctx-size"
        "8192"
        "--port"
        "8080"
        "--slots"
        "--metrics"
      ];
      autoStart = true;
      ports = [
        "8080:8080"
      ];
      volumes = [
        "/home/cjdell/Projects/llama.cpp:/llama.cpp:Z"
      ];
      environment = {
        TZ = "Europe/London";
      };
      extraOptions = [
        "--device=/dev/dri/renderD128"
        "--device=/dev/dri/card1"
      ];
    };
  };

  # View logs with: journalctl -u podman-llama-server-8b -f
  virtualisation.oci-containers.containers = {
    llama-server-8b = {
      hostname = "llama-server-8b";
      image = "llama-cpp-vulkan";
      cmd = [
        "-m"
        "/llama.cpp/models/nvidia_Llama-3.1-8B-UltraLong-4M-Instruct-Q6_K_L.gguf"
        "-ngl"
        "65"
        # "--ctx-size"
        # "0"
        "--port"
        "8080"
        "--parallel"
        "2"
        "--slots"
        "--metrics"
      ];
      autoStart = true;
      ports = [
        "8081:8080"
      ];
      volumes = [
        "/home/cjdell/Projects/llama.cpp:/llama.cpp:Z"
      ];
      environment = {
        TZ = "Europe/London";
      };
      extraOptions = [
        "--device=/dev/dri/renderD128"
        "--device=/dev/dri/card1"
      ];
    };
  };

  # View logs with: journalctl -u stable-diffusion -f
  systemd.services.stable-diffusion = {
    description = "Stable Diffusion";
    after = [ "network.target" ];

    # Ensure the service is started at boot
    wantedBy = [ "multi-user.target" ];

    serviceConfig = {
      ExecStart = "${pkgs.nix}/bin/nix develop .#rocm --command \"./webui.sh\"";
      WorkingDirectory = "/home/cjdell/Projects/stable-diffusion-webui";
      Restart = "always";
      User = "cjdell";
      Group = "users";
    };
  };
}
