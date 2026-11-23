# aibox-specific system configuration.  The settings shared with services1
# (timezone, i18n, keyboard, sudo, nix-ld, ssh, bootloader, autoRollback,
# experimental-features) live in common/base.nix; keep this file to what is
# specific to this box.
{pkgs, ...}: {
  imports = [../../common/base.nix];

  # This box is the desk machine as well as the AI/camera box: the user is
  # logged in on the console without a password prompt.
  services.displayManager.autoLogin.enable = true;
  services.displayManager.autoLogin.user = "leigh-admin";

  programs.firefox.enable = true;

  # The network exemption, and this box carries it for a reason that is not one of this
  # box's packages.  `door-entry-management-system` sets `__noChroot`, and nix refuses such
  # a derivation outright when the daemon's `sandbox` is `true`:
  #
  #     error: derivation '…-door-entry-management-system-1.1.9.drv' has '__noChroot' set,
  #            but that's not allowed when 'sandbox' is 'true'
  #
  # Measured 2026-11-23 on services1, forcing that flag for one run:
  #
  #     nix build --no-link --rebuild --option sandbox true \
  #       ".#nixosConfigurations.services1.pkgs.door-entry-management-system"
  #
  # refuses it; the same command with `--option sandbox relaxed` builds it, and `--rebuild`
  # then prints `may not be deterministic: output … differs` — the *other* half of what is
  # open, in `docs/follow-ups.md`.  That package is in **services1's** closure, not this
  # box's; it belongs to aibox because aibox is where the fleet's build gate gets typed —
  # `just check-build` builds all five system toplevels into the store of the box you run
  # it from, and this is the biggest box on the network (the other four have 15, 30, 7 GiB
  # and a Wi-Fi dongle) — so anything that has to compile services1's system rather than
  # substitute it lands here and stopped on that refusal.  The 2026-11-22 upgrade run lost
  # the gate to exactly this and had to move it to services1 ("the gate refused to run
  # where the runbook says to run it", docs/work-log-2026-11.md); with the exemption here
  # the gate answers where it is actually typed.
  #
  # `relaxed` is still a sandbox: `true` plus network for a derivation that *asks*, so
  # nothing that does not ask is any less hermetic than it was.  What asks, per box, is
  # answered by `just noChroot` (re-read 2026-11-23, all four reachable boxes: one in
  # services1's 8,886-derivation closure — that door-entry one; one in this box's own 18,547
  # — `llama-cpp-vulkan-0.0.0.drv`, a nixpkgs package that also asks, and which today is only
  # ever *substituted* here, so a strict daemon has never had to answer it; none in services2's
  # 8,027 or gw's 5,540; `services3` is Wi-Fi-only and was unreachable that day).  This box
  # is asked for the exemption twice over: once by its own closure (`llama-cpp-vulkan`,
  # answered today only by substitution) and once by services1's (door-entry, answered
  # today only when the gate is compiled here): leaving `sandbox = true` here has only ever
  # been survivable because substitution happened to cover what cannot be built under it.
  #
  # What it costs, said out loud rather than buried: a non-hermetic package now compiles
  # with the network on the box where the fleet's gate is typed, and its build is not
  # reproducible (the same section's `--rebuild` line says so).  Leaving this box strict
  # costs every gate that compiles services1's closure here.  The hermetic answer is upstream's and does not fit today: measured
  # 2026-11-23 under the current `deno` (2.9.8), `deno compile` downloads its own runtime
  # (`https://dl.deno.land/release/v2.9.8/denort-x86_64-unknown-linux-gnu.zip`), the jsr
  # deps' *version* manifests are fetched even with `--cached-only`, and the sass/esbuild
  # half pulls `baseline-browser-mapping` data — so a vendored closure would be deno-version
  # specific and would have to be re-pinned at every nixpkgs bump.  Receipts in
  # docs/follow-ups.md.
  nix.settings.sandbox = "relaxed";

  # **This box builds, and it builds bounded — for its own sake and for strata's.**
  # aibox stopped being the network's builder on 2026-11-23 (common/nix-gc.nix's header),
  # which removed the *incoming* half of the load but not the local half: a `just switch`
  # here, a `just check-build`, or a `just test` still compiles on this box, and nix's own
  # default for `max-jobs` is `auto` = `nproc` = **12 jobs on 6 cores**.  That is what the
  # two engine deaths of 2026-11-21 and 2026-11-23 were made of (machines/aibox/STRATA.md
  # has both receipts; the short form is that the engine's arena is 33-39 GiB of a 59 GiB
  # box, this box has 8.8 GiB of swap, and on an iGPU every GPU page is one the kernel
  # might be moving: 206 GiB scanned in direct reclaim since boot).
  #
  # So, with strata's `MemoryMin = "40G"` protecting the arena from the other side
  # (./strata.nix):
  #
  #   * 4 jobs, not 12: enough to keep the gate brisk, and a rust link peak of roughly
  #     2-3 GiB per job rather than twelve of them racing.
  #   * `nix-daemon` capped at 12 GiB (throttled) / 18 GiB (refusing).  Builds run as
  #     children of the daemon, so that cgroup is where a runaway `rustc` lands, and the
  #     number is this box's RAM minus strata's reservation minus the apps.  **What this
  #     changes on purpose:** a build that genuinely needs more — a browser link, a
  #     nixpkgs bump with the engine loaded — now fails as a build instead of succeeding
  #     and taking the endpoint with it.  That is the documented old advice
  #     (`systemctl stop strata` before a big compile) with the failure moved to the cheap
  #     side.  Escape hatch, per pass and non-declarative:
  #     `sudo systemctl set-property nix-daemon.service MemoryMax=32G MemoryHigh=24G`.
  nix.settings.max-jobs = 4;
  systemd.services.nix-daemon.serviceConfig = {
    MemoryHigh = "12G";
    MemoryMax = "18G";
  };

  # GPU diagnostics for this box's GPU only (aibox is the AMD/ROCm box — see the
  # amdgpu.* kernel params in ./hardware-configuration.nix and ./ai.nix).  The
  # Intel tools live on services1; common/tools.nix used to install both on both
  # machines.
  environment.systemPackages = [pkgs.amdgpu_top];

  # This value determines the NixOS release from which the default
  # settings for stateful data, like file locations and database versions
  # on your system were taken. It‘s perfectly fine and recommended to leave
  # this value at the release version of the first install of this system.
  # Before changing this value read the documentation for this option
  # (e.g. man configuration.nix or on https://nixos.org/nixos/options.html).
  system.stateVersion = "24.11"; # Did you read the comment?
}
