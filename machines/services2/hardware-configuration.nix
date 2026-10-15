# Hardware configuration for services2.
#
# Derived from `nixos-generate-config` output on the installed NVMe (Kingston
# SA2000M8 250G) with the per-machine bits services1 also carries added back:
# the NAS exports (common/nas.nix turns them into mounts + automounts) and the
# deliberate mitigations=off tradeoff.
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
    "xhci_pci"
    "ahci"
    "nvme"
    "usb_storage"
    "usbhid"
    "sd_mod"
  ];
  boot.initrd.kernelModules = [];
  boot.kernelModules = ["kvm-amd"];
  boot.extraModulePackages = [];

  # Performance tradeoff, decided deliberately: CPU side-channel mitigations
  # are off on all machines (see also security.sudo.wheelNeedsPassword = false
  # in common/base.nix).  These boxes are single-operator, on a trusted LAN
  # behind authentik/nginx, and the workload is I/O- and GPU-bound; the cost is
  # real (Spectre/Meltdown class attacks) and accepted, not overlooked.
  boot.kernelParams = [
    "mitigations=off"
  ];

  boot.kernel.sysctl = {
    "kernel.task_delayacct" = 1;
  };

  # AMD Cezanne Radeon iGPU (Mesa/radeonsi provides VA-API/VDPAU), unlike
  # services1 which is Intel/i915 and needs intel-media-driver.
  hardware.graphics = {
    enable = true;
  };

  fileSystems."/" = {
    device = "/dev/disk/by-uuid/b4f380f9-e3a0-4e1b-aacc-538f78f4e36a";
    fsType = "ext4";
  };

  fileSystems."/boot" = {
    device = "/dev/disk/by-uuid/FBB6-8ECF";
    fsType = "vfat";
    options = [
      "fmask=0022"
      "dmask=0022"
    ];
  };

  # NAS shares (TrueNAS, infra.nas.host).  common/nas.nix turns each of these
  # into the mount unit *and* its automount unit, and builds wait-for-nas.service
  # from the same list — see the header of that file for why the automount units
  # have to exist explicitly (the `x-systemd.automount` mount option is dead
  # config for unit-file mounts) and why NAS-dependent services must order
  # themselves against wait-for-nas.service.
  #
  # services2 has no services yet, so nothing consumes these; they are here so
  # the box is a drop-in clone of services1 once they are enabled.
  infra.nas.exports = [
    {
      where = "/mnt/cameras";
      share = "/mnt/sas-10k/cameras";
    }
    {
      where = "/mnt/filestore";
      share = "/mnt/sas-10k/filestore";
    }
    {
      where = "/mnt/backups";
      share = "/mnt/sas-10k/backups";
    }
  ];

  # Swap.  This box runs Strata's Qwen3.8-Flash-Next Coder engine
  # (./strata.nix, machines/services2/STRATA.md): the expert arena alone is
  # 23.4 GiB of a 30 GiB box, so strata (28.3 GiB RSS measured) plus
  # Prometheus/Grafana/gatus/exporters leaves ~1.5 GiB of headroom.  Without
  # swap the box goes unresponsive under that pressure - journald logs
  # "Under memory pressure, flushing caches" - and a build here (this box is
  # the network's build machine, common/build-farm.nix, maxJobs 8) has nowhere
  # to go and gets OOM-killed.  Swap is the safety net: the arena is page-
  # locked where it can (`LimitMEMLOCK = infinity` in ./strata.nix), so what
  # swaps is the overflow, not the model.
  #
  # A swap FILE, not a partition: unlike services1 (nvme0n1p3, 18 GiB) and
  # aibox (a by-uuid partition), this NVMe is only p1 (512M vfat /boot) + p2
  # (232G ext4 /), so there is no spare partition to enable and repartitioning
  # a live 232 GB disk is not something a switch can do.  120 GB is free on /,
  # and `size` is what makes nixpkgs create the file for us: it renders a
  # /etc/fstab line (which systemd-fstab-generator turns into
  # var-swap-services2.swap) plus an idempotent helper,
  # mkswap-var-swap-services2.service (truncate + chattr +C + dd + mkswap),
  # pulled in via var-swap-services2.swap.wants/ and ordered Before= it.  The
  # helper only writes the file when its size differs from `size`, so a
  # hand-created file of exactly this size is adopted, not overwritten - which
  # is how this swap was brought up: the 16 GiB file had to exist *before* the
  # switch, because evaluating this flake is itself a job the box had no room
  # for (1.1 GiB available, and `just check` OOMs here).
  swapDevices = [
    {
      device = "/var/swap/services2";
      size = 16384; # MiB = 16 GiB, the headroom services1's 18 GiB partition gives its box
    }
  ];

  nixpkgs.hostPlatform = lib.mkDefault "x86_64-linux";
  hardware.cpu.amd.updateMicrocode = lib.mkDefault config.hardware.enableRedistributableFirmware;
}
