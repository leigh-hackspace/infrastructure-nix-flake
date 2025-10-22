{ config, pkgs, ... }:

{
  users.users.leigh-admin = {
    uid = 1234;
    isNormalUser = true;
    description = "Leigh Admin";
    extraGroups = [
      "networkmanager"
      "wheel"
      "render"
      "video"
    ];
    packages = with pkgs; [ ];
    openssh.authorizedKeys.keys = [
      "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFGBmtfOyJYWOoWMiZljl+XArMepgIZQ+D1ZhoUqOhT7 admin@leighhack.org"
    ];
  };
}
