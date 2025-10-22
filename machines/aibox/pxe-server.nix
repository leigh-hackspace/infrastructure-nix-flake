pxe-server:
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
      /exports                        10.3.0.0/16(rw,fsid=0,no_subtree_check)
      /exports/pxe-server-squashfs    10.3.0.0/16(ro,nohide,insecure,no_subtree_check)
    '';
  };

  # fileSystems."/exports/pxe-server-nix-store" = {
  #   device = "${pxeServer.nixStore}/nix-store";
  #   options = [ "bind" ];
  # };

  fileSystems."/exports/pxe-server-squashfs" = {
    device = "${pxeServer.squashfsStore}";
    options = [ "bind" ];
  };

  # sudo journalctl -u pxe-server -f
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
}
