{
  lib,
  pkgs,
  INFRA,
  ...
}: let
  # Whisper.cpp with the Vulkan backend + ffmpeg so whisper-server's --convert
  # accepts non-WAV uploads. The Vulkan build was chosen for the GTX 1060, removed on
  # 2026-09-02; the pin therefore needs
  # repinning — see the note under whisper-server below.
  whisperVulkan = pkgs.whisper-cpp.override {
    vulkanSupport = true;
    withFFmpegSupport = true;
  };
in {
  environment.systemPackages = [whisperVulkan];

  # Whisper speech-to-text server.
  #
  # NOTE (2026-09-02, corrected 2026-11-16): the GTX 1060 (Vulkan1) is gone, and this
  # unit still passes `--device 1`, which no longer exists.  What the old note asserted —
  # "this service cannot (re)start until it is repinned" — was **falsified by the live
  # box**: the unit is `active`, `NRestarts=0`, started 2026-11-15 03:08, and answers.
  # What actually happens is quieter and worse than a crash: `--device 1` is out of range
  # (`whisper-server --help` at whisper.cpp 1.9.5: `-dev N, --device N [0] GPU device
  # ID`, and `Found 1 Vulkan devices: 0 = AMD Radeon 660M`), so it **falls back to CPU
  # without complaining** — the process holds no `/dev/dri` fd and its own `system_info:`
  # line lists CPU features and no Vulkan.  A 3 s clip transcribes in ~2.7 s that way,
  # which is why nobody noticed.  So the pending config change is not "make it start", it
  # is "decide which backend this is supposed to use": Vulkan0 contends with
  # `llama-server`, which owns that iGPU exclusively, and CPU at 8 of 12 threads is what
  # `crates/voice-assistant` has now measured its latency budget against
  # (`docs/voice-assistant-2026-11-16.md`).  Removing the flag is a restart of a service
  # the whole network's speech path depends on, so it is a deploy decision and is open in
  # `docs/follow-ups.md`, not a comment to delete.
  #
  # Two facts worth having beside the flags, both read off the running unit:
  # `journalctl -u whisper-server -f` shows **ffmpeg's whole banner and config dump per
  # request** (`--convert` shells out to ffmpeg without `-hide_banner`, ~20 KB each time) —
  # harmless, and now the thing to expect when the voice gateway asks per utterance.
  # The 1060 rode on an M.2→USB3 adapter and was historically intermittent, which is why
  # this unit keeps retrying rather than giving up (INFRA.mkNeverGiveUp); with no 1060 in
  # the box that policy is now load-bearing for a different reason — a CPU whisper-server
  # has no GPU to lose, so a restart here should mean the model or the port moved.
  systemd.services.whisper-server = INFRA.mkNeverGiveUp {
    description = "Whisper.cpp speech-to-text server (CPU: see the note above, `--device 1` is out of range)";
    after = ["wait-for-network.service"];
    wants = ["wait-for-network.service"];
    wantedBy = ["multi-user.target"];

    serviceConfig = {
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
    };
  };

  # OpenAI-Realtime-compatible WebSocket gateway in front of whisper-server
  # (source: crates/whisper-ws, stdlib-only Rust, packaged by its own
  # crates/whisper-ws/package.nix). Clients speak the
  # Realtime transcription subset over ws://10.3.1.32:8083/v1/realtime
  # (input_audio_buffer.append/commit → transcription.completed).
  #
  # `--assistant-url` is the origin the page's Assistant card calls: services1's voice
  # gateway, surfaced through `/status.json` rather than compiled into the bundle,
  # because a hostname baked into a SPA can only be changed by rebuilding it
  # (`crates/whisper-ws/src/status.rs`).
  systemd.services.whisper-ws = INFRA.mkNeverGiveUp {
    description = "WebSocket gateway for whisper.cpp (OpenAI Realtime protocol)";
    after = [
      "whisper-server.service"
      "wait-for-network.service"
    ];
    # Useless without whisper-server; restart with it.
    requires = ["whisper-server.service"];
    wants = ["wait-for-network.service"];
    wantedBy = ["multi-user.target"];

    serviceConfig = {
      ExecStart = ''
        ${pkgs.whisper-ws}/bin/whisper-ws \
          --bind 10.3.1.32 \
          --port 8083 \
          --whisper-url http://10.3.1.32:8082 \
          --assistant-url https://voice.int.leighhack.org
      '';
    };
  };
}
