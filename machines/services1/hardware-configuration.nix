{
  config,
  lib,
  pkgs,
  modulesPath,
  ...
}:

{
  imports = [
    (modulesPath + "/installer/scan/not-detected.nix")
  ];

  boot.initrd.availableKernelModules = [
    "xhci_pci"
    "ahci"
    "usbhid"
    "usb_storage"
    "sd_mod"
    "sr_mod"
  ];
  boot.initrd.kernelModules = [ "i915" ];
  boot.kernelModules = [ "kvm-intel" ];
  boot.extraModulePackages = [ ];

  boot.kernelParams = [
    "i915.enable_guc=2"
    "mitigations=off"
  ];

  boot.kernel.sysctl = {
    "kernel.task_delayacct" = 1;
  };

  hardware.graphics = {
    enable = true;
    extraPackages = with pkgs; [
      intel-media-driver
      libvdpau-va-gl
    ];
  };

  environment.sessionVariables = {
    LIBVA_DRIVER_NAME = "iHD";
  };

  fileSystems."/" = {
    device = "/dev/disk/by-uuid/b1bf35fa-af75-4e31-a88c-d3edda4c39a4";
    fsType = "ext4";
  };

  fileSystems."/boot" = {
    device = "/dev/disk/by-uuid/E026-09AD";
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
  infra.nas.exports = [
    { where = "/mnt/cameras"; share = "/mnt/sas-10k/cameras"; }
    { where = "/mnt/filestore"; share = "/mnt/sas-10k/filestore"; }
    { where = "/mnt/backups"; share = "/mnt/sas-10k/backups"; }
  ];

  swapDevices = [ ];

  nixpkgs.hostPlatform = lib.mkDefault "x86_64-linux";
  hardware.cpu.intel.updateMicrocode = lib.mkDefault config.hardware.enableRedistributableFirmware;
}
