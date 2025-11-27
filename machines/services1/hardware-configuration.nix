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

  systemd.services.wait-for-nfs-server = {
    description = "Wait for NFS server to be reachable";
    after = [ "network-online.target" ];
    wants = [ "network-online.target" ];
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
      ExecStart = "${pkgs.bash}/bin/bash -c 'until ${pkgs.iputils}/bin/ping -c1 -W2 10.3.1.6 >/dev/null 2>&1; do sleep 2; done'";
      TimeoutStartSec = 30;
    };
  };

  systemd.mounts = [
    {
      where = "/mnt/cameras";
      what = "10.3.1.6:/mnt/sas-10k/cameras";
      type = "nfs";
      options = "nfsvers=4.2,_netdev,x-systemd.automount,retry=5,timeo=5,x-systemd.mount-timeout=30";
      after = [ "wait-for-nfs-server.service" ];
      requires = [ "wait-for-nfs-server.service" ];
    }
    {
      where = "/mnt/filestore";
      what = "10.3.1.6:/mnt/sas-10k/filestore";
      type = "nfs";
      options = "nfsvers=4.2,_netdev,x-systemd.automount,retry=5,timeo=5,x-systemd.mount-timeout=30";
      after = [ "wait-for-nfs-server.service" ];
      requires = [ "wait-for-nfs-server.service" ];
    }
    {
      where = "/mnt/backups";
      what = "10.3.1.6:/mnt/sas-10k/backups";
      type = "nfs";
      options = "nfsvers=4.2,_netdev,x-systemd.automount,retry=5,timeo=5,x-systemd.mount-timeout=30";
      after = [ "wait-for-nfs-server.service" ];
      requires = [ "wait-for-nfs-server.service" ];
    }
  ];

  swapDevices = [ ];

  nixpkgs.hostPlatform = lib.mkDefault "x86_64-linux";
  hardware.cpu.intel.updateMicrocode = lib.mkDefault config.hardware.enableRedistributableFirmware;
}
