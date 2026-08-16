{pkgs, ...}: {
  # fileSystems."/sas-16tb/ds-downloads" = {
  #   device = "10.47.49.21:/sas-16tb/ds-downloads";
  #   fsType = "nfs4"; # Use NFSv4 for better performance
  #   options = [
  #     "rw"
  #     "hard"
  #     "intr"
  #     "rsize=1048576"
  #     "wsize=1048576"
  #     "proto=tcp" # Ensure TCP is used for NFS
  #     "acregmin=3"
  #     "acregmax=60"
  #     "acdirmin=30"
  #     "acdirmax=120" # Increase dir attribute cache time
  #     "noatime"
  #     "nodiratime"
  #     "x-systemd.after=network-online.target"
  #   ];
  # };

  # fileSystems."/samsung-4tb/ds-games" = {
  #   device = "10.47.49.22:/samsung-4tb/ds-games";
  #   fsType = "nfs4"; # Use NFSv4 for better performance
  #   options = [
  #     "rw"
  #     "hard"
  #     "intr"
  #     "rsize=1048576"
  #     "wsize=1048576"
  #     "proto=tcp" # Ensure TCP is used for NFS
  #     "acregmin=3"
  #     "acregmax=60"
  #     "acdirmin=30"
  #     "acdirmax=120" # Increase dir attribute cache time
  #     "noatime"
  #     "nodiratime"
  #     "x-systemd.after=network-online.target"
  #   ];
  # };

  # Wait for the NAS (10.3.1.6) to be up *and* for its NFS exports to be
  # genuinely mounted before NAS-dependent services start.
  #
  # The mounts themselves are lazy (x-systemd.automount in
  # hardware-configuration.nix), so simply pinging the NAS is not enough:
  # TrueNAS can answer ping long before NFS exports are ready.  This service
  # keeps triggering the automounts until `findmnt` reports a real nfs*
  # filesystem instead of an idle autofs mount, then exits successfully.
  #
  # Services that need the NAS should declare:
  #
  #   systemd.services.<svc> = {
  #     after = [ "wait-for-nas.service" ];
  #     requires = [ "wait-for-nas.service" ];
  #   };
  #
  systemd.services.wait-for-nas = {
    description = "Wait for NAS mounts to become available";
    wantedBy = ["multi-user.target"];
    after = [
      "network-online.target"
      "mnt-cameras.automount"
      "mnt-filestore.automount"
      "mnt-backups.automount"
    ];
    wants = ["network-online.target"];
    path = [pkgs.util-linux];

    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
      # Never give up: keep waiting for as long as the NAS takes to come back.
      TimeoutStartSec = 0;
      ExecStart = pkgs.writeShellScript "wait-for-nas" ''
        for m in /mnt/cameras /mnt/filestore /mnt/backups; do
          # `ls` on an automount point makes systemd attempt the real mount.
          # While the NAS (or its NFS exports) are still coming up the
          # attempt fails quickly; retrigger until the share is mounted.
          until [ "$(findmnt -n -o FSTYPE "$m" 2>/dev/null)" != "autofs" ] \
             && [ -n "$(findmnt -n -o FSTYPE "$m" 2>/dev/null)" ]; do
            ls "$m" >/dev/null 2>&1 || true
            sleep 2
          done
        done
      '';
    };
  };
}
