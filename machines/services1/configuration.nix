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
