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
  # box's; it belongs to aibox because aibox is asked to build this network's closures, and
  # everything that asks it for services1's system — `just check-build` run here, any
  # rebuild that has to compile that package rather than substitute it — stopped on that
  # refusal.  The 2026-11-22 upgrade run lost the gate to exactly this and had to move it
  # to services1 ("the gate refused to run where the runbook says to run it",
  # docs/work-log-2026-11.md); with the exemption here the gate answers on the builder too.
  #
  # `relaxed` is still a sandbox: `true` plus network for a derivation that *asks*, so
  # nothing that does not ask is any less hermetic than it was.  What asks, per box, is
  # answered by `just noChroot` (re-read 2026-11-23, all four reachable boxes: one in
  # services1's 8,886-derivation closure — that door-entry one; one in this box's own 18,547
  # — `llama-cpp-vulkan-0.0.0.drv`, a nixpkgs package that also asks, and which today is only
  # ever *substituted* here, so a strict daemon has never had to answer it; none in services2's
  # 8,027 or gw's 5,540; `services3` is Wi-Fi-only and was unreachable that day).  The
  # builder is asked for this exemption twice over: once by its own closure
  # (`llama-cpp-vulkan`, answered today only by substitution) and once by services1's
  # (door-entry, answered today only by services1 compiling it itself): leaving `sandbox =
  # true` on the network's builder has only ever been true because substitution happened to
  # cover what cannot be built under it.
  #
  # What it costs, said out loud rather than buried: a non-hermetic package now compiles
  # with the network on the box that compiles everything else, and its build is not
  # reproducible (the same section's `--rebuild` line says so).  Leaving this box strict costs every gate that asks the builder for
  # services1's closure.  The hermetic answer is upstream's and does not fit today: measured
  # 2026-11-23 under the current `deno` (2.9.8), `deno compile` downloads its own runtime
  # (`https://dl.deno.land/release/v2.9.8/denort-x86_64-unknown-linux-gnu.zip`), the jsr
  # deps' *version* manifests are fetched even with `--cached-only`, and the sass/esbuild
  # half pulls `baseline-browser-mapping` data — so a vendored closure would be deno-version
  # specific and would have to be re-pinned at every nixpkgs bump.  Receipts in
  # docs/follow-ups.md.
  nix.settings.sandbox = "relaxed";

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
