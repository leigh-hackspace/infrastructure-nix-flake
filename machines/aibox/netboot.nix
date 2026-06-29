{ lib, pkgs, ... }:

let
  sys = (import ../../pi-room-sys) { inherit lib pkgs; };
  build = sys.config.system.build;
  nfsServer = "aibox.int.leighhack.org";
in
{
  environment = {
    etc = {
      "tftp/ipxe.efi".source = "${pkgs.callPackage ../../common/overrides/ipxe.nix { }}/snponly.efi";
      "tftp/undionly.kpxe".source = "${pkgs.ipxe}/undionly.kpxe";
      "tftp/autoexec.ipxe".source = "${pkgs.writeText "autoexec.ipxe" ""}"; # Blank file to stop error message on boot

      "netboot.ipxe".source = pkgs.writeText "netboot.ipxe" ''
        #!ipxe

        console --x 1920 --y 1080 ||
        console --picture http://${nfsServer}/boot/logo.png ||

        # Show boot menu with timeout
        :start
        menu Boot Options
        item --gap --             ------------ Boot Options (Use UP/DOWN keys) -----------
        # item --default local      Windows 11
        # item network              Leigh Hackspace OS
        item --default network    Leigh Hackspace OS
        item local                Windows 11
        item --gap --             --------------------------------------------------------
        # choose --timeout 10000 --default local selected || goto cancel
        choose --timeout 10000 --default network selected || goto cancel
        goto ''${selected}

        :network
        kernel --name kernel  http://${nfsServer}/boot/bzImage
        initrd --name initrd0 http://${nfsServer}/boot/initrd
        boot kernel initrd=initrd0 init=${build.toplevel}/init ${lib.strings.concatStringsSep " " sys.config.boot.kernelParams}

        :local
        echo Starting Windows...
        sanboot --drive 0 --extra \\EFI\\Microsoft

        :cancel
        echo Boot cancelled, exiting to next boot device...
        exit 1
      '';
    };

    systemPackages = with pkgs; [
      ipxe
      tftp-hpa
      wol
      qemu
      OVMF
    ];
  };

  virtualisation.libvirtd = {
    enable = true;
    allowedBridges = [ "br227" ];
  };

  systemd.services = {
    tftpd = {
      after = [ "nftables.service" ];
      description = "TFTP server";
      serviceConfig = {
        User = "root";
        Group = "root";
        Restart = "always";
        RestartSec = 5;
        Type = "exec";
        ExecStart = "${pkgs.tftp-hpa}/bin/in.tftpd -l -a 10.3.1.32:69 -P /run/tftpd.pid /etc/tftp";
        TimeoutStopSec = 20;
        PIDFile = "/run/tftpd.pid";
      };
      wantedBy = [ "multi-user.target" ];
    };
  };

  services.nginx = {
    enable = true;

    recommendedProxySettings = true;
    recommendedTlsSettings = true;
    recommendedOptimisation = true;
    recommendedGzipSettings = true;

    virtualHosts = {
      "aibox.int.leighhack.org" = {
        locations = {
          "= /boot/bzImage" = {
            alias = "${build.kernel}/bzImage";
          };

          "= /boot/initrd" = {
            alias = "${build.netbootRamdisk}/initrd";
          };

          "= /boot/netboot.ipxe" = {
            alias = "/etc/netboot.ipxe";
          };

          "= /boot/logo.png" = {
            alias = "${../../pi-room-sys/leigh-logo.png}";
          };

          "/" = {
            tryFiles = "$uri $uri/ =404";
          };
        };
      };
    };
  };

  services.nfs.server = {
    enable = true;

    exports = ''
      /exports                    10.3.0.0/16(rw,fsid=0,no_subtree_check)
      /exports/nix-store          10.3.0.0/16(ro,nohide,insecure,no_subtree_check,async,no_auth_nlm)
      /exports/netboot-squashfs   10.3.0.0/16(ro,nohide,insecure,no_subtree_check)
    '';
  };

  fileSystems."/exports/nix-store" = {
    device = "/nix/store";
    fsType = "bind";
    options = [ "bind" ];
  };

  fileSystems."/exports/netboot-squashfs" = {
    device = "${build.squashfsStore}";
    fsType = "bind";
    options = [ "bind" ];
  };
}
