# MODULE — NixOS module; in a machine's module list or a directory's imports, never read as data.
#
# The fleet's accounts. What this file owns that nobody else can: the uid (1234, a
# fact this repo has learned is not portable — see machines/services3/users.nix for why
# `cjdell` is *not* here) and **both halves of the inter-machine hop**:
# `authorizedKeys` below is the half that accepts a hop; the half that *offers* the key is
# `programs.ssh.hosts` in `./ssh-hop.nix` — the system client config, which is the only
# declarative place for it in this nixpkgs (see the comment inside the account below), and the
# half that was missing everywhere on every box.
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

    # What this account declares about the hop is the half that *accepts* it (the public
    # key above, authorized on every flake machine). The half that *offers* the private
    # key is not here: this pinned nixpkgs' `users.users.<name>` submodule has no file
    # writing option of any kind — `just check` refuses
    # `users.users.leigh-admin.extraFiles` (and `homeDirectories.<name>.extraFiles` is
    # not an option in this release either), so home files would need Home Manager, which
    # this flake does not use. The declarative identity is therefore in the *system*
    # client config, `programs.ssh.hosts`, in `./ssh-hop.nix` — which is the file every
    # ssh on the box reads, but not enough to cover root: that block spells the path
    # `~/.ssh/agent-hop-key`, and for the daemon `~` is `/root`, where no copy of the key
    # exists. So root's hand-written `/root/.ssh/config` stanzas (naming the absolute
    # `/home/leigh-admin/.ssh/agent-hop-key`) are still the only thing standing in for
    # root's ssh, and that half stays open in `docs/follow-ups.md`.

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
