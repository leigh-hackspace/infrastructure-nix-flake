{ pxe-server, ... }:

{ config, pkgs, ... }:

let
  pxeServer = (pxe-server.pxeServer { nfsServer = "10.3.14.32"; });
in
{
  # journalctl -u nfs-server.service -f
  # journalctl -u nfs-mountd.service -f
  services.nfs.server = {
    enable = true;
    exports = ''
      /exports                          10.3.0.0/16(rw,fsid=0,no_subtree_check)
      /exports/pxe-server-squashfs      10.3.0.0/16(ro,nohide,insecure,no_subtree_check)
      /exports/pxe-server-squashfs-min  10.3.0.0/16(ro,nohide,insecure,no_subtree_check)
    '';
  };

  ## Test NFS is working...
  # sudo mount -t nfs4      10.3.14.32:/pxe-server-squashfs              /mnt/pxe-server-squashfs
  # sudo mount -t squashfs  /mnt/pxe-server-squashfs/squashfs.squashfs   /mnt/pxe-server-nix-store
  fileSystems."/exports/pxe-server-squashfs" = {
    device = "${pxeServer.squashfsStore}";
    fsType = "bind";
    options = [ "bind" ];
  };

  fileSystems."/exports/pxe-server-squashfs-min" = {
    device = "${pxeServer.squashfsStoreMin}";
    fsType = "bind";
    options = [ "bind" ];
  };

  # journalctl -u pxe-server -f
  # systemctl cat pxe-server
  systemd.services.pxe-server = {
    description = "PXE Server";
    requires = [
      "network.target"
    ];

    # Ensure the service is started at boot
    wantedBy = [ "multi-user.target" ];

    # Executable from your flake, running with systemd
    serviceConfig = {
      ExecStart = "${pxeServer.pixiecoreServer}";
      Restart = "always";
      RestartSec = 5;
    };
  };

  environment.systemPackages = with pkgs; [ qemu ];

  virtualisation.libvirtd = {
    enable = true;
    allowedBridges = [ "br227" ];
  };
}

## Launch a VM to test PXE boot
# qemu-system-x86_64 -m 2048 -netdev tap,id=net0,br=br227,helper=$(type -p qemu-bridge-helper) -device virtio-net-pci,netdev=net0 -display vnc=:0 -vga qxl
