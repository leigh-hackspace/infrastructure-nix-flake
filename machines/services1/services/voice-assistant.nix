# A voice assistant for the space: a browser page or cam8's mic in, a spoken answer
# out.  Two units, one binary, no secrets, and the camera speaker switched off.
#
# ```
#   cam8 mic ──go2rtc restream──▶ voice-satellite ──┐
#   browser mic (whisper.int) ─────────────────────┤
#                                                  ▼
#                              voice-gateway :8193 (loopback)
#                                ├ whisper.cpp on aibox :8082   (speech → text)
#                                ├ slack-agent listen :8194     (text → answer)
#                                └ piper en_GB-alba-medium      (answer → speech)
#                                                  │
#                              speaker off: the WAV is written to the state dir.
# ```
#
# ## Why a browser page at all
#
# The camera is the interesting device and the worst first one: it has no echo
# canceller (`rac status` says `aec: false`), its speaker is a 20 mm piezo on a
# ceiling mount, and its mic hears the space's music.  A laptop or phone has a real
# browser with a real AEC, a real speaker and a permission prompt, so the page is the
# path on which "is this any good?" can be answered honestly before anything is
# mounted in the room.  The page lives on `whisper.int.leighhack.org` because that is
# where the microphone capture, the level meter and the OpenWakeWord runtime already
# are (`crates/whisper-ws/`); it calls this box's gateway, which is why there is a
# `--allow-origin` list rather than a wildcard (see below).
#
# ## The measured budget, so the number is not a rumour
#
# Measured on services1, 2026-11-16, with these units' own binaries:
#
# | step | measured |
# | --- | --- |
# | 3.0 s of cam8 mic audio → transcript | **2.7 s** (whisper.cpp 1.9.5, `ggml-medium`, **on CPU** — whisper-server holds no `/dev/dri` fd, strata owns the iGPU) |
# | one-tool question → answer | **7.7 s** (`qwen3.8-flash-next-coder-iq1m` on the RTX 3060, 2 rounds, `space_status`) |
# | two-tool question → answer | **15.3 s** (3 s of speech, 2 tools) |
# | 31 words → 7.8 s of speech | **2.5 s** (piper, per-request process: startup dominates, synthesis is ~3× real time) |
# | end to end, spoken question → spoken answer | **~13 s** for a one-tool question |
#
# That is a hackspace toy, not an Alexa, and the two ways to make it faster are
# already in the tree rather than invented here: the *deterministic* paths
# (`triage::events_window` answers "what's on" with one HTTP GET and no model at all),
# and a smaller whisper model, which is a flag on aibox's unit.
#
# ## No secret is anywhere near this
#
# There is no `EnvironmentFile` and no sops entry here.  The gateway holds a whisper
# URL, an agent URL and a piper model path; the agent socket holds no token (that is
# `slack-agent.nix`'s argument, and this is its second client); the tools hold the Home
# Assistant token in a third process.  A voice assistant is a *microphone with an
# answer*, so the interesting security question is not a credential, it is who can
# ask — and that is the reach, below.
#
# ## Reach: loopback, and the one door in
#
# `voice-gateway` binds loopback and refuses any other address (its own module
# checks it, like `hackspace-mcp`).  A phone reaches it through the front door: the
# `voice.int.leighhack.org` row in `common/front-door.nix` (`upstream = "origin"`)
# and the origin vhost at the bottom of this file, which is the pair
# `machines/services1/int-dns.nix` requires to exist together (AGENTS rules 14 and
# 24).  `voice-satellite` listens on nothing at all: it writes a status file the
# gateway serves.
#
# Because the page is on one origin and the API on another, CORS is a policy and not
# a formality: `--allow-origin` echoes back *only* an exact match.  `*` here would
# mean any web page any member opens can ask the space's questions and read the
# answers, which is a small privacy leak and a free LLM.
#
# ## The camera speaker is off, and switching it on is a decision with a cost
#
# `infra.voiceAssistant.speaker = "off"` is the default and it is not shyness: the
# answers are synthesised, counted and written to the state directory, and nothing
# reaches the room.  Two things have to be true before that flips, and neither is a
# line of code:
#
# 1. **Someone has to have heard it.** The software path is measured (a 350 ms tone,
#    accepted by `rac play` and reported as played on 2026-11-16), audibility through a
#    C110's piezo across a working room is not.
# 2. **The unit needs a key it is allowed to read.** Playing means
#    `ssh root@cam8 'rac play - -r 16000'`, and the fleet's hop key lives in
#    `~/.ssh/agent-hop-key` of the *operator* accounts — a `DynamicUser` cannot read
#    it, and copying a private key into the Nix store is exactly what
#    `infra.buildTimeSecrets` exists to make expensive.  So `--speaker ssh` is wired
#    and unit-tested but no unit in this file uses it: the honest options are a
#    dedicated `voice-speaker` account whose *public* key is installed on the camera
#    (a device write, in a window, and thingino keeps `/etc` on the overlay) or a
#    systemd-managed agent holding the hop key.  Both are open in
#    `docs/follow-ups.md`, and neither is a reason to leave a room silent today:
#    the page already answers out loud.
#
# `cameras/README.md` owns what cam8 is; `crates/voice-assistant/README.md` owns the
# pipeline's design and its traps (the missing AEC, the `max_clients = 2` RTSP budget,
# the half-duplex rule, and the Goertzel factor-of-two that made the gate deaf).
{
  pkgs,
  lib,
  config,
  CRANE,
  INFRA,
  ...
}: let
  cfg = config.infra.voiceAssistant;
  HOSTS = import ../../../common/hosts.nix;
  mkOriginVhost = import ../../../common/nginx/origin-vhost.nix {inherit lib;};

  # The voices we will ship, pinned by hash.  nixpkgs packages the engine
  # (`piper-tts`, 1.8.0 at this lock) and no voices at all — the same gap
  # `crates/whisper-ws/package.nix` fills for the OpenWakeWord models, and the same
  # reason: they are data, upstreamed elsewhere, fetched by URL.
  #
  # piper insists on `<model>.onnx.json` sitting beside `<model>.onnx` (measured:
  # `FileNotFoundError: …/en_GB-alba-medium.onnx.json`), so the two are one store path.
  voices = {
    "en_GB-alba-medium" = {
      onnx = {
        url = "https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/en/en_GB/alba/medium/en_GB-alba-medium.onnx";
        sha256 = "sha256:QBNpxKgdCf3YbDLFyGRECBHb3MZkZs3i1k9xM6Zq0Ds=";
      };
      config = {
        url = "https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/en/en_GB/alba/medium/en_GB-alba-medium.onnx.json";
        sha256 = "sha256:qpZaLwLsztYywmlOH8crv/bWXyZfq1Z8qUWRjHPdifQ=";
      };
      note = "British English, male, medium quality, 22050 Hz. The space's own default.";
    };
  };

  voice = voices.${cfg.voice};

  voiceDir = pkgs.linkFarm "piper-voice" [
    {
      name = "${cfg.voice}.onnx";
      path = pkgs.fetchurl voice.onnx;
    }
    {
      name = "${cfg.voice}.onnx.json";
      path = pkgs.fetchurl voice.config;
    }
  ];

  voiceAssistant = CRANE.crate {
    pname = "voice-assistant";
    version = "0.1.0";
    member = "voice-assistant";
    # Transitive and hand-maintained: the four `common-rs/` crates this crate's
    # manifest names, none of which depend on the others (AGENTS rule 2's warning, and
    # the build-time-only failure recorded in `docs/slack-agent-2026-11-16.md` §13).
    sharedCrates = [
      "json"
      "http"
      "web"
      "service"
    ];
  };

  STATE = "voice-assistant";
  whisperUrl = "http://${HOSTS.aibox}:8082";
  agentUrl = "http://${cfg.agentListen}";

  commonFlags = [
    "--state-dir"
    "/var/lib/${STATE}"
    # Quoted, and the quotes are load-bearing rather than cosmetic: the default list
    # contains **phrases with spaces** (`hey jarvis`), systemd splits `ExecStart` on
    # whitespace, and unquoted this renders as `--wake-words hey jarvis,jarvis,...` —
    # the flag's value becomes `hey`, the rest becomes a positional argument, and the
    # unit starts happily with a wake word of `hey`.  Read back from the rendered unit
    # per rule 10, not believed from here.
    "--wake-words"
    ''"${lib.concatStringsSep "," cfg.wakeWords}"''
    "--max-speech-chars"
    (toString cfg.maxSpeechChars)
  ];

  gatewayFlags =
    commonFlags
    ++ [
      "--listen"
      cfg.listen
      "--whisper-url"
      whisperUrl
      "--whisper-language"
      cfg.whisperLanguage
      "--agent-url"
      agentUrl
      "--piper-bin"
      "${pkgs.piper-tts}/bin/piper"
      "--piper-model"
      "${voiceDir}/${cfg.voice}.onnx"
      "--length-scale"
      (toString cfg.lengthScale)
      "--allow-origin"
      (lib.concatStringsSep "," cfg.allowOrigins)
    ]
    ++ lib.optional (!cfg.recordQuestions) "--no-record-questions";

  satelliteFlags =
    commonFlags
    ++ [
      "--gateway"
      "http://${cfg.listen}"
      "--ffmpeg"
      "${pkgs.ffmpeg_7}/bin/ffmpeg"
      "--source"
      cfg.source
      "--speaker"
      cfg.speaker
    ]
    ++ lib.optionals (cfg.speaker == "ssh") [
      "--speaker-host"
      cfg.speakerHost
      "--speaker-user"
      cfg.speakerUser
      # Only ever read by `ssh -i`, and only when the speaker is on.  See the header:
      # nothing in this tree currently makes this path reachable.
      "--ssh-key"
      cfg.sshKey
    ];

  # The same hardening as the bot's own units: nothing here writes outside its state
  # directory, so nothing here needs a writable system, a home, or a core dump.
  hardening = {
    DynamicUser = true;
    StateDirectory = STATE;
    ProtectSystem = "strict";
    ProtectHome = true;
    NoNewPrivileges = true;
    PrivateTmp = true;
    LimitCORE = 0;
  };
in {
  options.infra.voiceAssistant = {
    enable = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Run the voice gateway and the camera satellite.";
    };

    listen = lib.mkOption {
      type = lib.types.str;
      default = "127.0.0.1:8193";
      description = ''
        The gateway's bind, loopback only.  The satellite builds its `--gateway` from
        this, and the origin vhost below proxies to it, so the port is stated once.
      '';
    };

    agentListen = lib.mkOption {
      type = lib.types.str;
      default = config.infra.slackAgent.ask.listen;
      defaultText = lib.literalExpression "config.infra.slackAgent.ask.listen";
      description = "Where the bot answers. Read from `slack-agent.nix`, never copied.";
    };

    voice = lib.mkOption {
      type = lib.types.enum (lib.attrNames voices);
      default = "en_GB-alba-medium";
      description = "Which piper voice to speak, from the pinned table above.";
    };

    whisperLanguage = lib.mkOption {
      type = lib.types.str;
      default = "en";
      description = "''auto'' lets whisper decide; a name saves it guessing on a short window.";
    };

    lengthScale = lib.mkOption {
      type = lib.types.float;
      default = 1.05;
      description = ''
        piper's tempo, > 1 is slower. 1.05 because the measured alternative is a room
        with a lathe running: 4 % slower speech is free and it is what made the numbers
        land when someone tested it aloud.
      '';
    };

    maxSpeechChars = lib.mkOption {
      type = lib.types.int;
      default = 320;
      description = ''
        Longest answer willing to be spoken. 320 characters is ~55 words and ~11 s of
        audio at the measured rate — already long for a ceiling speaker.  The model's
        own spoken budget is 200 tokens (`agent::MAX_SPEECH_TOKENS`); this is the
        backstop for when it ignores that (it has).
      '';
    };

    wakeWords = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [
        "hey jarvis"
        "jarvis"
        "hey hackspace"
        "hackspace"
      ];
      description = ''
        Whole-word phrases, matched on the transcript (`crates/voice-assistant/src/wake.rs`).
        Same list for the satellite's decision and the gateway's cleanup, so the page and
        the camera agree about what an address is.
      '';
    };

    allowOrigins = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = ["https://whisper.int.leighhack.org"];
      description = ''
        The only origins CORS will answer, echoed back exactly.  Adding a name here is
        the same kind of decision as adding a front-door row: it says which page may ask
        the space's questions from a browser.
      '';
    };

    recordQuestions = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Keep question and answer text in `/status.json`.  Same decision, same default,
        as `infra.slackAgent.ask.recordQuestions` and for the same reason: `whisper-ws`
        has published its last 20 transcriptions from the start, and this socket is no
        less private than that page.  `false` keeps counts and timings.
      '';
    };

    satellite.enable = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Run the camera end at all.  Off means the page works and nothing listens in the
        room — which is the right setting while the gate's false-probe rate is being
        measured with `just voice-probe`.
      '';
    };

    source = lib.mkOption {
      type = lib.types.str;
      default = "rtsp://127.0.0.1:8554/cam8";
      description = ''
        What ffmpeg reads, and deliberately **go2rtc's restream** rather than the camera:
        cam8's RTSP server allows two clients and frigate already holds one
        (`cameras/README.md`), so the microphone has to come from the copy that is
        already up here.
      '';
    };

    speaker = lib.mkOption {
      type = lib.types.enum [
        "off"
        "ssh"
      ];
      default = "off";
      description = ''
        Where the answer's audio goes.  `off` (the default) writes it to the state
        directory and says nothing out loud; see the header for the two things that must
        be true first.
      '';
    };

    speakerHost = lib.mkOption {
      type = lib.types.str;
      default = "cam8.int.leighhack.org";
      description = "The camera to push audio to when `speaker = \"ssh\"`.";
    };

    speakerUser = lib.mkOption {
      type = lib.types.str;
      default = "root";
      description = "The account on the camera that owns `rac`.";
    };

    sshKey = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/private/voice-assistant/ssh/id_ed25519";
      description = ''
        A key the unit may read, for `speaker = "ssh"`.  Nothing provisions it: see the
        header, and leave the speaker off until one of the two honest options in
        `docs/follow-ups.md` has been chosen.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    # The two verbs that are tools rather than services — `probe` and `gate-check`, both
    # of which exist to answer a question about a room a person is standing in — have to
    # be *on the box*, and `just voice-probe` says so out loud.  The unit already drags
    # this exact derivation in, so this costs no closure, only a PATH entry; ffmpeg comes
    # along for the same reason (the unit names it by store path, an operator typing
    # `voice-assistant probe --source …` does not).
    environment.systemPackages = [
      voiceAssistant
      pkgs.ffmpeg_7
    ];

    # The gateway: every caller's whisper, bot and voice in one loopback socket.
    systemd.services.voice-gateway = INFRA.mkNeverGiveUp {
      description = "Hackspace voice gateway (whisper + agent + piper, loopback)";
      wantedBy = ["multi-user.target"];
      after = [
        "network-online.target"
        "slack-agent-listen.service"
      ];
      # `wants`, not `requires`: with the bot's socket down the gateway still answers
      # /v1/transcribe and /v1/tts, which is enough for the page to show a transcript
      # and for `just voice-say` to work.  A voice assistant that answers with nothing
      # at all is worse than one that answers in text.
      wants = [
        "network-online.target"
        "slack-agent-listen.service"
      ];
      serviceConfig =
        hardening
        // {
          ExecStart = "${voiceAssistant}/bin/voice-assistant serve " + lib.concatStringsSep " " gatewayFlags;
        };
    };

    # The camera end.  No port, no certificate, no user input: it reads the restream,
    # cuts utterances, and asks the gateway the same two questions the page does.
    systemd.services.voice-satellite = INFRA.mkNeverGiveUp {
      description = "Hackspace voice satellite (cam8 mic in, answers out — speaker off)";
      wantedBy = ["multi-user.target"];
      after = [
        "network-online.target"
        "voice-gateway.service"
        # go2rtc lives inside frigate's container, so the restream this reads only
        # exists once that container is up.  `wants` rather than `requires`: frigate
        # restarting must not take the satellite down with it — the ffmpeg child dies,
        # the never-give-up loop starts another one five seconds later, and that is the
        # whole recovery.
        "podman-frigate.service"
      ];
      wants = [
        "network-online.target"
        "voice-gateway.service"
        "podman-frigate.service"
      ];
      serviceConfig =
        hardening
        // {
          ExecStart = "${voiceAssistant}/bin/voice-assistant satellite " + lib.concatStringsSep " " satelliteFlags;
        };
    };

    services.nginx.virtualHosts."voice.int.leighhack.org" = mkOriginVhost {
      proxyPass = "http://${cfg.listen}";
      # A turn is 8–150 s (the ask endpoint's own timeout is 150 s and the agent's
      # is 120 s).  nginx's 60 s read timeout would cut a slow model off mid-turn
      # and the page would report a network error rather than a slow answer.
      extraConfig = ''
        proxy_connect_timeout 10s;
        proxy_send_timeout 300s;
        proxy_read_timeout 300s;
        # **900k, and the number is a boundary in the right order.** Every server in this
        # fleet shares `common_web::MAX_BODY_BYTES = 1 MiB`, and a body over it is not a
        # 413 — `read_request` returns `None` and the connection closes with **no response**,
        # so a browser reports a network failure and the page says "cannot reach the
        # assistant" about a recording that was merely long.  Refusing here means the door
        # answers *first*, with a status a client can show, and the app's silent drop stays
        # what a loopback caller does rather than what a member's phone experiences.
        # 16 kHz mono PCM16 is 32 KB/s, so 900k is ~28 s of speech; the page's own
        # push-to-talk cap is 15 s (480 KB).
        client_max_body_size 900k;
      '';
    };
  };
}
