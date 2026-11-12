# services3-specific system configuration.  Everything shared with the other
# machines (timezone, i18n, keyboard, sudo, nix-ld, ssh, bootloader,
# autoRollback, experimental-features, the nixbld invariant) comes from
# common/base.nix, imported below — keep this file to what is specific to this box,
# which today is almost nothing, because this box runs nothing yet.
#
# What this box is, and what it is not yet, is ./README.md's job: the measured
# hardware, the bench Wi-Fi, the GTX 1080 Ti nobody drives, the UPS question, and the
# move to VLAN 225.  Add a per-service module here alongside the first real service
# that needs one, not before (the same rule machines/services2/configuration.nix
# states).
{...}: {
  imports = [../../common/base.nix];

  # Not measured, inherited: the box's previous flake
  # (`github:cjdell/nixos-config`, `common/system.nix`) declared 24.11 for every host
  # in it, and this disk has been through 24.11 → 25.05 → 26.11 under that tree.  It
  # is written as inherited rather than as a fact about an installer because nobody
  # here watched this box being installed; `24.11` keeps whatever deprecations that
  # release pins are already honoured, which is the conservative direction.
  system.stateVersion = "24.11"; # Did you read the comment?
}
