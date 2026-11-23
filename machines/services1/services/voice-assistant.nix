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

  # The wake model, pinned the same way the piper voices are: data, upstreamed
  # elsewhere, fetched by URL and hashed (AGENTS rule 30's shape — this repo keeps the
  # pin, not the artifact).
  #
  # Release `v2.1.3` of livekit/livekit-wakeword publishes exactly two phrases, and
  # `hey jarvis` is the one this fleet already uses as a wake word, so the keyword list in
  # `cfg.wakeWords` and the engine agree without anyone configuring a model name.  Only
  # the **streaming** export loads (the crate refuses v1/openWakeWord classifier files with
  # `NotStreamingModel`); `crates/voice-wake/README.md` keeps the sha256 of every asset in
  # that release.
  # `name` is not enough, and this is the reason: the crate keys a model by its **file
  # stem**, and every store path's basename carries its own hash — so `--model
  # /nix/store/h56ybx…-hey_jarvis.onnx` announced the model as
  # `h56ybx10i07v2c7pcnd6hsg1dw3zdl6w-hey_jarvis`. That name is what a native Wyoming client
  # would put in `detect {names: […]}` and what the page shows, so it gets a clean one here:
  # a linkFarm gives the file a path whose stem is the phrase, exactly as `voiceDir` does for
  # piper beside it.
  wakeModel = pkgs.linkFarm "wake-models" [
    {
      name = "hey_jarvis.onnx";
      path = pkgs.fetchurl {
        name = "hey_jarvis.onnx";
        url = "https://github.com/livekit/livekit-wakeword/releases/download/v2.1.3/hey_jarvis.onnx";
        sha256 = "sha256:i1VMUICYiKwU9n+iGsw72F5Y9WeO5PK+XQBB+aLmKEM=";
      };
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
      # The rolling-transcription slice, in seconds. The binary's default is the same number on
      # purpose — this line exists so the value is visible in the rendered unit (rule 10) rather
      # than discovered in a Rust default, and so the dial can be moved per box without a rebuild.
      "--asr-slice-secs"
      (toString cfg.asrSliceSecs)
    ]
    ++ lib.optional (!cfg.recordQuestions) "--no-record-questions"
    # The name says *engine* deliberately: this is the prefill of the prompt prefix into the
    # inference server, not the warm piper, which is started unconditionally by `serve` and stays
    # up whatever this says.
    ++ lib.optional (!cfg.warmEngine) "--no-engine-warm"
    # The engine is a proxy target, not a second origin: the page keeps exactly one address
    # to talk to, and the engine keeps its loopback-only rule.
    ++ lib.optionals cfg.wake.enable [
      "--wake-url"
      "http://127.0.0.1:${toString cfg.wake.listen}"
    ];

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
      # The satellite transcribes an open window while the speaker is still talking, so the same
      # dial belongs here as on the gateway; `--no-engine-warm` likewise means the satellite does
      # not fire a prefill from the middle of an utterance.
      "--asr-slice-secs"
      (toString cfg.asrSliceSecs)
    ]
    ++ lib.optional (!cfg.warmEngine) "--no-engine-warm"
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

    asrSliceSecs = lib.mkOption {
      type = lib.types.float;
      default = 1.2;
      description = ''
        Seconds of audio per rolling-transcription slice, on the gateway and on the satellite.
        Shorter means less left to transcribe when someone stops talking — and one more whisper
        call per sentence, against a whisper that is already about real-time on aibox's CPU. The
        number is measured rather than chosen: 1.2 s is roughly one clause, and the slices
        overlap so a word cut in half is still heard whole (`crates/voice-assistant/src/asr.rs`).
      '';
    };

    warmEngine = lib.mkOption {
      type = lib.types.bool;
      # **Off, because the box said so.** Measured on services1 the first time it was switched
      # on: a warm-up of this fleet's 4 079-token spoken prompt cost **4 629 ms** of the RTX 3060
      # and reported `cached 0` — the same prompt-prefix work the real turn was about to pay
      # for, done twice, and (before the listen socket learned to wait for a warm-up's lock) it
      # failed that turn outright with `another turn is in flight`. A warm-up is worth exactly
      # one thing: fired while **speech is still arriving**, so the engine prefills during
      # seconds nobody is listening. That is what the code now does
      # (`turn.rs::warm_skip_reason` refuses to warm a question that arrived as text), but the
      # speech left after a first partial is ~1 s and a cold prefill here is ~4.6 s, so on this
      # engine it is still a net loss. Read the counters before turning it on: `prefills`
      # climbing while `prefill_cached_tokens` stays at 0 is the sound of paying for nothing,
      # and `ask_waited_for_prefill` / `ask_wait_ms` say how often a warm-up held up a turn.
      # Does **not** affect the warm piper, which `serve` starts unconditionally.
      default = false;
      description = ''
        Fire a `max_tokens: 1` prefill of the question's prompt prefix at the inference engine
        while speech is still arriving, so the engine is warm with this conversation before the
        real request lands. Costs one extra request per utterance and, on this engine today,
        more seconds than it saves — see the measurement above, the prefill section of
        `docs/slack-agent-2026-11-16.md` §17, and `crates/slack-agent/src/engine.rs` for why
        `max_tokens: 1` and not `0`.
      '';
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

    wake.enable = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Run `voice-wake` and expose `/v1/wake/*` through the gateway: the server-side half
        of the wake tier, for a device that cannot run a detector itself (a browser tab, any
        dumb PCM stream) and so has to hand its audio somewhere.

        Detection only.  Nothing here answers, plays, or transcribes — a detection is a
        number the client decides what to do with — and the page must be switched to
        `server` wake by hand, so enabling this changes nothing that is already running.
      '';
    };

    wake.threshold = lib.mkOption {
      # `types.floats` does not exist at this pin (the error is literally
      # "attribute 'floats' missing … did you mean float?"); the clamped numeric type is
      # under `numbers`, and clamping matters here because a threshold of 0.0 would fire a
      # "detection" on every chunk and 2.0 would be silently dead.
      type = lib.types.numbers.between 0.0 1.0;
      default = 0.5;
      description = ''
        Score at or above which the engine reports a detection.  0.5 is upstream's default
        and **not** a tuned number for this room: their own figure is 0.5–2.5 false accepts
        per hour of conversational speech, and `docs/follow-ups.md` has the busy-hour
        measurement that should replace it.  Raising it trades recall for quiet.
      '';
    };

    wake.listen = lib.mkOption {
      type = lib.types.port;
      default = 8195;
      description = ''
        Loopback port the engine binds.  It is reachable from off-box only through the
        gateway's `/v1/wake/*` proxy and the front door's `voice.int` row, because the
        binary refuses any non-loopback bind (`voice-wake` has no authentication).
      '';
    };

    wake.record = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        Write what each wake session was fed to `wake.recordDir`, so a detection score has bytes
        behind it and a threshold becomes arguable instead of inherited.

        **Off by default because it records the room.** Not a hedge: audio is only written while a
        page has deliberately armed server-side wake, so it is someone choosing to hand the server
        their microphone — but it is still a recording of a room with people in it. Files are 0600
        and root-owned, one session is capped at 600 s by the engine, and `wake.recordAge` is the
        backstop for the measurement window nobody closed. Turn it on to measure, turn it off
        after; `just voice-wake-recs` lists what it caught.
      '';
    };

    wake.recordDir = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/${STATE}/wake-record";
      description = "Where the recordings go: the unit's own state directory, never the store.";
    };

    wake.recordAge = lib.mkOption {
      type = lib.types.str;
      default = "14d";
      description = ''
        tmpfiles age for the recordings.  The engine writes a WAV when a session closes or is
        reaped and never looks at it again, so retention is the system's job — the same reason
        `wav::prune_wavs` exists for the answer side, and the same fault it prevents: a state
        directory that grows on a box holding the space's databases.
      '';
    };

    wake.maxSessions = lib.mkOption {
      type = lib.types.ints.positive;
      default = 16;
      description = ''
        Concurrent streams.  One 7 MB model instance and its own inference each, so this is
        a memory and fairness cap rather than a capacity claim — measured cost is ~31
        streams per core single-threaded on the nix build (`crates/voice-wake/README.md`),
        and scoring is serialised across sessions in one mutex on purpose.
      '';
    };

    wake.engines = lib.mkOption {
      type = with lib.types; listOf (enum ["livekit" "oww"]);
      default = ["livekit"];
      apply = es: (lib.unique (lib.sort (a: b: a < b) es)) ++ lib.optionals (!lib.elem "livekit" es) ["livekit"];
      description = ''
        Which detectors the engine loads and therefore what a session may ask for.  The first
        element is the default for a session that names none; "livekit" is always present,
        because it is the only engine the page and `voice-assistant` have ever been scored by and
        a session that names nothing must never fail.

        **`oww` is offered, not recommended.**  It is the browser's model, run from the browser's
        own bytes — see `wake.owwAssets` — under my Rust port, and the port **scores ~0 on audio
        the browser detects confidently**: 1e-5 and decaying, on a clip the LiveKit model reads at
        0.626.  The exclusion list is long (identical bytes, dtype, scale, normalisation, window,
        step, history order, and a sweep proving the keyword graph's window is 16), so what is
        left is a tract-vs-ONNXRuntime numerical difference, which is the open item in
        `docs/follow-ups.md` together with the option of running the reference implementation
        instead.  Enable it to **measure the gap on identical bytes with `"engine":"both""**, never
        to drive a microphone: an always-no ear is worse than a mediocre one, and it is
        indistinguishable from silence.

        Cost if both are on: openwakeword is ~287 ms of CPU per audio-second against LiveKit's
        ~32 ms (`crates/voice-wake/README.md`), so a `both` session pays about ten times what a
        single-engine one does.  That is the price of an A/B that is not confounded by the room
        changing between the two runs, and it is affordable for a measurement, not for the fleet.
      '';
    };

    wake.owwAssets = lib.mkOption {
      # `str`, not `path`: it is a command-line argument, and a `types.path` default would make
      # this option a store path at eval time even when no engine is asked to load it.
      type = lib.types.str;
      default = "${pkgs.whisper-ws.passthru.openwakewordAssets}/models";
      defaultText =
        lib.literalExpression
        ''"''${pkgs.whisper-ws.passthru.openwakewordAssets}/models"'';
      description = ''
        The openwakeword graphs: `melspectrogram.onnx`, `embedding_model.onnx`, the keyword head.
        This is **the same store path the page serves**, so the server and the browser cannot be
        comparing two different files — the single source of truth is the pinned fetch in
        `crates/whisper-ws/package.nix`, and a second copy here would be a hash table waiting to
        become a fiction.
      '';
    };

    wake.owwKeyword = lib.mkOption {
      type = lib.types.str;
      default = "hey_jarvis_v0.1";
      description = ''
        Which keyword head to run.  Note the name: the browser ships `hey_jarvis_v0.1`, the
        LiveKit model file is `hey_jarvis.onnx`, and microWakeWord on an ESP32 calls the same
        phrase `hey_jarvis` again — three pins, one phrase, and scores that are not comparable
        across them (`docs/esp32-voice-satellites.md`).
      '';
    };

    wake.owwThreshold = lib.mkOption {
      type = lib.types.float;
      default = 0.5;
      description = ''
        Threshold for the openwakeword ear, deliberately separate from `wake.threshold`: the two
        detectors were trained separately on the same phrase, so the same number means different
        things in each, and one dial would mean a session on one engine judged by the other's
        scale.  Nothing is tuned here yet — upstream's figure, and `crates/voice-wake/README.md`
        states why 0.5 is a starting point rather than a measurement.
      '';
    };

    wake.idleSecs = lib.mkOption {
      type = lib.types.ints.positive;
      default = 120;
      description = ''
        Reap a session that has not been fed for this long, freeing one of the `wake.maxSessions`
        slots.  It is not a keep-alive of a live listener: a browser tab deliberately stops
        streaming while a take is in flight (the ear must not hear the command it is about to
        transcribe), and a slow answer can outlast the timeout — measured 2026-11-17, where the
        reap of a tab that was still armed looked to the user exactly like an engine that had
        locked up, because the page then disarmed itself and could not be re-armed from the
        select.  So this must comfortably exceed one turn of conversation (the agent gives up at
        150 s), and the page now re-arms transparently if a reap does happen
        (`recoverWakeSession` in the whisper page) — the two changes are one fix, and either
        alone leaves a visible fault.
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
    # The recorder's backstop: type `e` creates the directory and purges entries older than the
    # age.  Needed because the engine writes and forgets — see `wake.recordAge`.
    #
    # Mode only, never an owner, and that is a measurement rather than a style choice: the
    # engine that writes here runs as a **DynamicUser**, so naming `root root` in a tmpfiles
    # rule is a race the engine can lose — the first boot where tmpfiles wins chowns the
    # recordings away from the writer and recording then fails quietly, one journal line for
    # a whole window of silence.  (Written the day it was found: the directory did not exist
    # at all after the switch, because nothing had been recorded yet and `e` had not created
    # it either — the engine creates it on first write, as its own user.)
    #
    # Real retention belongs inside `dump()` beside the writer, mirroring `wav::prune_wavs`,
    # so one program owns the files it creates; that is the open item in `docs/follow-ups.md`
    # and this rule is the stopgap, not the mechanism.
    systemd.tmpfiles.rules = lib.optionals cfg.wake.record [
      "e ${cfg.wake.recordDir} 0700 - - ${cfg.wake.recordAge}"
    ];
    # The two verbs that are tools rather than services — `probe` and `gate-check`, both
    # of which exist to answer a question about a room a person is standing in — have to
    # be *on the box*, and `just voice-probe` says so out loud.  The unit already drags
    # this exact derivation in, so this costs no closure, only a PATH entry; ffmpeg comes
    # along for the same reason (the unit names it by store path, an operator typing
    # `voice-assistant probe --source …` does not).
    environment.systemPackages = [
      voiceAssistant
      pkgs.ffmpeg_7
      # On PATH so the measurement harness is usable from the box it runs on:
      # `voice-wake bench hey_jarvis.onnx room.wav` over a recording is how the busy-hour
      # false-accept rate in `docs/follow-ups.md` gets earned.
      pkgs.voice-wake
    ];

    # The ear.  Detection only: it scores PCM and says yes or no, and nothing downstream
    # is wired to it yet — the page asks for it, the satellite does not.
    #
    # No secrets, and no StateDirectory unless `wake.record` is on: it holds one model in memory
    # and a session table, so a restart costs a client one re-POST (/v1/wake/session) and, with
    # recording off, nothing on disk.  When recording is on the files land in the gateway's own
    # state directory, root-owned, 0600 per file, aged out by tmpfiles below.
    systemd.services.voice-wake = lib.mkIf cfg.wake.enable (INFRA.mkNeverGiveUp {
      description = "Hackspace wake-word engine (streaming detection, loopback)";
      wantedBy = ["multi-user.target"];
      after = ["network-online.target"];
      wants = ["network-online.target"];
      serviceConfig =
        hardening
        // {
          ExecStart =
            "${pkgs.voice-wake}/bin/voice-wake serve "
            + lib.concatStringsSep " " ([
                "--model"
                "${wakeModel}/hey_jarvis.onnx"
                "--listen"
                "127.0.0.1:${toString cfg.wake.listen}"
                "--threshold"
                (toString cfg.wake.threshold)
                "--max-sessions"
                (toString cfg.wake.maxSessions)
                "--idle"
                (toString cfg.wake.idleSecs)
                # Explicit, so a session that names no engine has a value in the unit to read
                # rather than a default buried in a binary that a future reader cannot see.
                "--engine"
                (builtins.head cfg.wake.engines)
              ]
              # Opt-in, and absent when off: a unit that never writes room audio and one that does
              # must not be one line apart by accident.
              ++ lib.optionals cfg.wake.record ["--record-dir" cfg.wake.recordDir]
              # Only when a second ear is actually offered: an engine pointed at an empty model
              # directory is a boot failure, and a unit that always names the assets would make
              # every deploy load graphs nothing asked for.
              ++ lib.optionals (lib.elem "oww" cfg.wake.engines) [
                "--oww-models"
                cfg.wake.owwAssets
                "--oww-keyword"
                cfg.wake.owwKeyword
                "--oww-threshold"
                (toString cfg.wake.owwThreshold)
              ]);
        };
    });

    # The gateway: every caller's whisper, bot and voice in one loopback socket.
    systemd.services.voice-gateway = INFRA.mkNeverGiveUp {
      description = "Hackspace voice gateway (whisper + agent + piper, loopback)";
      wantedBy = ["multi-user.target"];
      after =
        [
          "network-online.target"
          "slack-agent-listen.service"
        ]
        ++ lib.optionals cfg.wake.enable ["voice-wake.service"];
      # `wants`, not `requires`: with the bot's socket down the gateway still answers
      # /v1/transcribe and /v1/tts, which is enough for the page to show a transcript
      # and for `just voice-say` to work.  A voice assistant that answers with nothing
      # at all is worse than one that answers in text.
      wants =
        [
          "network-online.target"
          "slack-agent-listen.service"
        ]
        ++ lib.optionals cfg.wake.enable ["voice-wake.service"];
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
