# JUST — the voice assistant: what the gateway saw, one turn, nothing played out loud.
#
# Part of the justfile: the root justfile imports these, so `just` still
# resolves every recipe from the repo root and `just --list` still shows
# them.  Split by job (AGENTS.md rule 26), so no file here is over 1000
# lines, and the guard that enforces that rule now sees justfiles.

# --- the voice assistant: browser page, cam8 mic, spoken answers --------------------
#
# The gateway is loopback-only on services1, so every one of these asks it *through the
# front door* (`https://voice.int.leighhack.org`) — the same path a phone takes, which is
# the point: if it works here it works in the room.  No recipe here plays anything out
# loud: `voice-say` writes a WAV and stops.  Design and the measured latency budget:
# crates/voice-assistant/README.md.

# What the gateway, the mic and the last answers look like right now.
voice-wake-recs:
    # List what the wake recorder caught, newest first, with what each one is.  The recordings
    # only exist while `infra.voiceAssistant.wake.record` is on and a page has armed server wake,
    # so "nothing here" usually means the flag is off rather than that nobody spoke.
    sudo ls -lht /var/lib/voice-assistant/wake-record 2>/dev/null | head -20 \
      || echo "no recording directory — is infra.voiceAssistant.wake.record on?"

voice-status:
    #!/usr/bin/env bash
    set -euo pipefail
    curl -fsS -m 10 https://voice.int.leighhack.org/status.json | python3 -c '
    import json, sys
    s = json.load(sys.stdin)
    print("gateway: %d requests, %d transcribes, %d asks, %d tts, %d error(s) [%s]"
          % (s["requests"], s["transcribes"], s["asks"], s["tts"], s["errors"], s["last_error"] or "clean"))
    print("config:  whisper=%s agent=%s voice=%s@%d max=%d chars origins=%s record=%s"
          % (s["whisper"].split("//")[1], s["agent"], s["voice"]["name"], s["voice"]["sample_rate"],
             s["max_speech_chars"], ",".join(s["allow_origins"]), s["record_questions"]))
    for q in s.get("recent", [])[:6]:
        print("  %-9s %-14s %-46s %s" % (q.get("at", ""), q.get("route", ""),
                                         (q.get("detail") or "")[:46],
                                         ("ok %.1fs" % q["secs"]) if q.get("ok") else ("FAIL %.1fs" % q["secs"])))
    m = s.get("satellite")
    if not m:
        print("mic:     no satellite reporting (infra.voiceAssistant.satellite.enable?)")
    else:
        print("mic:     %s  source=%s  up %.0fs" % (m["speaker"], m["source"], m["uptime_secs"]))
        print("         floor %.0f dBFS  threshold %.0f  loudest %.0f  speech %d/%.0fs"
              % (m["floor_dbfs"], m["threshold_dbfs"], m["peak_dbfs"], m["speech_frames"], m["captured_secs"]))
        gap = m["threshold_dbfs"] - m["peak_dbfs"]
        if m["windows"] == 0 and m["speech_frames"] == 0:
            print("         -> nothing has been loud enough: %.0f dB below threshold (quiet room, or raise the level)" % gap)
        elif m["windows"] == 0:
            print("         -> %d loud frames never made a window: run gate-check on the clip" % m["speech_frames"])
        else:
            print("         -> %d window(s), %d ignored, %d asked, %d answered, %d unplayed(speaker), %d ffmpeg restart(s)"
                  % (m["windows"], m["ignored"], m["asked"], m["answers"], m["unplayed"],
                     # Named for the event, not the tool, since the ingest is a trait now.  Read
                     # both spellings: this verb runs against whichever generation is deployed,
                     # and a KeyError here loses the whole status render for want of one line.
                     m.get("ingest_restarts", m.get("ffmpeg_restarts", 0))))
        if m["last_transcript"]:
            print("         last: %r -> %s" % (m["last_transcript"][:60], m["last_action"]))
        if m["last_error"]:
            print("         last error: %s" % m["last_error"])
    '

# One question, spoken-answer included, and no audio path at all: prints the text, the
# timing, the tool calls, and writes the speech to a file instead of playing it.
voice-ask question:
    #!/usr/bin/env bash
    set -euo pipefail
    # stdin, for the reason every other recipe here gives: just has no quoting builtin at
    # this pin and "what's on tonight?" loses its apostrophe through two shell levels.  A
    # question is prose, so it goes through stdin into a python that *encodes* it: the JSON
    # string needs its apostrophes and any quotes inside it escaped by something that
    # understands JSON rather than by a shell.
    cat <<'VO_ASK' | python3 -c 'import json,sys; sys.stdout.write(json.dumps({"text": sys.stdin.read().strip()}))' \
        | curl -fsS -m 180 -X POST -H 'content-type: application/json' \
            --data-binary @- https://voice.int.leighhack.org/v1/ask > /tmp/voice-ask.json
    {{question}}
    VO_ASK
    # A question this cheap is worth the two lines of output it costs: the same turn is
    # 8-150 s depending on whether the model called tools, and a number is the only way to
    # know which one you just waited for.
    python3 -c 'import json,base64; a=json.load(open("/tmp/voice-ask.json"))
    print("answer: " + a["answer"])
    print("speech: " + a.get("speech","(nothing to say)"))
    if a.get("audio"):
        open("/tmp/voice-answer.wav","wb").write(base64.b64decode(a["audio"]))
        print("wrote:  /tmp/voice-answer.wav (%.1f s) — nothing has played" % a.get("audio_secs",0))
    print("[%.1fs total · agent %.1fs · %s · %d round(s) · tools: %s]" % (a["secs"], a.get("agent_secs",0), a.get("engine","-"), a.get("rounds",0), ", ".join(a.get("tools") or []) or "none"))
    for w in a.get("warnings") or []: print("WARN: " + w)
    '

# One question, answered **as a stream**: every event printed with the moment it arrived, so the
# claim of the streaming pipeline is measurable from a shell rather than argued from the code.
# The number to look at is `first audio` against `last token`: audio before the last token means
# the room heard the answer while the model was still producing it, which is the whole change.
# Writes the audio to /tmp/voice-turn.wav and plays nothing.
voice-turn question:
    #!/usr/bin/env bash
    set -euo pipefail
    # The question is prose, so it goes through a heredoc into a python that encodes it, exactly
    # as `voice-ask` does. It lands in a variable rather than straight into the pipeline because
    # the pipeline's last stage is the timed reader, and bash wants a heredoc's body on the lines
    # after the logical line that opens it.
    body=$(cat <<'VO_TURN_Q' | python3 -c 'import json,sys; sys.stdout.write(json.dumps(dict(text=sys.stdin.read().strip(), voice=True)))'
    {{question}}
    VO_TURN_Q
    )
    # Every line of the reader below is indented with the recipe, and just strips that indent
    # before handing the script to bash — a python at column zero here would end the recipe body
    # and be parsed as just syntax, which is a confusing way to discover it.
    if ! printf '%s' "$body" | curl -fsS -N -m 180 -X POST \
            -H 'content-type: application/json' -H 'accept: text/event-stream' \
            --data-binary @- https://voice.int.leighhack.org/v1/turn \
        | python3 -c '
    import base64, json, sys, time, wave
    t0 = time.monotonic()
    first, raw = dict(), b""
    name, data = None, []
    answer, engine = "", "-"
    def emit(ms, what, line):
        if what not in first:
            first[what] = ms
        print("  %7.0f ms  %-8s %s" % (ms, what, line[:78]))
    for line in sys.stdin:
        line = line.rstrip("\r\n")
        if line:
            if line.startswith("event:"):
                name = line[6:].strip()
            elif line.startswith("data:"):
                data.append(line[5:].strip())
            continue
        if name is None:
            continue
        blob = "\n".join(data)
        data = []
        v = json.loads(blob) if blob else dict()
        ms = (time.monotonic() - t0) * 1000.0
        if name == "token":
            emit(ms, name, repr(v.get("t", "")))
        elif name == "audio":
            raw += base64.b64decode(v.get("pcm16", ""))
            emit(ms, name, "%.1f kB of pcm at %s Hz" % (len(v.get("pcm16", "")) / 1024.0, v.get("rate")))
        elif name == "log":
            emit(ms, name, str(v.get("line", "")))
        elif name == "asr":
            emit(ms, name, "%s%s" % (v.get("asr", ""), "" if v.get("done") else " (still listening)"))
        elif name == "warn":
            emit(ms, name, str(v.get("warn", "")))
        elif name == "answer":
            answer, engine = v.get("answer", ""), v.get("engine", "-")
            emit(ms, name, answer)
        elif name == "reset":
            emit(ms, name, "the text so far became a tool call")
        elif name == "done":
            emit(ms, name, "first audio %s ms · %s sentence(s) · %s ms total"
                 % (v.get("first_audio_ms"), v.get("sentences"), v.get("total_ms")))
        elif name == "error":
            emit(ms, name, str(v.get("error", "")))
        else:
            emit(ms, name, blob)
        if name in ("done", "error"):
            break
    def show(what):
        return ("%.0f ms" % first[what]) if what in first else "never"
    print("answer:      %s" % (answer or "(none)"))
    print("engine:      %s" % engine)
    print("first asr:   %-9s first token: %s" % (show("asr"), show("token")))
    print("first audio: %-9s first answer:%s" % (show("audio"), show("answer")))
    if "audio" in first and "answer" in first and first["audio"] < first["answer"]:
        print("VERDICT:     the room began hearing the answer %.0f ms before it was complete"
              % (first["answer"] - first["audio"]))
    elif "audio" in first:
        print("VERDICT:     audio came only after the whole answer — that is the buffered path")
    if raw:
        w = wave.open("/tmp/voice-turn.wav", "wb")
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(16000)
        w.writeframes(raw)
        w.close()
        print("wrote:       /tmp/voice-turn.wav (%.1f s) — nothing has played" % (len(raw) / 2.0 / 16000.0))
    '; then
        echo
        echo "The stream failed. A 404 means this box is not running the generation that serves" >&2
        echo "/v1/turn yet: just switch on services1, or just deploy services1 from here." >&2
        exit 1
    fi

# Text-only: synthesise speech to a WAV and say nothing.  This is the half that is safe to
# run while nobody has confirmed what a C110 speaker sounds like across a working room.
voice-say text:
    #!/usr/bin/env bash
    set -euo pipefail
    cat <<'VO_SAY' | python3 -c 'import json,sys; sys.stdout.write(json.dumps({"text": sys.stdin.read().strip()}))' \
        | curl -fsS -m 60 -X POST -H 'content-type: application/json' \
            --data-binary @- -o /tmp/voice-say.wav https://voice.int.leighhack.org/v1/tts
    {{text}}
    VO_SAY
    python3 -c 'import wave; w=wave.open("/tmp/voice-say.wav"); print("/tmp/voice-say.wav: %d ch, %d Hz, %.1f s — play it yourself if you want to hear it" % (w.getnchannels(), w.getframerate(), w.getnframes()/w.getframerate()))'

# Does the camera's microphone path work, and what would the gate have done with it?
# Runs the deployed binary on services1 (it is a system package) against go2rtc's cam8
# restream, transcribes, and STOPS: no question reaches the assistant and no answer is
# spoken.  The measurement tool for the one thing this cannot answer at a desk: how often
# the space's music looks like speech.
voice-probe seconds="8": _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    # The binary is a package of services1's system, so this is a services1 command:
    # build-shaped recipes already refuse on gw, and the PATH check says which box it
    # actually wants.
    if ! command -v voice-assistant >/dev/null; then
        echo "voice-assistant is not on PATH here." >&2
        echo "  It is a package of services1's system: run this on services1," >&2
        echo "  or build it: nix build .#packages.x86_64-linux.voice-assistant" >&2
        exit 1
    fi
    # `--state-dir` is the tmpfs one, not /var/lib/voice-assistant: the unit owns that
    # directory and a hand-run probe writing answers into it would leave root-owned files
    # where the DynamicUser has to write.  The clip lands in the caller's /tmp instead.
    sudo voice-assistant probe \
        --state-dir /tmp/voice-probe-state \
        --seconds "{{seconds}}" \
        --out /tmp/voice-probe.wav
