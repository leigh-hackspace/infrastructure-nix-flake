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

  boot.supportedFilesystems = [ "nfs" ];

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

  systemd.mounts = [
    {
      where = "/mnt/cameras";
      what = "10.3.1.6:/mnt/sas-10k/cameras";
      type = "nfs";
      options = "nfsvers=4.2,_netdev,x-systemd.automount,retry=5,timeo=5,x-systemd.mount-timeout=30";
      after = [ "wait-for-network.service" ];
      requires = [ "wait-for-network.service" ];
    }
    {
      where = "/mnt/filestore";
      what = "10.3.1.6:/mnt/sas-10k/filestore";
      type = "nfs";
      options = "nfsvers=4.2,_netdev,x-systemd.automount,retry=5,timeo=5,x-systemd.mount-timeout=30";
      after = [ "wait-for-network.service" ];
      requires = [ "wait-for-network.service" ];
    }
    {
      where = "/mnt/backups";
      what = "10.3.1.6:/mnt/sas-10k/backups";
      type = "nfs";
      options = "nfsvers=4.2,_netdev,x-systemd.automount,retry=5,timeo=5,x-systemd.mount-timeout=30";
      after = [ "wait-for-network.service" ];
      requires = [ "wait-for-network.service" ];
    }
  ];

  swapDevices = [ ];

  nixpkgs.hostPlatform = lib.mkDefault "x86_64-linux";
  hardware.cpu.intel.updateMicrocode = lib.mkDefault config.hardware.enableRedistributableFirmware;
}
