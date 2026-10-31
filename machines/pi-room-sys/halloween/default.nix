# PACKAGE — the SDDM greeter theme the Pi Room netboot image runs, and the Halloween
# screensaver that lives inside it.
#
# A greeter theme is the only code that draws while nobody is logged in, so "a
# screensaver at the login screen" is a theme, not a service: there is no X client to
# attach (the greeter is kwin_wayland) and nothing outside the theme can paint over it.
# `theme/Main.qml` is the login screen, `theme/Screensaver.qml` is the scare, and the
# knobs they read are generated into `Knobs.js` from the arguments below.
#
# **Every binary asset is generated here, from nothing.**  The five sounds are ffmpeg
# `aevalsrc`/`anoisesrc` expressions and the four static frames are crops of one frame
# of ffmpeg noise, so the repo carries no images or audio to license, attribute or
# keep in sync — the build *is* the provenance.  Sizes are small on purpose: 2.4 MB
# built (measured off the store path), of which the four static frames are 1.9 MB —
# noise does not PNG-compress, so they, not the 470 KB of sound, are what this costs a
# squashfs every client mounts over NFS.
#
# Changing a scare means editing the QML or the expressions below and switching aibox
# (`just switch`), then reading back `nix eval .#nixosConfigurations.pi-room-sys...`
# — this package is only reachable through `machines/pi-room-sys/configuration.nix`.
{
  lib,
  stdenvNoCC,
  ffmpeg,
  # Silence is not idleness: how long the login screen sits untouched before the
  # panel goes away.
  idleSeconds ? 60,
  # Month-and-day bounds (MMDD), inclusive, evaluated on the greeter's own clock.
  # Not a year, so it re-arms itself each October and disarms itself on its own.
  activeFrom ? 1030,
  activeTo ? 1102,
  # For testing: ignore the window entirely.
  alwaysOn ? false,
}: let
  themeName = "leighhack-halloween";
  themeDir = "$out/share/sddm/themes/${themeName}";
in
  stdenvNoCC.mkDerivation {
    pname = "sddm-theme-${themeName}";
    version = "1";

    dontUnpack = true;
    nativeBuildInputs = [ffmpeg];

    buildCommand = ''
      install -d ${themeDir}/sounds ${themeDir}/static

      install -m644 ${./theme/Main.qml} ${themeDir}/Main.qml
      install -m644 ${./theme/Screensaver.qml} ${themeDir}/Screensaver.qml
      install -m644 ${./theme/SoundPlayer.qml} ${themeDir}/SoundPlayer.qml
      install -m644 ${./theme/Field.qml} ${themeDir}/Field.qml
      install -m644 ${./theme/theme.conf} ${themeDir}/theme.conf
      install -m644 ${./theme/metadata.desktop} ${themeDir}/metadata.desktop

      # The values the QML reads.  A `.pragma library` script rather than theme.conf
      # because SDDM's `config` object is only guaranteed to expose the theme keys it
      # documents, and this is not worth depending on.
      cat > ${themeDir}/Knobs.js <<'EOF'
      .pragma library
      var idleSeconds = ${toString idleSeconds};
      var activeFrom = ${toString activeFrom};
      var activeTo = ${toString activeTo};
      var alwaysOn = ${lib.boolToString alwaysOn};
      EOF
      chmod 644 ${themeDir}/Knobs.js

      # ---- sound ------------------------------------------------------------
      #
      # Mono 22.05 kHz WAV: SoundEffect wants a whole file in memory, so these stay
      # tiny, and 22.05 kHz is the top of a whisper, a creak and a thump anyway.
      # Each is normalised to a peak near 0 dBFS and then scaled per clip by
      # SoundPlayer.qml — the bang is the one that is meant to land.

      # A low hit with a noise transient on top: the face.
      ffmpeg -v error -y \
        -f lavfi -i "aevalsrc='0.9*sin(2*PI*52*t)*exp(-10*t)':s=22050:d=0.8" \
        -f lavfi -i "anoisesrc=d=0.4:c=brown:a=0.6:s=22050" \
        -filter_complex "[1:a]lowpass=f=700,afade=t=out:st=0.05:d=0.35[n];[0:a][n]amix=inputs=2:duration=longest:normalize=0,alimiter=limit=0.95,volume=0.9[o]" \
        -map "[o]" -ac 1 -c:a pcm_s16le ${themeDir}/sounds/bang.wav

      # Two detuned rising tones, tremolo on top, decaying: the television.
      ffmpeg -v error -y \
        -f lavfi -i "aevalsrc='0.45*(sin(2*PI*(700+900*t)*t)+0.7*sin(2*PI*(706+905*t)*t))*(1-exp(-30*t))*exp(-1.1*t)':s=22050:d=1.6" \
        -af "tremolo=f=17:d=0.4,alimiter=limit=0.95,volume=1.3" \
        -ac 1 -c:a pcm_s16le ${themeDir}/sounds/shriek.wav

      # Band-limited pink noise breathing at 4.5 Hz: the eyes, and the words.
      ffmpeg -v error -y \
        -f lavfi -i "anoisesrc=d=2.4:c=pink:a=0.9:s=22050" \
        -af "highpass=f=600,lowpass=f=3000,tremolo=f=4.5:d=0.85,afade=t=in:st=0:d=0.5,afade=t=out:st=1.9:d=0.5,volume=1.1" \
        -ac 1 -c:a pcm_s16le ${themeDir}/sounds/whisper.wav

      # A wobbling rising tone through a narrow band: the figure.
      ffmpeg -v error -y \
        -f lavfi -i "aevalsrc='0.5*sin(2*PI*(140+60*t+20*sin(2*PI*2.3*t))*t)*(0.55+0.45*sin(2*PI*1.3*t))':s=22050:d=1.9" \
        -af "bandpass=f=900:width_type=o:w=1.5,alimiter=limit=0.95,afade=t=in:st=0:d=0.15,afade=t=out:st=1.5:d=0.4,volume=0.8" \
        -ac 1 -c:a pcm_s16le ${themeDir}/sounds/creak.wav

      # Two low thumps 340 ms apart: the red.
      ffmpeg -v error -y \
        -f lavfi -i "aevalsrc='0.95*sin(2*PI*44*t)*exp(-16*t)+0.8*sin(2*PI*41*(t-0.34))*exp(-18*(t-0.34))*gte(t,0.34)':s=22050:d=1.3" \
        -af "alimiter=limit=0.95,volume=0.9" \
        -ac 1 -c:a pcm_s16le ${themeDir}/sounds/heartbeat.wav

      # ---- static -----------------------------------------------------------
      #
      # One frame of noise, cropped four ways: ffmpeg's noise filter has no seed, so
      # four *separate* runs would produce four copies of the same pattern.  Four
      # quadrants of one frame are four independent samples by construction.
      #
      # The source has to be **grey, not black**: `noise` modulates its input, so on
      # `nullsrc` (black) it can only add, and the first build of this produced static
      # with a mean luma of 6 — invisible against a black screen.  Measured: grey
      # source 127 average, black source 6.
      ffmpeg -v error -y -f lavfi -i "color=c=gray:s=1920x1080,noise=alls=100:allf=u,format=gray" \
        -frames:v 1 "$TMPDIR/noise.png"

      ffmpeg -v error -y -i "$TMPDIR/noise.png" -vf "crop=960:540:0:0"     -frames:v 1 ${themeDir}/static/static-0.png
      ffmpeg -v error -y -i "$TMPDIR/noise.png" -vf "crop=960:540:960:0"   -frames:v 1 ${themeDir}/static/static-1.png
      ffmpeg -v error -y -i "$TMPDIR/noise.png" -vf "crop=960:540:0:540"   -frames:v 1 ${themeDir}/static/static-2.png
      ffmpeg -v error -y -i "$TMPDIR/noise.png" -vf "crop=960:540:960:540" -frames:v 1 ${themeDir}/static/static-3.png

      # A listing is the only check this package can make on itself: nothing here can
      # run the QML, so the journal of a real greeter is the test (see the README).
      find $out -type f | sort
      du -sh $out
    '';

    meta = {
      description = "Halloween SDDM greeter theme for the Pi Room netboot image";
      license = lib.licenses.mit;
      platforms = lib.platforms.linux;
    };
  }
