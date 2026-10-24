# UniFi OS Server — the appliance Ubiquiti now recommends for self-hosted
# Network controllers, run as a privileged podman container.
#
# This is a *trial* install, deliberately run alongside the existing
# `unifi`/`unifi-db` containers (machines/services1/services/unifi.nix) rather
# than in their place: every host port is offset so both controllers can be up
# at once, and nothing here touches /srv/unifi or /srv/unifi-db.  Rollback is
# `nixos-rebuild switch` without this file imported.
#
# What the artifact actually is, why the ports are what they are, and the
# in-container quirks the drop-ins below work around: docs/unifi-os-server.md.
# The runtime contract (env vars, state subdirs, drop-ins) follows
# github.com/rcambrj/unifi-os-server (MIT), which is the reference NixOS
# packaging; the extraction here is our own because it needs two tools, not
# binwalk.
#
# The image is Ubiquiti's, unfree, and *older* than the controller it is meant
# to replace: it ships Network Application 10.5.67 and MongoDB 3.6, while
# services1 runs 10.6.106 on MongoDB 4.4.  Bumping it is a manual step inside
# the container (see the doc) — the .deb Ubiquiti publishes as "for UniFi OS"
# is the update payload, and that is the other file in unify.tmp.
{
  config,
  lib,
  pkgs,
  ...
}: let
  mkOriginVhost = import ../../../common/nginx/origin-vhost.nix {inherit lib;};

  # Pin: the installer is a self-extracting ELF with the payload as a zip at
  # offset 4751312; `image.tar` is one member of it.  Both hashes below are
  # content hashes of Ubiquiti's bytes, so a silent vendor re-upload fails the
  # build rather than the migration.
  installerVersion = "5.1.42";
  imageVersion = "c9603dec9010";

  uosInstaller = pkgs.fetchurl {
    url = "https://fw-download.ubnt.com/data/unifi-os-server/5172-linux-x64-5.1.42-12e9e3cf-8f8b-4e54-928c-76b80a10c8a4.42-x64";
    sha256 = "sha256-9hEek5akLHQBb13emwH770hqb+ae/hN5Nebji3wi+U0=";
  };

  # The OCI archive, out of the installer.  bsdtar reads the zip through the
  # ELF prefix by finding the central directory, so this is a single member
  # extraction and needs no carving tool; it produces bytes identical to an
  # independent inflate of the same entry (sha256 b0a3e0a3fc2d…).
  uosImage = pkgs.stdenvNoCC.mkDerivation {
    pname = "unifi-os-server-image";
    version = installerVersion;
    src = uosInstaller;
    nativeBuildInputs = [pkgs.libarchive];
    dontUnpack = true;
    dontFixup = true;
    installPhase = ''
      runHook preInstall
      mkdir -p $out
      bsdtar -xf "$src" image.tar
      mv image.tar $out/image.tar
      runHook postInstall
    '';
    # The image name the archive carries, and what `podman load` registers.
    passthru.imageTag = "uosserver:${imageVersion}";
  };

  stateDir = "/srv/unifi-os-server";

  # The appliance runs systemd as PID 1 and its units assume a plain Debian
  # host, which a container is not.  These are the four that actually bite, as
  # drop-ins mounted into the container's /etc/systemd/system.
  fakeNtpService = pkgs.writeText "uos-fake-ntp.service" ''
    [Unit]
    Description=Fake NTP service for UniFi OS Server timedatectl checks

    [Service]
    Type=oneshot
    ExecStart=/bin/true
    RemainAfterExit=yes

    [Install]
    WantedBy=multi-user.target
  '';

  fakeNtpList = pkgs.writeText "uos-fake-ntp.list" ''
    unifi-os-server-fake-ntp.service
  '';

  # unifi-core refuses to start if its config dir is not there, and wants the
  # clock "synchronised", which a container does not have.
  ucorePreStart = pkgs.writeText "uos-unifi-core-prestart.conf" ''
    [Unit]
    Wants=unifi-os-server-fake-ntp.service
    After=unifi-os-server-fake-ntp.service

    [Service]
    ExecStartPre=-/bin/mkdir -p /data/unifi-core/config/http
  '';

  nginxPreStart = pkgs.writeText "uos-nginx-prestart.conf" ''
    [Service]
    ExecStartPre=-/bin/mkdir -p /var/log/nginx
  '';

  # mongodb's datadir is a bind mount, so it arrives owned by whoever made it
  # on the host rather than by the in-container mongodb user.
  mongoPreStart = pkgs.writeText "uos-mongodb-prestart.conf" ''
    [Service]
    ExecStartPre=+/bin/bash -c "mkdir -p /var/log/mongodb && chown mongodb:mongodb /var/log/mongodb /var/lib/mongodb"
  '';

  stateSubdirs = [
    "persistent"
    "data"
    "srv"
    "unifi"
    "mongodb"
    "log"
    "rabbitmq-ssl"
  ];

  # CUT OVER 2026-10-14: the appliance now owns the real ports and the old
  # controller is parked (see the `autoStart` overrides at the bottom).
  #
  # This is not cosmetic.  Devices are told to inform
  # `http://10.3.1.20:8080/inform`, and `system_ip` is 10.3.1.20 for both
  # controllers, so whichever process holds host 8080 owns the network.  While
  # the appliance published 18080 instead it was healthy but stone deaf: its
  # server.log had zero inform hits and every AP showed offline.  Only one of
  # the two can ever hold 8080, which is why the migration ends here rather
  # than with both running.
  #
  # 443 stays at 11443 because host 443 belongs to nginx; the old
  # `unifi-admin.int` vhost still points at 127.0.0.1:8443, which is now the
  # appliance's controller UI.
  portMappings = [
    "11443:443" # UniFi OS web UI
    "8080:8080" # device inform
    "8443:8443" # controller HTTPS
    "6789:6789" # mobile speed test
    "8880:8880" # captive portal HTTP
    "8843:8843" # captive portal HTTPS
    "28082:28082" # device support files
    "5671:5671" # traffic-flow logging
    "3478:3478/udp" # STUN
    "10001:10001/udp" # device discovery
  ];
in {
  systemd.tmpfiles.rules =
    ["d ${stateDir} 0755 root root -"]
    ++ map (d: "d ${stateDir}/${d} 0755 root root -") stateSubdirs;

  virtualisation.oci-containers.containers.unifi-os-server = {
    image = uosImage.passthru.imageTag;
    imageFile = "${uosImage}/image.tar";
    autoStart = true;
    privileged = true;
    ports = portMappings;

    # What the installer would have set on a real appliance.  APP_MODEL and
    # PRODUCT_NAME are what unifi-core keys its whole product shape off;
    # getting them wrong gives a container that boots and serves nothing.
    environment = {
      APP_MODEL = "UOSSERVER";
      APP_VERSION = installerVersion;
      PRODUCT_NAME = "uosserver";
      UOS_SERVER_VERSION = installerVersion;
      FIRMWARE_PLATFORM = "linux-x64";
    };

    volumes = [
      "${stateDir}/persistent:/persistent"
      "${stateDir}/log:/var/log"
      "${stateDir}/data:/data"
      "${stateDir}/srv:/srv"
      "${stateDir}/unifi:/var/lib/unifi"
      "${stateDir}/mongodb:/var/lib/mongodb"
      "${stateDir}/rabbitmq-ssl:/etc/rabbitmq/ssl"
      "${fakeNtpService}:/etc/systemd/system/unifi-os-server-fake-ntp.service:ro"
      "${fakeNtpList}:/etc/systemd/ntp-units.d/unifi-os-server-fake-ntp.list:ro"
      "${ucorePreStart}:/etc/systemd/system/unifi-core.service.d/prestart-fix.conf:ro"
      "${nginxPreStart}:/etc/systemd/system/nginx.service.d/prestart-fix.conf:ro"
      "${mongoPreStart}:/etc/systemd/system/mongodb.service.d/prestart-fix.conf:ro"
    ];

    extraOptions = [
      "--systemd=always"
      "--add-host=host.docker.internal:host-gateway"
      "--pids-limit=8192"
    ];
  };

  systemd.services.podman-unifi-os-server = {
    # A new image means the container is recreated, which is the only way to
    # act on a version bump.
    restartTriggers = [uosImage];

    # The appliance's identity: a UUID it stores in /data, and the address it
    # hands devices as their inform URL.  The UUID is derived from
    # /etc/machine-id rather than random so a rebuilt container keeps the same
    # identity, and pre-created so the setup wizard starts from a known state.
    preStart = lib.mkBefore ''
      uuid_file="${stateDir}/data/uos_uuid"
      if ! grep -qP '^[0-9a-f]{8}-[0-9a-f]{4}-5' "$uuid_file" 2>/dev/null; then
        ${pkgs.util-linux}/bin/uuidgen -s -n @dns -N "$(cat /etc/machine-id)" > "$uuid_file"
      fi

      system_properties="${stateDir}/unifi/system.properties"
      system_ip=10.3.1.20
      if [ -s "$system_properties" ]; then
        if ${pkgs.gnugrep}/bin/grep -q '^system_ip=' "$system_properties"; then
          ${pkgs.gnused}/bin/sed -i "s|^system_ip=.*|system_ip=$system_ip|" "$system_properties"
        else
          printf '\n%s\n' "system_ip=$system_ip" >> "$system_properties"
        fi
      else
        printf 'system_ip=%s\n' "$system_ip" > "$system_properties"
      fi
    '';
  };

  # The controller's UI, on the name it has always had.
  #
  # This vhost used to live in ./unifi.nix and proxy to the old controller's
  # 8443.  It moved here with the controller: the file that owns the controller
  # owns its front door.  Two things about it are load-bearing:
  #
  #   * `websockets` — the UI opens `wss://<window.location.host>/proxy/network/ws`,
  #     so through a proxy it needs the Upgrade dance or the dashboard never
  #     updates.  (`/proxy/network/ws` answers 401 on the appliance, which is how
  #     it was identified: every other path is the SPA's catch-all 200.)
  #   * long timeouts — nginx's 60 s proxy_read_timeout otherwise drops an idle
  #     websocket and the UI silently stops updating until you reload.
  #
  # Upstream TLS is the appliance's own self-signed cert; nginx neither verifies
  # it nor sends SNI, which is the default and is what we want.  Passing the
  # client's Host through (which `recommendedProxySettings` does) is fine: the
  # appliance answers 200 for any Host, so there is no host-validation trap here.
  services.nginx.virtualHosts = {
    "unifi-admin.int.leighhack.org" = mkOriginVhost {
      proxyPass = "https://127.0.0.1:11443";
      websockets = true;
      timeouts = "3600s";
    };
  };

  # The controller this replaced, parked rather than deleted.
  #
  # `autoStart = false` stops the units and keeps them off boot; the containers,
  # their images, /srv/unifi and /srv/unifi-db all stay exactly as they were, so
  # backing out is deleting these two lines and switching again.  Do not "tidy"
  # those directories while this block is here: they are the only way back to a
  # working controller if the appliance ever misbehaves badly.
  virtualisation.oci-containers.containers.unifi.autoStart = lib.mkForce false;
  virtualisation.oci-containers.containers.unifi-db.autoStart = lib.mkForce false;
}
