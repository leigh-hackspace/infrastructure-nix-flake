# This module creates netboot media containing the given NixOS configuration.
{nfsServer}: {
  config,
  lib,
  pkgs,
  modulesPath,
  ...
}:
with lib; let
  # Create the directory that contains the Nix store.
  #
  # NOTE: `./make-store-dir.nix` does not exist and never did in this tree — it is
  # left over from the nixpkgs netboot example this module was cut from.  It is
  # harmless only because nothing ever forces `system.build.nixStore` below (the
  # squashfs variant is what aibox exports), and Nix never evaluates the `let`
  # binding, so the missing path literal is never copied.  If anything ever wants
  # `system.build.nixStore`, this is the line that fails, not the move.
  nixStore = pkgs.callPackage ./make-store-dir.nix {
    # Closures to be copied to the Nix store, namely the init
    # script and the top-level system configuration directory.
    storeContents = [config.system.build.toplevel];
  };

  # Alternative: Create the squashfs image that contains the Nix store.
  squashfsStore = pkgs.callPackage ./make-squashfs.nix {
    storeContents = [config.system.build.toplevel];
    comp = "zstd -Xcompression-level 10";
    hydraBuildProduct = true;
  };
in {
  imports = [
    (modulesPath + "/profiles/base.nix")
  ];

  documentation.man.enable = lib.mkOverride 500 true;
  hardware.enableRedistributableFirmware = lib.mkOverride 70 false;
  system.extraDependencies = lib.mkOverride 70 [];
  networking.networkmanager.enable = lib.mkOverride 500 false;

  hardware.enableAllHardware = true;

  # Don't build the GRUB menu builder script, since we don't need it
  # here and it causes a cyclic dependency.
  boot.loader.grub.enable = false;

  boot.kernelParams = [];

  networking.useDHCP = lib.mkForce true;

  boot.initrd = {
    network = {
      enable = true;
      flushBeforeStage2 = false; # otherwise NFS dosen't work

      ssh = {
        enable = true;
        authorizedKeys = [
          "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFGBmtfOyJYWOoWMiZljl+XArMepgIZQ+D1ZhoUqOhT7 admin@leighhack.org"
          "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJFQDmak7ApelH4ybyNm2Sa86vOP3kakBUR6g05eSqJJ leigh-admin@aibox"
        ];
        hostKeys = [
          # A private host key, in the open, because it has to be a real file at
          # build time and it has been committed here since this tree existed.  It
          # belongs in sops (rendered by config.sopsSecretText, common/sops.nix)
          # and then rotated — anyone who can netboot can read this initrd.  See
          # docs/follow-ups.md.
          ./ssh_host_ed25519_key
        ];
      };
    };

    supportedFilesystems = {
      nfs = true;
      nfsv4 = true;
      zfs = false;
    };

    availableKernelModules = [
      "squashfs"
      "overlay"
      "nfs"
      "nfsv4"
      "r8169"
      "e1000"
      "e1000e"
      "igb"
      "ixgbe"
      "tg3"
      "bnx2"
      "bnx2x"
    ];

    kernelModules = [
      "loop"
      "overlay"
      "i915"
      "qxl"
    ];

    systemd = {
      initrdBin = with pkgs; [
        fuse
        nfs-utils
        iputils
        iproute2
      ];

      network.wait-online.extraArgs = ["-4"]; # Need to wait for IPv4 to be ready so NFS mounts

      emergencyAccess = true;

      services.plymouth-start = {
        after = ["systemd-modules-load.service"];
      };
    };
  };

  boot.supportedFilesystems.zfs = false;

  boot.kernel.sysctl = {
    # TCP buffer sizes
    "net.core.rmem_max" = 134217728;
    "net.core.wmem_max" = 134217728;
    "net.ipv4.tcp_rmem" = "4096 87380 134217728";
    "net.ipv4.tcp_wmem" = "4096 65536 134217728";
    "net.core.netdev_max_backlog" = 5000;

    # Readahead buffering
    "vm.dirty_ratio" = 40;
    "vm.dirty_background_ratio" = 10;
  };

  fileSystems."/" = mkImageMediaOverride {
    fsType = "tmpfs";
    options = ["mode=0755"];
  };

  fileSystems."/nix/netboot-squashfs" = mkImageMediaOverride {
    fsType = "nfs4";
    device = "${nfsServer}:/netboot-squashfs";
    options = [
      "ro"
      "vers=4.2"
      "rsize=1048576"
      "hard"
      "intr"
      "nocto"
      "noatime"
      "actimeo=86400"
      "_netdev"
    ];
    neededForBoot = true;
  };

  fileSystems."/nix/.ro-store" = mkImageMediaOverride {
    depends = [
      "/nix/netboot-squashfs"
    ];
    fsType = "squashfs";
    device = "/sysroot/nix/netboot-squashfs/squashfs.squashfs";
    options = [
      "loop"
      "threads=multi"
    ];
    neededForBoot = true;
  };

  # fileSystems."/nix/.ro-store" = mkImageMediaOverride {
  #   fsType = "nfs4";
  #   device = "${nfsServer}:/nix-store";
  #   options = [
  #     "ro"
  #     "vers=4.2"
  #     "rsize=1048576"
  #     "hard"
  #     "intr"
  #     "nocto"
  #     "noatime"
  #     "actimeo=86400"
  #     "_netdev"
  #     "nconnect=16"
  #     "noacl"
  #     "fsc"
  #     "lookupcache=all"
  #     "actimeo=86400"
  #     "nolock"
  #   ];
  #   neededForBoot = true;
  # };

  fileSystems."/nix/.rw-store" = mkImageMediaOverride {
    fsType = "tmpfs";
    options = ["mode=0755"];
    neededForBoot = true;
  };

  fileSystems."/nix/store" = mkImageMediaOverride {
    overlay = {
      lowerdir = ["/nix/.ro-store"];
      upperdir = "/nix/.rw-store/store";
      workdir = "/nix/.rw-store/work";
    };
    neededForBoot = true;
  };

  system.build.nixStore = nixStore;
  system.build.squashfsStore = squashfsStore;

  # Create the initrd
  system.build.netbootRamdisk = pkgs.makeInitrdNG {
    inherit (config.boot.initrd) compressor compressorArgs;
    prepend = ["${config.system.build.initialRamdisk}/initrd"];
    contents = [];
  };

  boot.loader.timeout = 10;

  boot.postBootCommands = ''
    # After booting, register the contents of the Nix store
    # in the Nix database in the tmpfs.
    ${config.nix.package}/bin/nix-store --load-db < /nix/store/nix-path-registration

    # nixos-rebuild also requires a "system" profile and an
    # /etc/NIXOS tag.
    touch /etc/NIXOS
    ${config.nix.package}/bin/nix-env -p /nix/var/nix/profiles/system --set /run/current-system

    # Set password for user nixos if specified on cmdline
    # Allows using nixos-anywhere in headless environments
    for o in $(</proc/cmdline); do
      case "$o" in
        live.nixos.passwordHash=*)
          set -- $(IFS==; echo $o)
          ${pkgs.gnugrep}/bin/grep -q "root::" /etc/shadow && ${pkgs.shadow}/bin/usermod -p "$2" root
          ;;
        live.nixos.password=*)
          set -- $(IFS==; echo $o)
          ${pkgs.gnugrep}/bin/grep -q "root::" /etc/shadow && echo "root:$2" | ${pkgs.shadow}/bin/chpasswd
          ;;
      esac
    done
  '';
}
