# Leigh Hackspace Infrastructure Nix Flake

Provides the configuration for various servers running Leigh Hackspack infrastructure.

## Applying Config

```bash
sudo nixos-rebuild boot   --flake . --impure --max-jobs 1    # Build for next reboot
sudo nixos-rebuild switch --flake . --impure --max-jobs 1    # Build and apply now
```

podman run -it --rm -v "/home/cjdell/Projects/llama.cpp:/llama.cpp:Z" --device /dev/dri/renderD128:/dev/dri/renderD128 --device /dev/dri/card1:/dev/dri/card1 llama-cpp-vulkan-light -m "/llama.cpp/models/nvidia_Llama-3.1-8B-UltraLong-4M-Instruct-Q6_K_L.gguf" -p "Building a website can be done in 10 simple steps:" -n 400 -e -ngl 65

podman run -it --rm -p 8081:8080 -v /home/cjdell/Projects/llama.cpp:/llama.cpp:Z -e TZ=Europe/London --device=/dev/dri/renderD128 --device=/dev/dri/card1 llama-cpp-vulkan -m /llama.cpp/models/nvidia_Llama-3.1-8B-UltraLong-4M-Instruct-Q6_K_L.gguf -ngl 65 --ctx-size 0 --port 8080 --parallel 2 --slots --metrics

podman run -it --rm -p 8081:8080 -v /home/cjdell/Projects/llama.cpp:/llama.cpp:Z -e TZ=Europe/London --device=/dev/dri/renderD128 --device=/dev/dri/card1 llama-cpp-vulkan-server -m /llama.cpp/models/nvidia_Llama-3.1-8B-UltraLong-4M-Instruct-Q6_K_L.gguf -ngl 65 --port 8080 --parallel 2 --slots --metrics
