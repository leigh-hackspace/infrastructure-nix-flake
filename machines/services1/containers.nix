{ config, lib, pkgs, modulesPath, ... }:

let
  update-containers = (pkgs.writeShellScriptBin "update-containers" ''
    images=$(${pkgs.podman}/bin/podman ps -a --format="{{.Image}}" | sort -u)

    for image in $images
    do
      ${pkgs.podman}/bin/podman pull $image
    done

    ${pkgs.systemd}/bin/systemctl restart podman-*
  '');
in
{
  systemd.timers = {
    update-containers = {
      timerConfig = {
        Unit = "update-containers.service";
        OnCalendar = "Mon 02:00";
      };
      wantedBy = [ "timers.target" ];
    };
  };
  systemd.services = {
    update-containers = {
      serviceConfig = {
        Type = "oneshot";
        ExecStart = "${update-containers}/bin/update-containers";
      };
    };
  };

  virtualisation.podman = {
    enable = true;
    autoPrune.enable = true;
    dockerCompat = true;
    dockerSocket.enable = true;
    defaultNetwork.settings.dns_enabled = true;
  };

  virtualisation.oci-containers.backend = "podman";

  virtualisation.oci-containers.containers = {
    # sudo smem -r | sort -k 4 -nr | head
    # sudo smem -rs swap | head
    frigate = {
      hostname = "frigate";
      image = "ghcr.io/blakeblackshear/frigate:0.16.0-beta3";
      autoStart = true;
      volumes = [
        # "/srv/frigate/storage:/media/frigate"
        "/mnt/cameras/storage:/media/frigate"
        "/srv/frigate/config:/config"
        "/srv/frigate/certs:/etc/letsencrypt/live/frigate:ro"
      ];
      ports = [
        "5000:5000" # Insecure
        "8971:8971" # Secure
        "1984:1984" # go2rtc admin panel
        "8554:8554"
        "8555:8555/tcp"
        "8555:8555/udp"
      ];
      environment = {
        FRIGATE_RTSP_PASSWORD = "password";
      };
      extraOptions = [
        "--mount=type=tmpfs,destination=/tmp/cache,tmpfs-size=1000000000"
        "--device=/dev/dri/renderD128"
        "--device=/dev/apex_0:/dev/apex_0"
        "--shm-size=1024m"
        "--cap-add=CAP_PERFMON"
        "--privileged"
      ];
    };

    zigbee2mqtt = {
      hostname = "zigbee2mqtt";
      image = "koenkk/zigbee2mqtt:latest-dev";
      autoStart = true;
      ports = [
        "8080:8080"
      ];
      volumes = [
        "/srv/zigbee2mqtt:/app/data"
        "/run/udev:/run/udev:ro"
      ];
      environment = {
        TZ = "Europe/London";
      };
      extraOptions = [
        "--device=/dev/ttyUSB0"
      ];
    };

    mqtt = {
      hostname = "mqtt";
      image = "docker.io/eclipse-mosquitto";
      autoStart = true;
      ports = [
        "1883:1883"
      ];
      volumes = [
        "/srv/mosquitto:/mosquitto"
      ];
      environment = {
        TZ = "Europe/London";
      };
      extraOptions = [
        #        "--pull=always"
      ];
    };
  };
}
