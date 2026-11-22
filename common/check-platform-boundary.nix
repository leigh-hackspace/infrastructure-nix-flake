# CHECK — plain-eval assertion file: `nix eval --impure --raw -f` prints ok or throws; run by `just check` / `just check-eval` and by flake.nix's `checks`.
# check-platform-boundary.nix
#
# Assertions over the **device boundary** in `crates/voice-assistant/`: that the turn logic
# still knows nothing about what kind of hardware it is attached to, and that everything which
# does know lives in `src/drivers/`.
#
#   nix eval --impure --raw -f common/check-platform-boundary.nix
#
# ## Why this is a check and not a comment
#
# `crates/voice-assistant` is designed as a core (gateway, gate, wake-word rule, the half-duplex
# arithmetic) plus per-platform adapters (`crate::source`, `crate::sink`), on the premise that
# the next microphone — a different camera, an ESP32, a phone app, a browser tab — should be a
# new file in `drivers/` and a row in the unit, never an edit to the turn logic. That premise is
# exactly the kind that survives one refactor and dies in the next one, because reaching for
# `opts.source.starts_with("rtsp://")` from the loop is *easier* than writing the adapter. The
# boundary therefore has to be as measurable as the format. `AGENTS.md` rule 24's guards are the
# precedent: a rule two docs can disagree about is a rule nobody keeps.
#
# ## What is checked, and what deliberately is not
#
#   1. no **device fact** — a host, a URL, a tool's shell syntax, a vendor — appears in a core
#      file. These are the strings someone would have to change to add a platform, which is the
#      whole question;
#   2. the core **never spawns a process**. A source or a sink is a trait precisely so that the
#      `Command`/`Stdio` plumbing has one home; the documented exception is `tts.rs`, which
#      spawns the *voice engine* — a pinned service dependency of the answer path, not a device,
#      and named as an exception here rather than left to be noticed;
#   3. the drivers still **exist and still know** what they are for. Without this the check
#      would pass by deleting the adapters and taking the device facts somewhere the check is not
#      looking, which is the failure mode every "no X here" guard has.
#
# What it does not check, honestly: **flag names**. `--ffmpeg`, `--ssh-key` and `--speaker-host`
# still appear in `main.rs`, because they are the deployed CLI contract that `voice-assistant`'s
# `ExecStart` passes (`AGENTS.md` rule 10) and renaming them is a deploy, not a refactor. So a
# file that mentioned a device only inside a flag name would pass. That is the same limitation
# `check-doc-pointers.nix` states about backticks: the one way this guard can be lied to, written
# down rather than discovered later.
let
  root = ../.;

  # The files the streaming turn added are in here because they *are* the turn logic now:
  # `turn.rs` is the pipeline, `turn_routes.rs` is its HTTP surface, `events.rs` is the wire
  # contract, `asr.rs` slices the audio, `sentence.rs` decides what is speakable and `gateway.rs`
  # is the satellite's client of the gateway. A device fact would be equally at home in none of
  # them, which is what makes the check worth extending as the crate grows.
  #
  # `tts.rs` and `testkit.rs` are the two deliberate exclusions — one spawns the voice engine (the
  # exception asserted at the bottom of this file), the other is `#[cfg(test)]` and exists to stand
  # in for the processes the drivers own.
  CORE = [
    "crates/voice-assistant/src/api.rs"
    "crates/voice-assistant/src/turn.rs"
    "crates/voice-assistant/src/turn_routes.rs"
    "crates/voice-assistant/src/events.rs"
    "crates/voice-assistant/src/asr.rs"
    "crates/voice-assistant/src/gateway.rs"
    "crates/voice-assistant/src/sentence.rs"
    "crates/voice-assistant/src/satellite.rs"
    "crates/voice-assistant/src/gate.rs"
    "crates/voice-assistant/src/wav.rs"
    "crates/voice-assistant/src/wake.rs"
    "crates/voice-assistant/src/agent.rs"
    "crates/voice-assistant/src/stt.rs"
    "crates/voice-assistant/src/spoken.rs"
    "crates/voice-assistant/src/b64.rs"
    "crates/voice-assistant/src/source.rs"
    "crates/voice-assistant/src/sink.rs"
    "crates/voice-assistant/src/main.rs"
  ];

  # The strings that name a platform rather than a job.  Quoted deliberately: these appear in
  # URLs, log lines and shell arguments, and every one of them is something an adapter owns.
  DEVICE_FACTS = [
    "cam8"
    "rac play"
    "rtsp://"
    "go2rtc"
    "thingino"
    ":8554"
  ];

  # How a device actually gets driven, in this crate: a child process.  That belongs in
  # `drivers/` and nowhere else in the core.
  PROCESS_SPAWNS = [
    "Command::new"
    "Stdio::"
  ];

  read = rel: builtins.readFile (root + "/${rel}");
  has = sub: src: builtins.replaceStrings [sub] [""] src != src;

  need = cond: msg:
    if cond
    then true
    else builtins.throw "check-platform-boundary: ${msg}";

  facts = f: let
    src = read f;
  in
    builtins.filter (x: x != "") (
      (map (w:
        if has w src
        then "${f}: contains the device fact \"${w}\" — that belongs in src/drivers/"
        else "")
      DEVICE_FACTS)
      ++ (map (w:
        if has w src
        then "${f}: spawns a process (\"${w}\") — a source or a sink is a trait so this has one home"
        else "")
      PROCESS_SPAWNS)
    );

  findings = builtins.concatMap facts CORE;

  drivers = {
    mod = read "crates/voice-assistant/src/drivers/mod.rs";
    ingest = read "crates/voice-assistant/src/drivers/ffmpeg_stream.rs";
    noise = read "crates/voice-assistant/src/drivers/rac_ssh.rs";
    silent = read "crates/voice-assistant/src/drivers/play_nothing.rs";
  };

  # The traits both sides implement, so "the boundary exists" is checked rather than assumed.
  source = read "crates/voice-assistant/src/source.rs";
  sink = read "crates/voice-assistant/src/sink.rs";
in
  # `assert`, so the value this file evaluates to is the string `just check` prints — and every
  # failure still arrives as `need`'s own message with a file name in it.
  assert need (findings == [])
  ("the voice assistant's turn logic has learned what a device is — add an adapter to "
    + "crates/voice-assistant/src/drivers/ instead of teaching the core about a platform "
    + "(docs/esp32-voice-satellites.md is the contract for the next one):\n       "
    + builtins.concatStringsSep "\n       " findings)
  # The adapters must still be there, still be the only ones, and still know their job.
  && need (has "cam8" drivers.mod && has "rtsp://" drivers.mod)
  "drivers/mod.rs no longer holds the defaults, so where did they go?"
  && need (has "rtsp://" drivers.ingest)
  "drivers/ffmpeg_stream.rs no longer knows about RTSP; is the ingest adapter still real?"
  && need (has "rac play" drivers.noise)
  "drivers/rac_ssh.rs no longer names the command it runs — a sink that does not say what it does"
  && need (has "off" drivers.silent)
  "drivers/play_nothing.rs lost its label, and status.json's `speaker` value is an operator-facing string"
  # And the seam itself must still be a seam.
  && need (has "fn read" source && has "fn play" sink)
  "the source/sink traits are no longer both there — the boundary is the point of this check"
  # tts.rs is the named exception above; if it ever stops spawning anything, the exception has
  # to be retired from this file's header — and that is a finding, not a rumour.
  && need (has "Command::new" (read "crates/voice-assistant/src/tts.rs"))
  "tts.rs no longer spawns the voice engine, so this file's documented exception is stale"; "ok\n"
