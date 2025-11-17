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

  # Define a user account. Don't forget to set a password with ‘passwd’.
  users.users.cjdell = {
    uid = 1001;
    isNormalUser = true;
    description = "Chris Dell";
    extraGroups = [
      "networkmanager"
      "wheel"
      "render"
      "video"
    ];
    packages = with pkgs; [ ];
    openssh.authorizedKeys.keys = [
      "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQCbDJ7tQwODw2kx2f1bstOUElKnaR3hP2RbwCsf6zebZ5n/1CFUoM2Ye78D/IG/6kgDc22wD9EkzyvIwF/96fp3IgxK5ja/Q0pEhbd8xAPGIpFC7BUyePqozRusSvJXl7RamBb8lgsjySQxJxYX9MQzbQkfasWOwWE+WWqiC9nwk6WiER7EraOdEVNNF9cuNS/LVFrQZG5xdzI5gSgaxth2kQSgE3z7jIIvmlYkChEjTMXSQt9MrluhWB1nzGDHVrcqW8uu/jAqeMhRCXP39wtmL21v3WFn1jwDQlOgbR1CxnBzy+jE62TqvOJg8x6/J2WC/VXcdndHq1vKYP0s5mQn cjdell@gmail.com"
      "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAID8l40zlEkMtfTXjDq6L7JGaLDIFGYSqNr9gVoa4I7jS cjdell@rocketlakelatitude-nixos"
    ];
  };

  users.users.backups = {
    uid = 3000;
    isNormalUser = true;
    description = "Backup User";
  };

  users.groups.secrets = {
    gid = 9999;
  };

  users.groups.backups = {
    gid = 3000;
  };
}
