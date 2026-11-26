{
  config,
  lib,
  ...
}: let
  CONFIG = import ../../../common/config.nix;
  ZIGBEE2MQTT_UID = 8124;
  mkOriginVhost = import ../../../common/nginx/origin-vhost.nix {inherit lib;};

  # The dongle, named by the table rather than by the number the kernel handed
  # out. `common/radio-tables.nix` is one implementation of the rules over
  # `common/radio-dongles.nix`, asserted at three entry points: `just check-eval`
  # (`common/check-radio-tables.nix`), `nix flake check` (the `radio-tables` flake
  # check), and here plus the two services that read the same table — a deploy of
  # the box that runs the radios cannot pass them with `just check` skipped. The
  # fault this refuses is not hypothetical here: this unit is the live Zigbee
  # coordinator (an SMLIGHT SLZB-06M, `advanced: channel: 11`, the space's mesh),
  # and `--device=/dev/ttyUSB0` is a name the kernel re-hands in **plug order on
  # every reboot**, so a Thread radio or a second dongle in another port silently
  # moves this coordinator onto a different chip — zigbee2mqtt starts, its
  # coordinator answers, and every Zigbee entity in Home Assistant keeps arriving
  # over MQTT from a mesh nobody is maintaining any more. That is the fault
  # `machines/services1/services/home-assistant.nix` calls "two Zigbee stacks on
  # one coordinator", and it is why HA passes `/dev/ttyUSB0` by name instead of
  # handing over `/dev`.
  TABLES = import ../../../common/radio-tables.nix;

  coordinatorRows = TABLES.radio "zigbee-coordinator";
  coordinator =
    if coordinatorRows == []
    then {
      device = "";
      mountAt = "";
      channel = null;
      name = "no zigbee-coordinator row";
    }
    else builtins.head coordinatorRows;
in {
  assertions = [
    {
      assertion = TABLES.problems == [];
      message = "common/radio-tables.nix found ${builtins.toString (builtins.length TABLES.problems)} problems with the radio table: ${builtins.concatStringsSep "; " TABLES.problems}";
    }
    {
      # The row has to name a dongle that exists: an empty device is what the
      # table says when a dongle is unplugged, and this service is the one the
      # space's whole Zigbee mesh depends on, so the case where silence is not
      # "nothing is wrong" is worth refusing rather than discovering at 03:00.
      assertion = coordinator.device != "" && builtins.match "/dev/serial/by-id/.*" coordinator.device != null;
      message = ''zigbee2mqtt's coordinator row (`${coordinator.name}`) names no device, or names one that is not a /dev/serial/by-id path: fill `common/radio-dongles.nix` with what `just radio-plug` prints. A bare /dev/ttyUSB<N> is handed out in plug order and re-handed on every reboot, and this unit is the live coordinator for the space's Zigbee mesh.'';
    }
  ];
  users.users.zigbee2mqtt = {
    uid = ZIGBEE2MQTT_UID;
    group = "users";
    extraGroups = ["dialout"];
    isNormalUser = true;
  };

  virtualisation.oci-containers.containers.zigbee2mqtt = {
    hostname = "zigbee2mqtt";
    image = "koenkk/zigbee2mqtt:latest";
    autoStart = true;
    ports = [
      "8282:8080"
    ];
    volumes = [
      "/srv/zigbee2mqtt:/app/data"
      "/run/udev:/run/udev:ro"
    ];
    environment = {
      TZ = "Europe/London";
    };
    extraOptions = [
      # Two halves of one fact: the by-id name the table says the space's mesh
      # runs on, mounted inside the container at the path zigbee2mqtt's own
      # runtime config already names (`serial: port: /dev/ttyUSB0`,
      # /srv/zigbee2mqtt/configuration.yaml — runtime state this flake does not
      # edit). What changes is that the flake now says which dongle answers that
      # name instead of trusting the kernel's ordering. Verify on the box after
      # the switch that takes this change: `ls -l /dev/ttyUSB*` inside
      # `podman-exec podman-zigbee2mqtt-service ls -l /dev/ttyUSB*`-shaped check
      # and the mesh's own next write — the devices in
      # /srv/zigbee2mqtt/configuration.yaml are the proof, not this file.
      "--device=${coordinator.device}:${coordinator.mountAt}"
      "--user=${toString ZIGBEE2MQTT_UID}:100"
      "--group-add=${toString config.users.groups.dialout.gid}"
    ];
  };

  services.nginx.virtualHosts = {
    "zigbee2mqtt.leighhack.org" = mkOriginVhost {
      proxyPass = "http://127.0.0.1:8282";
      websockets = true;
    };

    "zigbee2mqtt.int.leighhack.org" = mkOriginVhost {
      proxyPass = "http://127.0.0.1:8282";
      websockets = true;
    };
  };

  system.activationScripts.zigbee2mqtt = ''
    # Create config directory
    mkdir -p /srv/zigbee2mqtt

    # Ensure correct permissions
    chown -R ${toString ZIGBEE2MQTT_UID}:users /srv/zigbee2mqtt
    chmod -R g+rw /srv/zigbee2mqtt
  '';
}
