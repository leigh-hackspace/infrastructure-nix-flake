# MODULE — Home Assistant, moved off `apps1` (10.3.1.30) onto this box on 2026-11-16.
#
# The move itself, the client map it had to satisfy, and the legacy `10.3.1.30:8123`
# proxy that is watching for whoever still dials the old address:
# `docs/home-assistant-on-services1-2026-11-16.md`.  What is here is the four things
# the flake has to get right; the rest is runtime state under `/srv/homeassistant`,
# which the flake only mounts and never edits.
#
# WHY THE HOST NETWORK — and why this is the only container on this box that takes it
#
# The old unit on `apps1` was `docker run --network=host --privileged`, and host
# networking is not incidental there: HA's `matter`, `thread`, `apple_tv` and `zeroconf`
# half depend on sending and receiving **multicast on the LAN**, which a podman NAT
# network does not carry, and clients that dial HA by address rather than by name must
# reach it on `:8123` with their real source IP intact.  Publishing `8123:8123` on a
# bridge instead would have preserved the browser path and quietly broken discovery,
# and would have made every direct client arrive as the podman gateway (`10.88.0.1`),
# which is in nobody's `trusted_proxies`.
#
# The cost is stated plainly so it is not a surprise later: HA now answers `:8123` on
# **every address this box has**, unauthenticated as far as the network is concerned
# (HA's own auth is the boundary — there is no `sso = true` row for it in
# `common/front-door.nix`, and the authentik round-trip is an HA login flow, not a gate).
# That is exactly the surface `apps1` had, and `machines/services1/networking.nix` runs
# with `networking.firewall.enable = false`, so nothing here is newly exposed by the
# move; it is newly exposed *by this box*, which is a different audience of one.
#
# WHY NO `--privileged`, and why nothing is passed on `/dev` — this is the landmine
#
# `apps1` had no serial device at all, so the `zha` entry in HA's
# `.storage/core.config_entries` (a "Sonoff Zigbee 3.0 USB Dongle Plus" on
# `/dev/ttyUSB0`) has never once had a radio there; it is inert config.  This box's
# `/dev/ttyUSB0` **is** a radio — the `SMLIGHT SLZB-06M` that
# `machines/services1/services/zigbee2mqtt.nix` already owns (`port: /dev/ttyUSB0`,
# `adapter: ember`).  Reproducing `--privileged` here would hand that same dongle to the
# copied ZHA entry, and two Zigbee stacks on one coordinator is not a benign race: every
# Zigbee entity in HA arrives through zigbee2mqtt over MQTT, so winning the serial port
# is how HA "moves" and the whole mesh goes dark.
#
# So no device is passed, which is what reproduces the *measured* behaviour on the old
# box (a ZHA entry with no radio) rather than its declared behaviour.  If ZHA is ever
# genuinely wanted here, the decision is "which of the two stacks owns the dongle", not
# one flag; if some other integration turns out to need a device, name the device.
#
# WHY THE TAG IS A VERSION AND NOT `:stable`
#
# The old unit pulled `:stable` in `ExecStartPre` on every start, which is how HA core
# went `2026.10.0` → `2026.10.4` on `apps1` without anyone touching anything, and 2026.10.4
# turned the Frigate integration's deprecated `via_device` into a fatal error — twelve
# failed camera-entity registrations, cameras grey while their sensors kept counting
# (`docs/homeassistant-frigate-via-device-2026-11-02.md`).  It has drifted twice since:
# `.HA_VERSION` read `2026.10.4` on 2026-11-02 and `2026.11.0` on 2026-11-16.
#
# `2026.11.0` is therefore **the version the box was already running**, so the move
# carries no core change, and the pin closes the open item in `docs/follow-ups.md`
# ("Home Assistant on `apps1` runs whatever `:stable` meant today").  Existence of the
# tag was checked before this line was written, not assumed: the manifest for
# `ghcr.io/home-assistant/home-assistant:2026.11.0` answers 200.  A pin that cannot be
# pulled takes HA down at the next start, so bump it deliberately —
# `docker pull` it first, or read `just ha-status`.
#
# It also means HA stops updating by itself, which is the point.  The nightly
# `update-containers.timer` (`machines/services1/containers.nix`) walks *running*
# containers, re-pulls each image, and restarts the unit when the digest moved; against
# a pinned tag the digest never moves, so that timer leaves Home Assistant alone.
# Upgrading HA is now an edit to `HA_IMAGE` below plus a switch.
{
  config,
  lib,
  ...
}: let
  HA_IMAGE = "ghcr.io/home-assistant/home-assistant:2026.11.0";
  HA_STATE = "/srv/homeassistant";
in {
  virtualisation.oci-containers.containers.home-assistant = {
    image = HA_IMAGE;
    autoStart = true;
    volumes = ["${HA_STATE}:/config"];
    environment = {
      TZ = "Europe/London";
    };
    # `networkMode` does not exist in this pinned nixpkgs' oci-containers — measured:
    # `error: The option `virtualisation.oci-containers.containers.home-assistant.networkMode'
    # does not exist`, suggesting `networks`.  `networks = [ "host" ]` would put the
    # module's own network abstraction in the path instead of the flag, so the flag is
    # the honest form.  `hostname` is likewise dropped: with the host network it is
    # ignored, and config that cannot take effect is the kind this repo deletes.
    # Read the rendered unit back after any change here: podman must be offered
    # `--network=host` exactly once and no `-p` at all.
    extraOptions = [
      "--network=host"
      # The two capabilities two of HA's components ask for by name, and nothing wider.
      # Without them the container logs two ERRORs on every start — `habluetooth.manager:
      # Missing required permissions for Bluetooth management … Add NET_ADMIN and NET_RAW`
      # and `aiodhcpwatcher: Cannot watch for dhcp packets: [Errno 1] Operation not
      # permitted` — both of them consequences of dropping `--privileged`, both of them
      # something the next reader will file as an incident.  Neither is a working feature
      # on either box: `apps1` has **no Bluetooth adapter at all** (`/sys/class/bluetooth/`
      # empty, no Bluetooth device in `lsusb`), so its privileged habluetooth had nothing
      # to recover, and the DHCP watcher only *discovers new* ESPHome devices, all three
      # of which are configured by static host.  Granting the two caps anyway is the
      # deliberate choice: it buys back what the old box could do, and — unlike
      # `--privileged` — it does **not** open `/dev`, so the SLZB-06M above stays
      # zigbee2mqtt's.  Re-verify with:
      # `sudo podman logs home-assistant 2>&1 | grep -ciE "habluetooth|dhcp packets"` → 0.
      "--cap-add=NET_ADMIN"
      "--cap-add=NET_RAW"
    ];
  };

  # Host networking and `ports` are mutually exclusive in podman, and oci-containers
  # still emits `-p` for anything listed there; assert rather than discover it in a
  # container that fails to start at 3am.
  assertions = [
    {
      assertion = config.virtualisation.oci-containers.containers.home-assistant.ports == [];
      message = "home-assistant runs on the host network: `ports` would emit `-p` and podman would refuse.";
    }
  ];

  # The state directory must exist before the container starts, or podman creates it as
  # an empty root-owned dir and HA boots into an empty house with no way back that is not
  # a restore.  Deliberately *not* a recursive `chown`: the old box ran HA as root inside
  # the container against a root-owned tree, this one does the same, and the tree is
  # 556 MB — `docs/home-assistant-on-services1-2026-11-16.md` has the sizes and why the
  # copy skips the 278 MB of 2024 `*.corrupt.*` debris that came with it.
  system.activationScripts.home-assistant-dir = lib.stringAfter ["users"] ''
    mkdir -p ${HA_STATE}
  '';
}
