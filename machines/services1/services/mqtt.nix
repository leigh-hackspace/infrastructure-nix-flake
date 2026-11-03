{
  config,
  lib,
  pkgs,
  modulesPath,
  ...
}: let
  MQTT_UID = 1883;
in {
  # MODULE — the network's MQTT broker: an `eclipse-mosquitto` podman container on
  # services1, publishing **1883 only** to `0.0.0.0` — `podman ps` maps
  # `0.0.0.0:1883->1883/tcp` and `/srv/mosquitto/config/mosquitto.conf` holds one
  # `listener 1883` line, so there is no TLS listener on 8883 to add to `ports` or to
  # point a name at (checked on the box 2026-11-02 rather than read off this comment,
  # which used to claim 8883 as well). Conf under /srv/mosquitto
  # (`listener 1883`, `allow_anonymous true` — an anonymous LAN broker on purpose,
  # and the flake's `LOCAL_NETWORK` nft set is what keeps the LAN itself private).
  #
  # **Who dials it, and the trap that bit on 2026-10-24.** Nothing here names a
  # broker target: the *clients* are the ones that hardcode it, and all three dial
  # the name `mqtt.int.leighhack.org` in files this flake does not read —
  # Home Assistant's MQTT config entry (`apps1`'s runtime `.storage/core.config_entries`),
  # Frigate's publisher (`/srv/frigate/config/config.yaml`, bind-mounted only),
  # zigbee2mqtt's `server:` (`/srv/zigbee2mqtt/configuration.yaml`, bind-mounted only).
  # That name used to be a `legacy` row in `common/int-dns.nix`, i.e. "answer with the
  # front door, served by no vhost", and the 2026-10-24 deploy that moved the front door to
  # `gw` therefore re-pointed all three at 10.3.1.1, where nothing listens on 1883:
  # an MQTT client does not get a 404, it gets a hang, and a HA whose broker dial is
  # dead renders every Frigate **camera** entity Unavailable while its **sensors**
  # (plain HTTP) keep counting — the availability flip happens only when the broker
  # answers (`docs/mqtt-int-legacy-answer-2026-11-02.md`).
  #
  # Measured 2026-11-02, and the clients were retargeted to this box's address in those
  # three runtime files to stop the bleeding. The declarative answer landed the same day:
  # `mqtt` is now in `common/int-dns.nix`'s `answeredBy` group with `via = "services1"`, so
  # the name resolves here (one IPv4 line) rather than at the door, and
  # `machines/gw/lib/check-network-tables.nix` refuses a `via` that is not a machine, a
  # label in two groups, and any rendering but exactly that one line. Two things are still
  # outstanding and both are in `docs/follow-ups.md`: deploying `gw` (this is DNS — the
  # answer only moves when the router's dnsmasq restarts with the new hosts file), and then
  # putting the three dials back on the **name** rather than on `10.3.1.20`. Until the
  # second of those is done, this box's address is written into three files the flake
  # cannot see, which is the drift class `common/int-dns.nix` exists to prevent.
  #
  # Read before editing anything here or in `/srv/mosquitto`: `docs/router-and-dns.md`
  # (what the router answers for a `legacy` name, and the PARTIAL half-answer trap)
  # and `docs/mqtt-int-legacy-answer-2026-11-02.md` (the socket-port class).
  users.users.mqtt = {
    uid = MQTT_UID;
    group = "mqtt";
    isNormalUser = true;
  };

  users.groups.mqtt = {
    gid = MQTT_UID;
  };

  virtualisation.oci-containers.containers.mqtt = {
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
      "--user=${toString MQTT_UID}:${toString MQTT_UID}"
    ];
  };
}
