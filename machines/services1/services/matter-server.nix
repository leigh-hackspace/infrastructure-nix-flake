# MODULE — the Matter controller (the WebSocket server behind Home Assistant's
# Matter integration). Disabled on 2026-11-26: a Matter fabric with no Thread
# radio under it is a fabric that can be started and cannot be commissioned
# into, and the radio `thread-router.nix` mounts is not on this network yet.
#
# Same kill switch as the Thread border router next to it, and the same shape as
# `authentik.nix`'s `staged`: the decision is made at a deploy, in front of a
# reader who has read docs/thread-border-router-2026-11-26.md, not in code.
#
# THE TWO HALVES OF THE PLAN, AND WHY THIS IS NOT "AN ADD-ON"
#
# Home Assistant's Matter support is two pieces: an integration in core (present
# in this box's pinned core 2026.11.0) and a **controller process** that owns the
# Matter fabric — the app/add-on every guide installs. Apps are a Supervisor
# feature and this box runs Home Assistant **Container** under podman, so the app
# does not exist here and the flake owns the process instead, exactly as
# `zigbee2mqtt.nix` owns zigbee2mqtt and `mqtt.nix` owns the broker. HA then
# connects to it over a WebSocket at a URL, which the `matter` integration's
# config flow asks for ("already running the Matter server in a custom container:
# deselect the checkbox, provide the URL"). That URL is loopback state in HA's
# browser, not something this file can write.
#
# The other half is the **radio**: Matter devices that speak Thread do so through
# a Thread border router, and the router — not this server — touches the mesh.
# The two never talk to each other: the Matter server sends IPv6 UDP to the
# border router's address, which forwards it to the mesh. So enabling this without
# `infra.thread.enable` is legal only for Matter-over-Wi-Fi devices, and that is a
# decision to write down in the doc rather than an assertion to invert later.
#
# WHY `matter-js/matterjs-server` AND NOT THE PYTHON MATTER SERVER
#
# The Python Matter Server is what every older guide and every older add-on is
# built from; the JS server in `github.com/matter-js/matterjs-server` states that
# it is a **drop-in replacement** for it — same WebSocket API, reads the Python
# server's data files and migrates them on first start — and it is what Home
# Assistant's own 2026.x documentation points container users at
# ("If you run Home Assistant in a container, you can run a Docker image of the
# Matter server"). Pinning the newer one is therefore not a rewrite of HA's
# Matter support: it is what HA's current docs name. Its 1.5-or-newer camera
# support and 1.4-or-newer network-map support are stated in those docs against
# container versions; the `0.7.x` this file pins is far past either, so a Matter
# camera and the Matter network map are both inside what the pin can do.
#
# WHY THE UID IS DECLARED RATHER THAN LEFT AT THE IMAGE'S
#
# The image runs `USER 1000:1000` and chowns `/data` to 1000:1000 — but `/data` is
# a bind mount of `${MATTER_STATE}`, so the image's chown is overwritten by the
# host directory's ownership before the process starts, and uid 1000 on *this* box
# is `leigh-admin` (uid 1000 in `common/users.nix`): the Matter fabric — every
# commissioned device, its credentials, and the DCL seed data — would be a tree
# the operator owns and can rewrite. The container is therefore handed an account
# of its own, 8125, next to zigbee2mqtt's 8124 and the other pinned service uids,
# and the activation script below is what makes `${MATTER_STATE}` belong to it.
# Same rule `hackspace-api.nix`'s header states about a credential: runtime state
# on disk, not a store path, and owned by the thing that reads it.
#
# WHY COMMISSIONING IS AN OPEN ITEM AND NOT A FLAG
#
# Commissioning a Matter device needs Bluetooth from somewhere: the phone's BLE
# stack for a normal device, or a host BLE adapter. This box has **no Bluetooth at
# all** — measured in `machines/services1/services/home-assistant.nix`'s header:
# `/sys/class/bluetooth/` is empty and `lsusb` has no Bluetooth device — so
# `BLUETOOTH_ADAPTER` cannot be set. The JS server's alternative is `BLE_PROXY=true`
# (a `/ble` WebSocket endpoint that HA's Matter integration drives from wherever
# the adapter actually lives), and HA's docs say that needs HA 2026.6+ (this box
# is 2026.11.0) and an ESPHome 2026.5 / esp-idf 6.0.1 device to host it. This
# network runs ESPHome devices already, so the bridge is a device and a
# configuration, not a new service — which device hosts it is open in
# `docs/follow-ups.md`, and until it exists the flag below is `true` by decision
# and BLE commissioning is dark.
{
  config,
  lib,
  ...
}: let
  cfg = config.infra.matter;
  # The containers this module may start, whichever way `enable` decided. Reading
  # `containers.<name>` directly would throw when the module is disabled — the
  # attribute simply is not there — and an assertion that throws is a deploy that
  # cannot be made, which is the wrong shape for a kill switch.
  ours = builtins.attrValues (lib.filterAttrs
    (n: _: builtins.elem n ["matter-server"])
    config.virtualisation.oci-containers.containers);

  # Measured 2026-11-26 against the registry's tag list: the newest numeric tag
  # is `0.7.1` and `stable`/`latest`/`dev` move with it. A numeric pin is this
  # repo's habit for exactly the reason `home-assistant.nix` pins `2026.11.0`:
  # `:stable` is how HA went from 2026.10.4's fatal `via_device` to whatever the
  # next release decides.
  MATTER_IMAGE = "ghcr.io/matter-js/matterjs-server:0.7.1";
  MATTER_STATE = "/srv/matter-server";
  MATTER_UID = 8125;
in {
  options.infra.matter.enable = lib.mkOption {
    type = lib.types.bool;
    default = false;
    description = ''
      Run the Matter controller (the WebSocket server Home Assistant's Matter
      integration connects to) on this box.

      False by default. Nothing can be commissioned until
      `infra.thread.enable` runs a Thread radio on the backbone this server
      shares, and the radio the network was offered — a Sonoff ZBDongle-P,
      TI CC2652P — is a measured dead end for that. Read
      docs/thread-border-router-2026-11-26.md before turning this on.
    '';
  };

  config.assertions = [
    {
      # `--network=host` and `ports` cannot both be asked of podman:
      # oci-containers emits `-p` for anything in `ports`, and podman refuses
      # the pair. Same assertion as home-assistant.nix and thread-router.nix.
      assertion = builtins.all (c: c.ports == []) ours;
      message = "a matter-server container with `ports` would emit `-p` and podman would refuse the pair with `--network=host`: `ports` would emit `-p` and podman would refuse.";
    }
    {
      # Matter-over-Thread needs a Thread border router on the same physical
      # network as this server. Enabling Matter without one is a decision
      # about Wi-Fi-only Matter devices, and this network has no such device
      # — so a fabric with no radio is a unit that starts, works, and
      # commissions nothing, forever. Stated as an assertion because that is
      # the failure a reader would otherwise spend a week on.
      assertion =
        if !cfg.enable
        then true
        else config.infra.thread.enable;
      message = "infra.matter.enable is on with infra.thread.enable off: a Matter fabric with no Thread border router on this box can commission nothing.";
    }
  ];

  config.virtualisation.oci-containers.containers.matter-server = lib.mkIf cfg.enable {
    image = MATTER_IMAGE;
    autoStart = true;
    volumes = ["${MATTER_STATE}:/data"];
    extraOptions = [
      # IPv6 UDP to and from the Thread border router, with this box's real
      # addresses: the Matter server has to sit on the LAN's network, and a
      # podman bridge would make every peer a 10.88.0.x the mesh cannot reach.
      # The Matter server's own docs name the same requirement for container
      # installs ("this container installation uses host networking, and relies
      # on the networking managed by your operating system").
      "--network=host"
      # Not the image's 1000:1000 — that is `leigh-admin` here (see the header).
      # 8125:8125 and the chown below are one decision, not two settings.
      "--user=${toString MATTER_UID}:${toString MATTER_UID}"
    ];
    environment = {
      STORAGE_PATH = "/data";
      # Loopback: the dashboard and the WebSocket would otherwise answer
      # unauthenticated on every address this box has, which is what
      # home-assistant.nix says its own unit does and calls out as a different
      # audience. HA runs on this box, so loopback is the whole path.
      # Loopback, and the server's own default port: 5580 answered connection-
      # refused on this box on 2026-11-26, so it is this network's to take. HA
      # dials it as a WebSocket URL in the integration's config flow, which is
      # runtime state in a browser, not something this file can write.
      LISTEN_ADDRESS = "127.0.0.1";
      PORT = "5580";
      # The interface the LAN's IPv6 lives on, read off this box
      # (/proc/net/if_inet6). Matter is link-local multicast: pinning the
      # interface is what stops it picking a podman `veth*` it cannot reach.
      PRIMARY_INTERFACE = "vlan225";
      LOG_LEVEL = "info";
      # No local BLE adapter exists on this box, so the endpoint is the shape
      # and the adapter is elsewhere — see the header's open item.
      BLE_PROXY = "true";
    };
  };

  # The state directory, and its owner: the image chowns `/data` to 1000:1000
  # inside itself, and a bind mount replaces that, so the host directory's
  # ownership is the one that has to be right before the first start. Non-
  # recursive on a fresh mkdir for the same reason home-assistant.nix's is: the
  # tree is created here, and a recursive chown of a directory that already
  # holds a commissioned fabric is a command that can wipe nothing but can
  # certainly be typed wrong.
  config.system.activationScripts.matter-server-dir = lib.mkIf cfg.enable (lib.stringAfter ["users"] ''
    mkdir -p ${MATTER_STATE}
    chown ${toString MATTER_UID}:${toString MATTER_UID} ${MATTER_STATE}
    chmod 0750 ${MATTER_STATE}
  '');
}
