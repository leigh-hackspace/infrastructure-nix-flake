{
  lib,
  pkgs,
  specialArgs,
  INFRA,
  ...
}: let
  # The Pi Room image, as a built nixosSystem.  It is this flake's own
  # `nixosConfigurations.pi-room-sys` (machines/pi-room-sys/README.md), handed to
  # this module through specialArgs by flake.nix because an attrset cannot pass a
  # sibling to its own member.
  sys = specialArgs.piRoomSys;
  build = sys.config.system.build;
  # Read, not retyped: this box's own address, which is what the TFTP server binds and
  # what the router's `dhcp-boot=` lines already name.  `common/hosts.nix` is the table
  # both halves read — machines/gw/dns.nix: "the server address is read from the table
  # rather than written in either place".
  HOSTS = import ../../common/hosts.nix;
  tftpAddr = HOSTS.aibox;
  nfsServer = "aibox.int.leighhack.org";
in {
  environment = {
    etc = {
      "tftp/ipxe.efi".source = "${pkgs.callPackage ../../common/ipxe.nix {}}/snponly.efi";
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

        # `init=` names the image's own closure through a symlink the image
        # carries (machines/pi-room-sys/netboot.nix's `pseudoFiles`), not a store
        # path.  A store path here is a *generation*, and the client does not
        # necessarily mount the store image of the generation whose menu it
        # fetched: aibox swaps both on `nixos-rebuild switch`, and a client that
        # chains loaded just before the switch and mounts the export just after
        # it used to die in `initrd-find-nixos-closure`
        # (`/nix/store/<old-init>/init: No such file or directory`) — on
        # 2026-10-21 that looked exactly like an NFS root failure and was not.
        # A constant path resolves inside whichever image was mounted.
        :network
        kernel --name kernel  http://${nfsServer}/boot/bzImage
        initrd --name initrd0 http://${nfsServer}/boot/initrd
        boot kernel initrd=initrd0 init=/nix/store/.netboot-closure/init ${lib.strings.concatStringsSep " " sys.config.boot.kernelParams}

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
    allowedBridges = ["br227"];
  };

  # The TFTP server, on the address the router names in every `dhcp-boot=` line.
  #
  # The ordering and the bounded wait are both about one measured failure: at boot the
  # address comes from DHCP on vlan225, `systemd.network.wait-online` is disabled on
  # this box (machines/aibox/networking.nix), and `in.tftpd -a <addr>:69` exits 71 with
  # `cannot bind to local IPv4 socket: Cannot assign requested address` when it wins the
  # race.  Measured 2026-10-23 19:23:39 — three restarts, running by 19:23:44 — and the
  # box only stayed up because `INFRA.mkNeverGiveUp` retries forever.  A PXE attempt
  # inside that window got no answer at all, which is indistinguishable from a dead
  # netboot server, so the point is to make the *first* start succeed rather than the
  # fourth.  45 s, deliberately under systemd's default start timeout, and it fails
  # loudly rather than binding something else.
  #
  # `-v` is there to answer an open question, not to decorate the journal: whether a
  # client ever fetches `/etc/tftp/ipxe.efi` or falls back to the blank
  # `autoexec.ipxe` because `in.tftpd 5.2` refuses its option negotiation (a UEFI client
  # was seen retrying `tsize`/`blksize`/`windowsize` then taking the fallback on
  # 2026-10-26, and that fallback is why the empty file is load-bearing).
  # `docs/follow-ups.md` owns the question; this is what lets the next board's boot
  # answer it from the journal instead of needing a capture at exactly the right moment.
  systemd.services = {
    tftpd = INFRA.mkNeverGiveUp {
      after = [
        "nftables.service"
        "sys-subsystem-net-devices-vlan225.device"
      ];
      requires = ["sys-subsystem-net-devices-vlan225.device"];
      description = "TFTP server";
      serviceConfig = {
        User = "root";
        Group = "root";
        Type = "exec";
        # No double quotes inside: this is a Nix double-quoted string, and the whole
        # argument to `bash -c` is single-quoted for systemd, so bash sees plain words.
        # `ip` is spelled absolutely because a NixOS unit's PATH is the five packages
        # named in `path=:` below — coreutils, findutils, gnugrep, gnused, systemd — and
        # `ip` is not among them: with a bare `ip` this gate fails 45 times, exits 1, and
        # TFTP never starts, which is a worse version of the bug it fixes. Read back from
        # the rendered unit (AGENTS.md rule 10), not assumed.
        ExecStartPre = "+${pkgs.bash}/bin/bash -c 'for i in $(seq 45); do ${pkgs.iproute2}/bin/ip -4 addr show dev vlan225 2>/dev/null | grep -q ${tftpAddr}/ && exit 0; sleep 1; done; echo tftpd: ${tftpAddr} never appeared on vlan225 1>&2; exit 1'";
        ExecStart = "${pkgs.tftp-hpa}/bin/in.tftpd -l -v -a ${tftpAddr}:69 -P /run/tftpd.pid /etc/tftp";
        TimeoutStopSec = 20;
        PIDFile = "/run/tftpd.pid";
      };
      wantedBy = ["multi-user.target"];
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
            alias = "${../pi-room-sys/leigh-logo.png}";
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
    options = ["bind"];
  };

  fileSystems."/exports/netboot-squashfs" = {
    device = "${build.squashfsStore}";
    fsType = "bind";
    options = ["bind"];
  };
}
