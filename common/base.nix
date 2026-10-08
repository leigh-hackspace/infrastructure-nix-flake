# Settings shared by every machine in this flake.
#
# Imported from `machines/<name>/configuration.nix`.  Keep the per-machine file
# to what genuinely differs: `system.stateVersion`, boot/kernel specifics
# (extra module packages, kernel params), hardware, networking and services.
#
# Everything below used to be copy-pasted in both configuration.nix files — the
# audit counted ~15 identical settings — so a change made on one machine silently
# did not happen on the other.
{
  # Both machines run nixos-rebuild from this flake's git tree.
  nix.settings.experimental-features = [
    "nix-command"
    "flakes"
  ];

  # GOLDEN RULE: the auto-rollback timer rolls back to the last *confirmed*
  # generation within a minute or two of a switch, so `nixos-confirm` has to run
  # in the same shell invocation as the switch (just switch/boot do this).
  system.autoRollback.enable = true;

  # Bootloader: both boxes are UEFI with systemd-boot.
  boot.loader.systemd-boot.enable = true;
  boot.loader.efi.canTouchEfiVariables = true;

  time.timeZone = "Europe/London";

  i18n.defaultLocale = "en_GB.UTF-8";

  i18n.extraLocaleSettings = {
    LC_ADDRESS = "en_GB.UTF-8";
    LC_IDENTIFICATION = "en_GB.UTF-8";
    LC_MEASUREMENT = "en_GB.UTF-8";
    LC_MONETARY = "en_GB.UTF-8";
    LC_NAME = "en_GB.UTF-8";
    LC_NUMERIC = "en_GB.UTF-8";
    LC_PAPER = "en_GB.UTF-8";
    LC_TELEPHONE = "en_GB.UTF-8";
    LC_TIME = "en_GB.UTF-8";
  };

  # Keyboard: UK layout in X11 and on the console.
  services.xserver.xkb = {
    layout = "gb";
    variant = "";
  };
  console.keyMap = "uk";

  # Deliberate: both boxes are single-operator machines with `leigh-admin` in
  # wheel and passwordless sudo (the deploy recipes assume it).  Paired with
  # `mitigations=off` in each machine's hardware-configuration.nix — the same
  # "trusted LAN, single operator, speed matters" tradeoff, decided once here
  # rather than discovered in two kernel-param lists.
  security.sudo.wheelNeedsPassword = false;

  # Prebuilt binaries from cargo/npm find the dynamic linker they need.
  programs.nix-ld.enable = true;

  services.openssh.enable = true;
}
