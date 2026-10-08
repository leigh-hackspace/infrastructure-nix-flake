# aibox-specific system configuration.  The settings shared with services1
# (timezone, i18n, keyboard, sudo, nix-ld, ssh, bootloader, autoRollback,
# experimental-features) live in common/base.nix; keep this file to what is
# specific to this box.
{
  imports = [ ../../common/base.nix ];

  # This box is the desk machine as well as the AI/camera box: the user is
  # logged in on the console without a password prompt.
  services.displayManager.autoLogin.enable = true;
  services.displayManager.autoLogin.user = "leigh-admin";

  programs.firefox.enable = true;

  # This value determines the NixOS release from which the default
  # settings for stateful data, like file locations and database versions
  # on your system were taken. It‘s perfectly fine and recommended to leave
  # this value at the release version of the first install of this system.
  # Before changing this value read the documentation for this option
  # (e.g. man configuration.nix or on https://nixos.org/nixos/options.html).
  system.stateVersion = "24.11"; # Did you read the comment?
}
