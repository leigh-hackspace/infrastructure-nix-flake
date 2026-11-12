# The `cjdell` account, declared **for this box only**.
#
# WHY THIS FILE EXISTS AT ALL — the fact that made it necessary, read off
# `nixos-rebuild dry-activate` before the first switch of this machine:
#
#     would remove user ‘cjdell’
#
# NixOS removes a user that the *previous* system declared and the new one does
# not, and the box arrived from a flake that declared it
# (`cjdell/nixos-config`, `users/cjdell/nix.nix`).  So the honest reading is that a
# first switch of this machine, with no such declaration, deletes the account of the
# person the box belongs to — together with, on NixOS, the home directory that
# `userdel -r` takes with it, which on this box holds the previous fleet's checkout
# and several gigabytes of nobody-else's-files.  Nothing about "this machine is now
# services3" asks for that, and a machine that costs its operator their home to join
# the fleet is not assimilated, it is vandalised.
#
# WHY IT IS NOT IN `common/users.nix`: uncommenting the block that lives there would
# ask every box in the network to create uid 1000, and this repo has already learned
# the hard way that uids are not portable facts (`docs/auth-posix-logins.md`: on the
# sssd boxes every uid is `2000 + an authentik Postgres primary key`, and renumbering
# one is a change to the ACL on a person's NAS home).  `leigh-admin` is in
# `common/users.nix` because it is the fleet's account and it carries the fleet's uid
# (1234); `cjdell` here is this box's local owner account, and it says so.
#
# The password is deliberately **not** declared. `users.mutableUsers` is true here, so
# an undeclared password is left alone — which keeps the one
# password-authenticated way onto a box whose only uplink is a Wi-Fi dongle working
# through the switch that renames it.  It is the fallback for the failure mode this
# machine's README calls out, so it is a decision to revisit, not an oversight: check
# `sudo sshd -T | grep -i passwordauthentication` after any change to the ssh settings,
# because the key in `common/users.nix` is the primary path and a `PasswordAuthentication
# no` that arrives by default would silently retire the spare.
#
# The three keys below are the box's previous config's, kept verbatim so nothing the
# owner already uses stops working. `machines/services3/README.md` owns the open items.
{...}: {
  users.users.cjdell = {
    # The uid it already has on this box (`getent passwd cjdell` → 1000).  Declaring it
    # rather than letting NixOS pick keeps the ownership of everything under
    # /home/cjdell correct without a chown -R.
    uid = 1000;
    isNormalUser = true;
    description = "Chris Dell";
    # `networkmanager` because the uplink is NM's (./networking.nix); `wheel` because
    # `just deploy` runs `sudo nixos-rebuild` and sudo here is passwordless by
    # common/base.nix; `render`/`video` because the GTX 1080 Ti is in the box waiting
    # for a decision (./hardware-configuration.nix).
    extraGroups = [
      "networkmanager"
      "wheel"
      "render"
      "video"
    ];
    openssh.authorizedKeys.keys = [
      "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQCbDJ7tQwODw2kx2f1bstOUElKnaR3hP2RbwCsf6zebZ5n/1CFUoM2Ye78D/IG/6kgDc22wD9EkzyvIwF/96fp3IgxK5ja/Q0pEhbd8xAPGIpFC7BUyePqozRusSvJXl7RamBb8lgsjySQxJxYX9MQzbQkfasWOwWE+WWqiC9nwk6WiER7EraOdEVNNF9cuNS/LVFrQZG5xdzI5gSgaxth2kQSgE3z7jIIvmlYkChEjTMXSQt9MrluhWB1nzGDHVrcqW8uu/jAqeMhRCXP39wtmL21v3WFn1jwDQlOgbR1CxnBzy+jE62TqvOJg8x6/J2WC/VXcdndHq1vKYP0s5mQn cjdell@gmail.com"
      "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAID8l40zlEkMtfTXjDq6L7JGaLDIFGYSqNr9gVoa4I7jS cjdell@rocketlakelatitude-nixos"
      "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFKXFQsN5M23Yeh1jPYv/8Ys6wrH4VIluuJ5177ovXZW cjdell@alderlake-thinkpad"
    ];
  };
}
