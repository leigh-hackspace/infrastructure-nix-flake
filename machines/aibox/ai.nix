{ config, pkgs, ... }:

{
  environment.systemPackages = with pkgs; [
    llama-cpp-local-cpu
    rocmPackages.rocminfo
  ];

  systemd.tmpfiles.rules = [
    "L+    /opt/rocm/hip   -    -    -     -    ${pkgs.rocmPackages.clr}"
  ];

  # Run a 32B DeepSeek model on the GPU (thanks to UMA of the Ryzen 6600H)
  # View logs with: sudo journalctl -u llama-server-deepseek -f
  systemd.services.llama-server-deepseek = {
    description = "LLaMa Server Deepseek";
    after = [ "network.target" ];

    # Ensure the service is started at boot
    wantedBy = [ "multi-user.target" ];

    # -t 1      = Use a single CPU core
    # -ngl 1000 = Use the GPU for the rest
    serviceConfig = {
      ExecStart = "${pkgs.llama-cpp-leigh-rocm}/bin/llama-server -m /home/cjdell/Models/DeepSeek-R1-Distill-Qwen-32B-Q6_K_L.gguf -t 1 --host 0.0.0.0 --port 8080 -ngl 1000";
      Restart = "always";
      EnvironmentFile = pkgs.writeText "llama-server-deepseek-env" ''
        HSA_OVERRIDE_GFX_VERSION=10.3.0
      '';
    };
  };

  # Run a 7B Mistral model on the CPU
  # Source: https://huggingface.co/TheBloke/Mistral-7B-Instruct-v0.2-GGUF
  # View logs with: sudo journalctl -u llama-server-7b -f
  systemd.services.llama-server-7b = {
    description = "LLaMa Server Deepseek";
    after = [ "network.target" ];

    # Ensure the service is started at boot
    wantedBy = [ "multi-user.target" ];

    # -t 11 = Use the other 11 CPU cores
    serviceConfig = {
      ExecStart = "${pkgs.llama-cpp-leigh-rocm}/bin/llama-server -m /home/cjdell/Models/mistral-7b-instruct-v0.2.Q6_K.gguf -t 11 --host 0.0.0.0 --port 8081";
      Restart = "always";
    };
  };
}
