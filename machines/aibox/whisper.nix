{ lib, pkgs, ... }:

let
  # Whisper.cpp with the Vulkan backend + ffmpeg so whisper-server's --convert
  # accepts non-WAV uploads. The Vulkan build was chosen for the GTX 1060
  # (gtx1060-followup.md §4); with that card removed (2026-09-02) it needs
  # repinning — see the note under whisper-server below.
  whisperVulkan = pkgs.whisper-cpp.override {
    vulkanSupport = true;
    withFFmpegSupport = true;
  };
in
{
  environment.systemPackages = [ whisperVulkan ];

  # Whisper speech-to-text server.
  # NOTE (2026-09-02): the GTX 1060 (Vulkan1) is gone and the box now has a
  # single Vulkan device — the Radeon 660M iGPU (Vulkan0), which is
  # exclusively llama-server's — so `--device 1` below no longer exists and
  # this service cannot (re)start until it is repinned. Options: Vulkan0
  # (contends with llama-server) or drop the Vulkan override and run the CPU
  # backend. Config change pending.
  # journalctl -u whisper-server -f
  systemd.services.whisper-server = {
    description = "Whisper.cpp speech-to-text server (GTX 1060)";
    after = [ "wait-for-network.service" ];
    wants = [ "wait-for-network.service" ];
    wantedBy = [ "multi-user.target" ];

    serviceConfig =
      {
        ExecStart = ''
          ${whisperVulkan}/bin/whisper-server \
            --host 10.3.1.32 \
            --port 8082 \
            --model /home/leigh-admin/Models/whisper/ggml-medium.bin \
            --device 1 \
            --convert \
            --tmp-dir /tmp \
            -t 8
        '';
        # The 1060 rides on an M.2→USB3 adapter and has historically been
        # intermittent; keep retrying rather than giving up on a GPU hiccup.
        Restart = "always";
        RestartSec = 5;
      };
  };

  # OpenAI-Realtime-compatible WebSocket gateway in front of whisper-server
  # (source: ~/Projects/whisper-ws, stdlib-only Rust). Clients speak the
  # Realtime transcription subset over ws://10.3.1.32:8083/v1/realtime
  # (input_audio_buffer.append/commit → transcription.completed).
  systemd.services.whisper-ws = {
    description = "WebSocket gateway for whisper.cpp (OpenAI Realtime protocol)";
    after = [
      "whisper-server.service"
      "wait-for-network.service"
    ];
    # Useless without whisper-server; restart with it.
    requires = [ "whisper-server.service" ];
    wants = [ "wait-for-network.service" ];
    wantedBy = [ "multi-user.target" ];

    serviceConfig = {
      ExecStart = ''
        ${pkgs.whisper-ws}/bin/whisper-ws \
          --bind 10.3.1.32 \
          --port 8083 \
          --whisper-url http://10.3.1.32:8082
      '';
      Restart = "always";
      RestartSec = 5;
    };
  };
}
