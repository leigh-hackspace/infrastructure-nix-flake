# MODULE — the Thread border router: OpenThread on this box, plus an OpenThread
# RCP radio in one of its USB ports. Disabled on 2026-11-26 (`infra.thread.enable`
# is false), and that is a decision made at a deploy, in front of a reader, not one
# made in code.
#
# READ docs/thread-border-router-2026-11-26.md BEFORE FLIPPING THAT FLAG. It owns
# the three things a reader cannot re-derive: why the spare dongle this network
# was offered (a Sonoff ZBDongle-P, Texas Instruments CC2652P, 352 KB flash / 80 KB
# RAM) is **not** a Thread radio and never will be at this network's size; which
# dongle can be (an EFR32MG24-class one, 1536 KB flash / 256 KB RAM); and the
# measured IPv6 state of this box, which is today the one thing that would let a
# Thread mesh join and then never be reachable.
#
# WHY THE BORDER ROUTER IS A UNIT IN THIS FLAKE AND NOT "AN APP"
#
# Every guide tells readers to install the **OpenThread Border Router app**, and
# every one of them means Home Assistant OS's Supervisor: apps do not exist in
# Home Assistant Container, which is what this box runs
# (`machines/services1/services/home-assistant.nix`, ghcr.io/home-assistant/
# home-assistant:2026.11.0 under podman). Home Assistant's Thread *integration* is
# core, so it is already here; the border router that feeds it is not, and cannot
# be installed from inside HA. So this unit owns the process the same way
# `zigbee2mqtt.nix` owns zigbee2mqtt and `mqtt.nix` owns the broker, and HA
# discovers the border router the way it is documented to: mDNS/DNS-SD on the LAN,
# which makes the Thread integration's config flow appear by itself.
#
# WHY THIS IMAGE, AND WHAT IS PINNED INSIDE IT
#
# `ghcr.io/smnws/ha-otbr-docker` is this repo's, not Home Assistant's. The reason
# is that it is built **from** Home Assistant's OpenThread Border Router app —
# measured in that repository's `Dockerfile` on 2026-11-26, where the build args
# pin the same upstream commits the app pins, `OTBR_VERSION=cb4bdae242b22287cded07ae3aa10347c2d16d9b`
# (stable) and `OTBR_BETA_VERSION=76fb21d666a41050b43f8eaabc30e71d9d75d9fd`
# (beta), on `ghcr.io/home-assistant/base-debian:trixie-2026.11.0@sha256:d996c8f8…`,
# and carries the `patches/openthread/0001-posix-hdlc-software-flow-control.patch`
# that the app does not need — and its whole purpose is to run that code
# **without Home Assistant OS's Supervisor**, which is exactly the situation
# `home-assistant.nix` describes. Home Assistant's own app image is pullable too
# (measured the same afternoon through the registry: it has `stable`, `3.4.0`,
# `dev`, seven commit tags and seven `sha256-…` build-image tags, and **no
# `latest`**) — so this is not a case of the upstream image being unreachable; it
# is a case of the upstream image being a *Supervisor* deliverable. If the app
# image is ever switched to, `:3.4.0` is its pin, not `:latest`.
#
# `:latest` is a moving tag and this repo pins versions where it can. It cannot
# here: this image's tag list, read through the registry API on 2026-11-26, is
# `latest`, `edge`, `edge-7f9c38a`, `sha-4639ace`, `sha-4fd3193`, `sha-9132920` —
# build identifiers, no version. So `just thread-pins` prints the tag list and the
# pinned digests it resolves, and `docs/follow-ups.md` keeps the open half
# (nothing in this flake can re-pin a container image mid-flight; the flake pins
# what it *asked for*, and the box answers what that meant today).
#
# WHY `--network=host`, AND WHY THAT IS NOT AN EXPOSURE PROBLEM HERE
#
# Thread is IPv6-only, and the border router has to *receive* ICMPv6 router
# advertisements and *send* IPv6 UDP to the mesh. On a podman NAT bridge the
# mesh's addresses become `10.88.0.x` and stop being reachable — the same reason
# `home-assistant.nix` takes the host network. What is pinned here is the
# *unauthenticated* REST API and web UI that OpenThread serves: they are bound to
# loopback by the image's own options, because HA runs on this box. Loopback is a
# boundary (AGENTS.md's service rules); a LAN address is not, and on this box's
# addresses an unauthenticated border-router console is a public endpoint.
#
# WHY THE DEVICE IS NAMED, NOT PASSED AS A NUMBER, AND WHY THE TABLE IS ASSERTED
#
# `common/radio-dongles.nix` decides which dongle this unit gets;
# `common/radio-tables.nix` holds the rules over that table and
# `common/check-radio-tables.nix` runs them for `just check-eval`. The assertions
# below run the same list against a deploy of this box, the same pair of entry
# points `machines/gw/lib/check-network-tables.nix` and
# `machines/gw/lib/assert-network-tables.nix` already use. That is not belt-and-
# braces: zigbee2mqtt already owns the one dongle plugged in here (an SMLIGHT
# SLZB-06M, the space's whole Zigbee mesh, `advanced: channel: 11`), and the
# kernel numbers `/dev/ttyUSB<N>` in plug order, so handing this unit a bare
# number is how a reboot makes the **Thread radio the Zigbee coordinator** —
# zigbee2mqtt starts, its coordinator answers, and every Zigbee entity in Home
# Assistant arrives over MQTT from a mesh that is now half-dead. That is the
# class of fault `home-assistant.nix`'s header calls "two Zigbee stacks on one
# coordinator"; this is the same fault with a second stack added to it.
#
# WHY THE CHANNEL IS NOT A FLAG IN THIS FILE
#
# The IEEE 802.15.4 channel is set through Home Assistant's Thread integration
# ("Change channel"), which schedules the switch about five minutes out so every
# mesh member learns it together, and it is stored in the border router's own
# state — `/srv/otbr`, runtime state this flake mounts and never edits, exactly
# as `/srv/homeassistant` is. The table's `channel = 26` is therefore a contract
# the guard enforces and `just thread-status` verifies against the running router,
# not a value rendered into a file. It is 26 and not 11 (zigbee2mqtt's) because
# Thread and Zigbee are both 2.4 GHz IEEE 802.15.4, channels 11-26 overlap with
# the Wi-Fi this network is full of, and two radios on one channel drown each
# other — the "they must match" advice that floats around is about **MultiPAN**,
# one radio chip serving both, which Home Assistant marks deprecated and this
# network does not use.
{
  config,
  lib,
  ...
}: let
  TABLES = import ../../../common/radio-tables.nix;

  # The row this unit is built around. A row that does not exist is not a bug —
  # the row is what says "this network has a Thread radio planned" — so the module
  # reads an empty node rather than throwing, and the assertion below is what
  # refuses a deploy.
  threadRow =
    if TABLES.thread == null
    then {
      device = "";
      mountAt = "";
      channel = null;
      name = "no thread-rcp row";
    }
    else TABLES.thread;

  # Measured 2026-11-26 against the registry's tag list for this image: `latest`,
  # `edge`, `edge-<sha>`, three `sha-<hex>` build tags. No version tag exists, so
  # there is nothing more specific to pin, and `just thread-pins` prints the list
  # so the drift is a number someone can read.
  OTBR_IMAGE = "ghcr.io/smnws/ha-otbr-docker:latest";
  OTBR_STATE = "/srv/otbr";

  cfg = config.infra.thread;
  # The containers this module may start, whichever way `enable` decided. Reading
  # `containers.<name>` directly would throw when the module is disabled — the
  # attribute simply is not there — and an assertion that throws is a deploy that
  # cannot be made, which is the wrong shape for a kill switch.
  ours = builtins.attrValues (lib.filterAttrs
    (n: _: builtins.elem n ["otbr"])
    config.virtualisation.oci-containers.containers);
in {
  options.infra.thread.enable = lib.mkOption {
    type = lib.types.bool;
    default = false;
    description = ''
      Start the OpenThread border router (the Thread radio plus the OpenThread
      stack on this box) that Home Assistant's Thread integration and Matter-
      over-Thread devices need.

      False by default, and flipping it is a deploy-time decision: the radio
      `common/radio-dongles.nix` gives this unit is unplugged as measured on
      2026-11-26, the assertion below refuses a border router aimed at an
      unnamed node, and docs/thread-border-router-2026-11-26.md carries the
      measured reason a Sonoff ZBDongle-P (TI CC2652P) is the wrong dongle for
      this job and the measured IPv6 state of this box that has to change
      before a Thread mesh is reachable at all.
    '';
  };

  config.assertions = [
    {
      assertion =
        if !cfg.enable
        then true
        else threadRow.device != "";
      message = ''
        infra.thread.enable is on while `common/radio-dongles.nix`'s thread-rcp row has no device: fill it with the /dev/serial/by-id name `just radio-plug` prints once the dongle is in a USB port. A border router on a node that does not exist is a Thread network nothing can commission into, and it starts happily — this is the one configuration in which silence means nothing is wrong.
      '';
    }
    {
      # The table's rules, asserted where the table is read, so a deploy of
      # the box that runs the radios cannot pass them even with `just check`
      # skipped. One implementation (`common/radio-tables.nix`) means this
      # list and the one `just check-eval` runs cannot disagree.
      assertion = TABLES.problems == [];
      message = "common/radio-tables.nix found ${builtins.toString (builtins.length TABLES.problems)} problems with the radio table: ${builtins.concatStringsSep "; " TABLES.problems}";
    }
    {
      # `--network=host` and `ports` cannot both be asked of podman: oci-
      # containers emits `-p` for anything listed in `ports` and podman
      # refuses the pair. Same assertion, and same reason, as
      # home-assistant.nix's — a `-p` here is a container that fails to start
      # at 03:00, not a typo.
      assertion = builtins.all (c: c.ports == []) ours;
      message = "an otbr container with `ports` would emit `-p` and podman would refuse the pair with `--network=host`: `ports` would emit `-p` and podman would refuse.";
    }
  ];

  config.virtualisation.oci-containers.containers.otbr = lib.mkIf cfg.enable {
    image = OTBR_IMAGE;
    autoStart = true;
    # One persistent volume: this directory holds the Thread network's own
    # credentials — its PAN ID and channel — so it is state, like HA's
    # `/srv/homeassistant`, and the image refuses `/var/lib/thread` alongside
    # `/data/thread` (its `configure` script says "Thread credentials are
    # persisted" about `/data/thread`), so the mount has to be right before the
    # first start or the network's identity is written somewhere the next
    # start will not read.
    volumes = ["${OTBR_STATE}:/data/thread"];
    extraOptions = [
      "--network=host"
      # Two halves of one fact: the by-id name the table says, mounted at the
      # node the container is told about, so no other file in this flake has to
      # know which number the kernel handed out today.
      "--device=${threadRow.device}:${threadRow.mountAt}"
      # The adapter device the image's own example asks for: `otbr` sets up its
      # ingress filtering with iptables/ipset, which needs these two caps, and
      # the example says `privileged: true` is the fallback for a host that
      # blocks TUN/netfilter under `cap_add`. Not taken here, deliberately:
      # this box's /dev holds the Zigbee coordinator (`home-assistant.nix`'s
      # landmine — `/dev/ttyUSB0` on the host *is* the live coordinator), and
      # a `--privileged` container is one `ls /dev` away from owning it. If
      # netfilter is refused on the first deploy, that is a measured thing to
      # say in docs/thread-border-router-2026-11-26.md, not a reflex.
      "--cap-add=NET_ADMIN"
      "--cap-add=NET_RAW"
      "--device=/dev/net/tun"
    ];
    environment = {
      # The node *inside* the container — the second half of the `--device`
      # mapping above, and what `otbr-agent`'s `spinel+hdlc+uart://…` URL names.
      DEVICE = threadRow.mountAt;
      # 460800 and no flow control are the contract the firmware artifact's own
      # name states (`..._no_flow_460800.gbl` / `..._no_flow_460800.hex`,
      # `radios/firmware-pins.json`): the RCP builds for this silicon do not
      # implement software flow control, and a router that asks for it is the
      # `RadioSpinelNoResponse` crash class upstream sees on CC2652P dongles —
      # not a bug, and not only a URL typo.
      BAUDRATE = "460800";
      FLOW_CONTROL = "0";
      # The interface this box's LAN IPv6 lives on, measured on the box 2026-11-26
      # (/proc/net/if_inet6 answers `vlan225` with the network's own
      # fd99:dead:beef:: and the AAISP tunnel's 2001:08b0:… on it). A podman
      # `veth*` here would name a private subnet the LAN cannot see.
      BACKBONE_IF = "vlan225";
      # Matter does not need NAT64; MQTT-over-Thread does. Nothing on this
      # network has yet asked a Thread device to reach the space's MQTT broker
      # (`mqtt.nix`), so this stays a decision point rather than a guess — the
      # doc owns the question, and it is open in docs/follow-ups.md.
      NAT64 = "0";
      # One border router per network, which is true here (there is no second
      # Thread radio in `common/radio-dongles.nix`), so the ingress filtering
      # is the thing the upstream guide recommends rather than the thing it
      # warns about for networks that have more than one.
      FIREWALL = "1";
      # The radio's TX power class: the image's default is 6 dBm and its own
      # comment says EFR32MG24-class sticks may need 7 to report 6 dBm. Left at
      # the default until a real MG24 dongle is on this box, and `just
      # thread-status` is where that would be noticed.
      TX_POWER = "6";
      # The stable build, not the beta. BETA=1 would select the other pinned
      # upstream commit (`OTBR_BETA_VERSION` above); the docs' own warning about
      # beta builds is that they move, and this repo pins versions where it can.
      BETA = "0";
      OTBR_LOG_LEVEL = "notice";
      # Loopback, and a port that is this network's to allocate — the guide's
      # 8081 is **taken on this box**: the Space API container maps 127.0.0.1:8081
      # to its internal :80 (`machines/services1/services/hackspace-api.nix`,
      # `SPACE_API_PORT = 8081`, `ports = ["8081:80"]`), and `hackspace-mcp` dials
      # `--space-url http://127.0.0.1:8081`. Measured on services1 2026-11-26 with
      # `curl -i http://127.0.0.1:8081/`, which answers the Space API's uvicorn
      # (307 to /docs) with nothing listening per `ss -ltnp` from the host's view
      # — a podman-published socket, which is why it is invisible there and real
      # here. 8196 and 8098 are next in the band this network already uses
      # (8088 status-dashboard, 8095-8097 the apps, 8191-8195 the bot and the
      # voice tier) and both answered connection-refused the same afternoon.
      REST_LISTEN_ADDRESS = "127.0.0.1";
      REST_LISTEN_PORT = "8196";
      WEB_LISTEN_ADDRESS = "127.0.0.1";
      WEB_PORT = "8098";
    };
  };

  # The state directory has to exist before the container starts, or podman
  # creates it as an empty root-owned directory and the Thread network's
  # credentials are written into a tree the next start may not read. Same
  # shape, and same reasoning, as home-assistant.nix's: not recursive (the tree
  # is created here), and no `+` (a `+` would clear a directory that already
  # holds a commissioned fabric).
  config.system.activationScripts.otbr-dir = lib.mkIf cfg.enable (lib.stringAfter ["users"] ''
    mkdir -p ${OTBR_STATE}
  '');
}
