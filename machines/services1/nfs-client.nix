{
  config,
  lib,
  pkgs,
  modulesPath,
  ...
}:
{
  fileSystems."/sas-16tb/ds-downloads" = {
    device = "10.47.49.21:/sas-16tb/ds-downloads";
    fsType = "nfs4"; # Use NFSv4 for better performance
    options = [
      "rw"
      "hard"
      "intr"
      "rsize=1048576"
      "wsize=1048576"
      "proto=tcp" # Ensure TCP is used for NFS
      "acregmin=3"
      "acregmax=60"
      "acdirmin=30"
      "acdirmax=120" # Increase dir attribute cache time
      "noatime"
      "nodiratime"
      "x-systemd.after=network-online.target"
    ];
  };

  fileSystems."/samsung-4tb/ds-games" = {
    device = "10.47.49.22:/samsung-4tb/ds-games";
    fsType = "nfs4"; # Use NFSv4 for better performance
    options = [
      "rw"
      "hard"
      "intr"
      "rsize=1048576"
      "wsize=1048576"
      "proto=tcp" # Ensure TCP is used for NFS
      "acregmin=3"
      "acregmax=60"
      "acdirmin=30"
      "acdirmax=120" # Increase dir attribute cache time
      "noatime"
      "nodiratime"
      "x-systemd.after=network-online.target"
    ];
  };
}
