# services1-specific system configuration.  The settings shared with aibox
# (timezone, i18n, keyboard, sudo, nix-ld, ssh, bootloader, autoRollback,
# experimental-features) live in common/base.nix; keep this file to what is
# specific to this box.
{
  config,
  pkgs,
  ...
}: {
  imports = [../../common/base.nix];

  # The hackspace Slack bot is live in the workspace as of 2026-11-16: it answers
  # unmentioned in the channel a request came from, like `vulcan-bot` did.  Set this
  # back to `true` (one line, one switch) to make it read and log without posting
  # anything — that is the kill switch, and it is why the option defaults to `true`
  # for any future first deploy.  Design, receipts and the overlap rules:
  # docs/slack-agent-2026-11-16.md.
  infra.slackAgent.dryRun = false;

  # The voice assistant, live but **silent in the room**: `voice-gateway` answers the
  # browser page (whisper.int.leighhack.org → the Assistant card) and `voice-satellite`
  # listens on cam8's mic and writes its answers to the state directory instead of
  # playing them, because nobody has yet decided a ceiling speaker should talk over a
  # room (and the unit still needs a key it may read before it could).  Flip
  # `infra.voiceAssistant.satellite.enable = false;` to stop the camera listening
  # altogether while the gate's false-probe rate is being measured with
  # `just voice-probe`.  Design, the measured latency budget and the traps:
  # crates/voice-assistant/README.md.
  infra.voiceAssistant.enable = true;

  # ── Wake-word measurement window, opened 2026-11-17, threshold closed same day ─────
  # Opened because server wake fired far less often than the browser's own detector and
  # every explanation so far was a guess: a score with no bytes behind it cannot be
  # re-run or compared.  So it records what the engine is fed, and the threshold went to
  # 0.30 on the strength of a measured room-noise peak of 0.031 — a figure taken from a
  # clip of a *nearly empty* room, which is the error to remember: the thing a wake word
  # competes with is not silence, it is people talking.
  #
  # The corpus this window collected (418 s of the room with people in it, scored by the
  # LiveKit model with `voice-wake bench` over the recordings) reads:
  #
  #     clean synthetic "hey jarvis"      peak 0.626
  #     390 s of ordinary room speech     peak 0.583   ← eight accepts at 0.30, two at 0.50
  #     21.7 s of room speech             peak 0.302
  #
  # A clean phrase and ordinary speech are a factor of two apart, so **no threshold
  # separates them** and 0.30 was not a probe with margin, it was a probe with a guess in
  # it.  0.50 is the least-bad setting, chosen while the detector is replaced rather than
  # tuned; the number is not the problem and should not be re-tuned as if it were.
  #
  # `wake.record` stays on, because it is the only instrument that has ever compared two
  # detectors on the same room audio — the replacement gets judged with it before it is
  # trusted with a microphone.
  infra.voiceAssistant.wake.record = true;
  infra.voiceAssistant.wake.threshold = 0.50;

  # `relaxed` rather than the default `true` for exactly **one** derivation in this box's
  # build closure: `door-entry-management-system`, whose `installPhase` runs `deno i` and
  # `deno cache` and therefore needs the network, so the package sets `__noChroot` — and nix
  # refuses a `__noChroot` derivation outright when `sandbox = "true"` (the refusal, and why
  # the builder cannot take this package, is `docs/follow-ups.md`).  `relaxed` is `true`
  # plus that one exemption, so this is the narrowest setting that lets the box build its own
  # system; it is *not* "no sandbox".
  #
  # It used to say this was for sops' build-time secret decryption, which is not true: that
  # path needs `/var/lib/sops-nix` to be *visible inside* the sandbox, which
  # `common/sops.nix` already arranges with `extra-sandbox-paths` (it shows up in
  # `nix config show`'s `sandbox-paths` on every box, aibox's strict one included).
  #
  # The one-line recipe that answers "does this box still need it" — and gives a different
  # answer the day the package stops setting `__noChroot`:
  #
  #   sudo nix-store -qR $(sudo nix-store -qd /run/current-system) | grep '\.drv$' \
  #     | xargs -r grep -l __noChroot
  #
  # (measured 2026-10-25: one hit here, none on services2 — which is why services2 is back
  # on the default; see machines/services2/configuration.nix.)
  nix.settings.sandbox = "relaxed";

  # GPU diagnostics for this box's GPU only (services1 is Intel/i915 — see
  # boot.initrd.kernelModules in ./hardware-configuration.nix).  The AMD tools
  # live on aibox; common/tools.nix used to install both on both machines.
  environment.systemPackages = [pkgs.intel-gpu-tools];

  # The gasket driver (Coral TPU) is not in the running kernel's package set,
  # so pull it from the matching kernel's package set explicitly.
  boot.extraModulePackages = [pkgs.linuxKernel.packages.linux_6_18.gasket];

  # This value determines the NixOS release from which the default
  # settings for stateful data, like file locations and database versions
  # on your system were taken. It‘s perfectly fine and recommended to leave
  # this value at the release version of the first install of this system.
  # Before changing this value read the documentation for this option
  # (e.g. man configuration.nix or on https://nixos.org/nixos/options.html).
  system.stateVersion = "23.11"; # Did you read the comment?
}
