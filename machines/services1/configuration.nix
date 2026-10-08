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

  # This box builds packages that need to reach the network/NAS at build time
  # (see common/sops.nix for the build-time secret decryption).
  nix.settings.sandbox = "relaxed";

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
