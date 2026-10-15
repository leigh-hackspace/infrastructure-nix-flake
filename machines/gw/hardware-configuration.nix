# PLACEHOLDER — there is no hardware for this machine yet.  See ./README.md
# ("Getting a real hardware-configuration.nix") for the procedure and for the
# two facts this file owns that `nixos-generate-config` cannot know: the NIC
# names the VLAN netdevs and the network files in ./networking.nix key on, and
# the fact that this box mounts nothing from the NAS.
#
# Everything below is deliberately empty rather than guessed: an invented
# `boot.initrd.availableKernelModules` list looks like evidence and is a lie,
# and the difference between the two is a box that fails to boot from its own
# initrd at 04:00 after a power cut.  `nix flake check` evaluates this file;
# nothing in it claims anything about a board that does not exist.
{
  config,
  lib,
  pkgs,
  modulesPath,
  ...
}: {
  # Replace this whole attrset with what `nixos-generate-config` writes when the
  # box exists, then re-apply the NIC naming below.
  boot.initrd.availableKernelModules = [];
  boot.initrd.kernelModules = [];
  boot.extraModulePackages = [];
  boot.kernelModules = [];

  # The router's board is expected to be an mpc858 (the old OPNsense box's
  # `uname` said FreeBSD 15.1 on it) or something equivalent, i.e. x86_64 with
  # a wired LAN NIC that does VLAN tagging in hardware and a second NIC for the
  # uplink.  Confirm on the real box; do not assume it because the old one was.
  #
  # `mitigations=off` is what the other two machines carry (same
  # "trusted LAN, single operator, speed matters" tradeoff, decided once in
  # common/base.nix's comment).  Left out here on purpose: a router is the one
  # box whose job is to answer for the whole network, and if it is the box that
  # gets a kernel param wrong, every other machine's DNS goes with it.  Add it
  # when the hardware is known, not before.
  boot.kernelParams = [];

  # THE ROOT FILESYSTEM, and the one place this file breaks its own rule about
  # inventing nothing.  Without *a* root filesystem NixOS refuses to build the
  # system at all (“the ‘fileSystems’ option does not specify your root file
  # system”), and that would cost the whole machine its CI value: `nix flake check`
  # only evaluates, whereas a build renders dnsmasq's conf, the networkd files and
  # — the one that matters most — validates ./firewall.nix's ruleset against a real
  # kernel in userspace (nixpkgs' `networking.nftables.checkRuleset`).  So the two
  # entries below are the *installer's default layout*, written as labels rather
  # than UUIDs so nobody mistakes them for a measurement of a board:
  #
  #   * they are not a claim about any disk, and nothing here will mount them;
  #   * replace this whole attrset with what `nixos-generate-config` writes the
  #     moment the box exists, which is the procedure ./README.md gives;
  #   * a box booted with these lines still in place will fail to mount its root —
  #     loudly, at boot, which is the correct failure for a machine that has no
  #     hardware yet, and is why this is a placeholder rather than a default.
  fileSystems."/" = {
    device = "/dev/disk/by-label/nixos";
    fsType = "ext4";
  };

  # The ESP.  common/base.nix enables systemd-boot for every machine in the fleet,
  # so this is what that needs to exist; the UEFI variables it would touch are a
  # fact about the board, and `canTouchEfiVariables` stays in common/base.nix where
  # the fleet decided it until the board is known.
  fileSystems."/boot" = {
    device = "/dev/disk/by-label/BOOT";
    fsType = "vfat";
  };

  # NIC NAMING — the contract ./networking.nix reads.
  #
  # The VLAN devices in ./networking.nix hang off one NIC, and the network files
  # match it by *name*, because a VLAN device has no MAC of its own to match on.
  # The kernel's name for the LAN NIC is a hardware fact, so it lives here:
  # rename it to a stable name with a `links` rule once the box exists, and set
  # LAN_NIC in ./networking.nix to the same name.
  #
  #   systemd.network.links."0-lan-nic" = {
  #     ruleConfig = {
  #       MACAddress = "<the LAN NIC's ether, off the real box>";
  #       Name = "router-lan";
  #     };
  #   };
  #
  # The uplink NIC (the one AAISP's PPPoE tunnel arrives on — the old box called
  # it `enc0`) gets the same treatment with the name `router-uplink`.  Neither
  # rule is written yet, so ./networking.nix's `LAN_NIC`/`UPLINK_NIC` match
  # nothing and the box would answer nothing: that is the honest state, not a
  # half-configured router that looks like it works.
}
