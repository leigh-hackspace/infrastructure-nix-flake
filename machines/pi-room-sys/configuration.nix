{
  config,
  lib,
  pkgs,
  ...
}: let
  homeServer = "10.3.1.6";

  # Login script - runs when user logs in
  loginScript = pkgs.writeScript "pam-login" ''
    #!/bin/sh

    # Available variables from PAM:
    # PAM_USER - username
    # PAM_RHOST - remote host (for SSH)
    # PAM_SERVICE - service name (sshd, login, etc)
    # PAM_TTY - terminal
    # PAM_TYPE - session open/close

    logger "PAM Login: user=$PAM_USER service=$PAM_SERVICE from=$PAM_RHOST"
    logger "whoami=$(whoami)"

    # Get user info
    USER_HOME=$(getent passwd "$PAM_USER" | cut -d: -f6)
    USER_UID=$(id -u "$PAM_USER")
    USER_GID=$(id -g "$PAM_USER")

    # Mount NFS share for user
    MOUNT_POINT="$USER_HOME/Filestore"

    # Create mount point if it doesn't exist
    if [ ! -d "$MOUNT_POINT" ]; then
      logger "MKDIR"
      mkdir -p "$MOUNT_POINT"
    fi

    # Mount SMB share
    logger "MOUNTING USER_HOME=$USER_HOME MOUNT_POINT=$MOUNT_POINT USER_UID=$USER_UID USER_GID=$USER_GID"

    logger "mount -t cifs -o user=hackspacer,pass=caffeine1234,uid=$USER_UID,gid=$USER_GID //${homeServer}/filestore $MOUNT_POINT"

    mount -t cifs -o user=hackspacer,pass=caffeine1234,uid=$USER_UID,gid=$USER_GID //${homeServer}/filestore $MOUNT_POINT

    logger "DONE MOUNT"

    # ${pkgs.shadow}/bin/usermod -aG dialout "$PAM_USER" 2>/dev/null || true

    # logger "DONE ADD GROUP"

    exit 0
  '';

  # Logout script - runs when user logs out
  logoutScript = pkgs.writeScript "pam-logout" ''
    #!/bin/sh

    logger "PAM Logout: user=$PAM_USER service=$PAM_SERVICE"

    # Get user info
    USER_HOME=$(getent passwd "$PAM_USER" | cut -d: -f6)
    MOUNT_POINT="$USER_HOME/Filestore"

    # Unmount SMB share
    if mountpoint -q "$MOUNT_POINT"; then
      # Check if any processes are using the mount
      if ! fuser -m "$MOUNT_POINT" 2>/dev/null; then
        umount "$MOUNT_POINT" 2>/dev/null

        if [ $? -eq 0 ]; then
          logger "PAM Logout: Unmounted SMB share for $PAM_USER"
        else
          logger "PAM Logout: Failed to unmount SMB share for $PAM_USER"
        fi
      else
        logger "PAM Logout: Cannot unmount $MOUNT_POINT - still in use by $PAM_USER"
      fi
    fi

    exit 0
  '';
in {
  imports = [../../common/auth-posix.nix];

  networking.hostName = "pxeclient";

  boot.kernelPackages = pkgs.linuxPackages_latest;

  systemd.sleep.settings.Sleep = {
    AllowSuspend = "no";
    AllowHibernation = "no";
    AllowHybridSleep = "no";
    AllowSuspendThenHibernate = "no";
  };

  boot.kernelParams = [
    "mitigations=off"
    "plymouth.use-simpledrm"
    "quiet"
  ];

  boot.plymouth = {
    enable = true;
    theme = "solar";
    logo = ./leigh-logo.png;
  };

  nix.settings.experimental-features = [
    "nix-command"
    "flakes"
  ];

  systemd.services.immediate-shutdown = {
    description = "Immediate Shutdown Script";

    before = [
      "shutdown.target"
      "reboot.target"
      "halt.target"
    ];

    requiredBy = [
      "halt.target"
      "reboot.target"
      "shutdown.target"
    ];

    unitConfig = {
      DefaultDependencies = "no";
    };

    serviceConfig = {
      Type = "oneshot";
      ExecStart = "${lib.getExe (
        pkgs.writeShellScriptBin "immediate-shutdown" ''
          sync
          echo b > /proc/sysrq-trigger
        ''
      )}";
    };

    enable = true;
  };

  hardware.graphics = {
    enable = true;
    enable32Bit = true;
    extraPackages = with pkgs; [
      intel-media-driver # LIBVA_DRIVER_NAME=iHD
      intel-vaapi-driver # LIBVA_DRIVER_NAME=i965 (older but works better for Firefox/Chromium)
      libvdpau-va-gl
    ];
  };

  hardware.firmware = [pkgs.linux-firmware];

  # Enable sound with pipewire.
  services.pulseaudio.enable = false;
  security.rtkit.enable = true;
  services.pipewire = {
    enable = true;
    alsa.enable = true;
    alsa.support32Bit = true;
    pulse.enable = true;
  };

  services.xserver = {
    enable = true;
    # Configure keymap in X11
    xkb = {
      layout = "gb";
      variant = "";
    };
  };

  services.displayManager.sddm = {
    enable = true;
    wayland.enable = true;
  };

  # Enable the KDE Plasma Desktop Environment.
  services.desktopManager.plasma6.enable = true;
  # services.desktopManager.gnome.enable = true;

  services.openssh = {
    enable = true;
    settings.PermitRootLogin = "yes";
    hostKeys = [
      {
        bits = 256;
        path = "/var/secrets/ssh_host_ed25519_key";
        type = "ed25519";
      }
    ];
  };

  system.activationScripts.addHostKey = ''
    mkdir -p /var/secrets
    cat ${./ssh_host_ed25519_key} > /var/secrets/ssh_host_ed25519_key
    chmod 0400 /var/secrets/ssh_host_ed25519_key
  '';

  programs.firefox.enable = true;

  programs.nix-ld.enable = true;

  # Set your time zone.
  time.timeZone = "Europe/London";

  # Select internationalisation properties.
  i18n.defaultLocale = "en_GB.UTF-8";

  i18n.extraLocaleSettings = {
    LC_ADDRESS = "en_GB.UTF-8";
    LC_IDENTIFICATION = "en_GB.UTF-8";
    LC_MEASUREMENT = "en_GB.UTF-8";
    LC_MONETARY = "en_GB.UTF-8";
    LC_NAME = "en_GB.UTF-8";
    LC_NUMERIC = "en_GB.UTF-8";
    LC_PAPER = "en_GB.UTF-8";
    LC_TELEPHONE = "en_GB.UTF-8";
    LC_TIME = "en_GB.UTF-8";
  };

  # Configure console keymap
  console.keyMap = "uk";

  # Remote home can be slow. NFSv4 causes KDE desktop to be unstable so using NFSv3
  fileSystems."/home" = {
    fsType = "nfs";
    device = "${homeServer}:/mnt/sas-10k/ds-home";
    options = [
      "nfsvers=3"
      "rsize=1048576"
      "hard"
      "nocto"
      "noatime"
      "actimeo=86400"
      "nconnect=16"
      "noacl"
      "fsc"
      "lookupcache=all"
      "actimeo=86400"
      "nolock"
    ];
  };

  # Give the root user a hashed password
  users.users.root = {
    initialHashedPassword = "$y$j9T$./XbJUW/kdpt.HyqflLNA/$j3ttxvFfwFL2YjiILTjubR8MMZINk5O1JcPHQD2I.I5";
    openssh.authorizedKeys.keys = [
      "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFGBmtfOyJYWOoWMiZljl+XArMepgIZQ+D1ZhoUqOhT7 admin@leighhack.org"
      "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJFQDmak7ApelH4ybyNm2Sa86vOP3kakBUR6g05eSqJJ leigh-admin@aibox"
    ];
  };

  # Don't require sudo/root to `reboot` or `poweroff`.
  security.polkit.enable = true;

  # Allow passwordless sudo from nixos user
  security.sudo = {
    enable = true;
    wheelNeedsPassword = false;
  };

  environment.systemPackages = with pkgs; [
    just
    git
    openldap # for ldapsearch
    google-chrome
    libreoffice

    freecad
    kicad
    librecad
    qcad

    (vscode.fhsWithPackages (ps: [
      ps.git
      ps.avrdude
      ps.platformio-core
      ps.mklittlefs
      (pkgs.python3.withPackages (ps: [ps.intelhex]))
    ]))
  ];

  services.udev.packages = [
    pkgs.platformio-core
    pkgs.openocd
  ];

  # authentik as the POSIX identity source: the sssd domain, the CA it pins, the sudo rule.
  # One module for this and for machines/aibox/sso.nix, which had drifted (cache_credentials,
  # debug_level).  Read `common/auth-posix.nix`'s header before touching any of it: the
  # uid/gid numbers this box logs in with are computed from authentik database keys and are
  # stored nowhere, and they are what owns this user's files on the NAS.
  infra.authPosix = {
    enable = true;
    # Public by construction: this image is a squashfs anyone on the Pi Room VLAN can PXE
    # and mount.  The plan to make the credential unnecessary is docs/auth-posix-logins.md.
    bindToken = "pgina";
  };

  # The module sets makeHomeDir on sddm/login/sshd; only this image's own session hooks
  # (mount the sharer's filestore at login) live here.
  security.pam.services = {
    sddm = {
      text = ''
        # Account management.
        account include login # login (order 10100)

        # Authentication management.
        auth substack login # login (order 10100)

        # Password management.
        password substack login # login (order 10100)

        # Session management.
        session include login # login (order 10100)

        #### The above is the default. Below we are adding ####

        session optional ${pkgs.pam}/lib/security/pam_exec.so stdout seteuid type=open_session ${loginScript}
        session optional ${pkgs.pam}/lib/security/pam_exec.so stdout seteuid type=close_session ${logoutScript}
      '';
    };
  };

  services.udev.extraRules = ''
    KERNEL=="ttyACM*", MODE="0666"
    KERNEL=="ttyUSB*", MODE="0666"
  '';

  # Keep local users working
  users.mutableUsers = true;

  environment.plasma6.excludePackages = [pkgs.kdePackages.baloo];

  system.stateVersion = config.system.nixos.release;
}
