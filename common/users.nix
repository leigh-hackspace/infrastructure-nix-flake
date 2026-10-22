# MODULE — NixOS module; in a machine's module list or a directory's imports, never read as data.
{pkgs, ...}: {
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
    openssh.authorizedKeys.keys = [
      "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFGBmtfOyJYWOoWMiZljl+XArMepgIZQ+D1ZhoUqOhT7 admin@leighhack.org"
      # Shared inter-machine key: same private key on every flake-managed
      # machine so agents can hop across them passwordlessly.
      # Private key: ~/.ssh/agent-hop-key (copy to the other machines).
      "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICtr1ewQo09xFMODjPXlPXXvjuW94zQCMqhrpKAT/yGI machine-hop-key"
    ];

    # Lingering must be on, or an agent session takes tmux down with it.
    #
    # The agent's shell runs in `user@1234.service/tmux-spawn-*.scope` — inside the
    # user manager's cgroup, not the ssh session's — and tmux itself is in the login
    # session's scope. When the harness times out a long tool call it SIGKILLs the
    # shell, sshd reports `mm_reap: child terminated by signal 9`, the login session
    # closes, and with lingering off logind stops `user@1234.service` — which SIGKILLs
    # everything in that cgroup: the tmux server, the shell, and any build still
    # running in it. That is why `nixos-rebuild switch`/`boot` looked like the culprit:
    # they are just the commands long enough to hit the harness limit.
    #
    # With lingering on the user manager survives the session teardown, so tmux and a
    # detached build survive too.
    linger = true;
  };

  # # Define a user account. Don't forget to set a password with ‘passwd’.
  # users.users.cjdell = {
  #   uid = 1001;
  #   isNormalUser = true;
  #   description = "Chris Dell";
  #   extraGroups = [
  #     "networkmanager"
  #     "wheel"
  #     "render"
  #     "video"
  #   ];
  #   packages = with pkgs; [ ];
  #   openssh.authorizedKeys.keys = [
  #     "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQCbDJ7tQwODw2kx2f1bstOUElKnaR3hP2RbwCsf6zebZ5n/1CFUoM2Ye78D/IG/6kgDc22wD9EkzyvIwF/96fp3IgxK5ja/Q0pEhbd8xAPGIpFC7BUyePqozRusSvJXl7RamBb8lgsjySQxJxYX9MQzbQkfasWOwWE+WWqiC9nwk6WiER7EraOdEVNNF9cuNS/LVFrQZG5xdzI5gSgaxth2kQSgE3z7jIIvmlYkChEjTMXSQt9MrluhWB1nzGDHVrcqW8uu/jAqeMhRCXP39wtmL21v3WFn1jwDQlOgbR1CxnBzy+jE62TqvOJg8x6/J2WC/VXcdndHq1vKYP0s5mQn cjdell@gmail.com"
  #     "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAID8l40zlEkMtfTXjDq6L7JGaLDIFGYSqNr9gVoa4I7jS cjdell@rocketlakelatitude-nixos"
  #   ];
  # };

  users.users.backups = {
    uid = 3000;
    isNormalUser = true;
    description = "Backup User";
  };

  users.users.hackspacer = {
    uid = 3002;
    isNormalUser = true;
    description = "Hackspace Generic User";
  };

  users.groups.secrets = {
    gid = 9999;
  };

  users.groups.backups = {
    gid = 3000;
  };
}
