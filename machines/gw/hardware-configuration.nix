# Hardware facts for the router box, measured by `nixos-generate-config` on the
# box itself (./README.md's install notes: what was read, and when) plus the two
# NIC-naming rules below, which `nixos-generate-config` cannot know because they
# are a contract with ./networking.nix rather than a measurement.
#
# The board: a Supermicro X9SCL/X9SCM (BIOS 2.3a, UEFI), a Xeon E3-1220 v2, 8 GB
# of RAM, one 256 GB Micron SATA SSD (`/dev/disk/by-id/ata-Micron_MTFDDAK256TDL_
# 2007267FBA4B`), and two Intel e1000e NICs.  It replaced a Windows install on
# the same disk, which is what the flake was asked to erase; the partition layout
# is the fleet's (1 GB ESP labelled NIXBOOT, the rest ext4 labelled NIXROOT, no
# swap — a router with 8 GB and four daemons does not need one, and a router that
# swaps is a router that answers DNS slowly).
#
# The one thing on this box that is *not* a measurement is the assumption that
# `eno1` is the LAN and `eno2` the uplink.  It is a very good assumption — eno1
# is the port carrying this network's VLAN 225 traffic today, and eno2 has no
# carrier — but it is still an assumption, and ./README.md says how to re-check
# it (`ip -br link`, and which one answers the network).
{
  config,
  lib,
  pkgs,
  modulesPath,
  ...
}: {
  imports = [
    (modulesPath + "/installer/scan/not-detected.nix")
  ];

  boot.initrd.availableKernelModules = [
    "ehci_pci"
    "ahci"
    "usbhid"
    "usb_storage"
    "sd_mod"
  ];
  boot.initrd.kernelModules = [];
  boot.kernelModules = ["kvm-intel"];
  boot.extraModulePackages = [];

  # `mitigations=off` is what the other machines carry (the "trusted LAN, single
  # operator, speed matters" tradeoff, decided once in common/base.nix's comment).
  # It stays off this box's command line deliberately even though the hardware is
  # now known: this is the one machine whose job is to answer for the whole
  # network, the tradeoff is about *its* CPU and *its* exposure, and the person
  # who decides it should be the one cutting it over — not whoever installed the
  # OS.  ./docs/follow-ups.md has it as an open item.
  boot.kernelParams = [];

  # The root filesystem, as `nixos-generate-config` measured it: the ext4
  # partition on the Micron SSD, by UUID (the label is NIXROOT, which is what
  # ./README.md's layout says to re-create if this disk is ever rebuilt by hand).
  fileSystems."/" = {
    device = "/dev/disk/by-uuid/e9588957-633f-4a80-8d0a-6a3b2e29c582";
    fsType = "ext4";
  };

  # The ESP, for the systemd-boot common/base.nix enables fleet-wide.
  fileSystems."/boot" = {
    device = "/dev/disk/by-uuid/76D5-29C0";
    fsType = "vfat";
    options = ["fmask=0022" "dmask=0022"];
  };

  swapDevices = [];

  # NIC NAMING — the contract ./networking.nix reads.
  #
  # The VLAN devices in ./networking.nix hang off one NIC and its network files
  # match by *name*, because a VLAN device has no MAC of its own to match on.  So
  # the kernel's name for each port is pinned to a name that survives a BIOS
  # update, a firmware bump, or the two cables moving between the two ports:
  # `router-lan` is whichever port owns this MAC, and the LAN follows it.  The
  # names are ./constants.nix's, and both files read that one place.
  #
  # Renaming by MAC rather than editing ./networking.nix to say `eno1` is also
  # what makes the two ports interchangeable: the day the uplink cable moves to
  # the other physical port, nothing in the flake changes.  `matchConfig` is the
  # .link file's [Match] section and `linkConfig` its [Link] section — the MAC is
  # what identifies the port, the name is what we want it called.
  systemd.network.links = {
    # Intel e1000e, link up, carrying the VLAN 225 trunk this box is plugged into
    # (measured: 100 Mbit — the port or the switch it lands on is not doing
    # gigabit, which ./README.md notes because it bounds what this router can
    # forward once it is the real one).
    "0-router-lan" = {
      matchConfig.MACAddress = "00:25:90:d3:f0:ea";
      linkConfig.Name = "router-lan";
    };

    # Intel e1000e, no carrier: the port AAISP's handoff goes into when this box
    # takes over the uplink (./uplink.nix, ./networking.nix's `10-uplink`).
    "1-router-uplink" = {
      matchConfig.MACAddress = "00:25:90:d3:f0:eb";
      linkConfig.Name = "router-uplink";
    };
  };

  nixpkgs.hostPlatform = lib.mkDefault "x86_64-linux";
  hardware.cpu.intel.updateMicrocode = lib.mkDefault config.hardware.enableRedistributableFirmware;
}
