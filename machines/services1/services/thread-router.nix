# MODULE — the Thread border router: OpenThread on this box, plus an OpenThread
# RCP radio in one of its USB ports. Disabled (`infra.thread.enable` is false),
# and that is a decision made at a deploy, in front of a reader, not in code.
#
# READ `docs/thread-border-router-2026-11-26.md` §13 BEFORE FLIPPING THAT FLAG.
# It carries the correction that changed this module's shape: a Sonoff ZBDongle-P
# (TI **CC2652P10**) flashed to the launchpad `ot_rcp` image **was measured
# working** on the sibling fleet on 2026-11-26 — `otbr-agent --radio-version`
# answered `OPENTHREAD/1.4.0-Koenkk-2025.3.1; CC13XX_CC26XX` — so this network's
# own spare dongle is the Thread radio, and what is open is whether *mesh joins*
# hold on it (upstream's tracker reports a CC2652P-class RCP losing the radio
# when a Thread device joins, `RadioSpinelNoResponse`, closed without a root
# cause), not whether the dongle can be a radio at all.
#
# The four things in that file that this module is shaped by, all of them
# measured on a box rather than read off a guide, are:
#
# 1. **the environment names are upstream's, not a fork's.** The authoritative
#    list is `etc/docker/border-router/` in the ot-br-posix repository (the s6
#    `otbr-agent` run script reads `OT_RCP_DEVICE`, `OT_INFRA_IF`, `OT_THREAD_IF`,
#    `OT_REST_LISTEN_ADDR`, `OT_REST_LISTEN_PORT`, `OT_LOG_LEVEL`). An
#    `OTBR_*`/`BACKBONE_IF`-shaped name in front of this image is *silently
#    ignored* — a container that starts, configs nothing, and sits at state
#    `disabled` is the symptom, and it is the same "a channel that dies while
#    still answering" shape as AGENTS.md rule 29.
# 2. **the upstream image does not form a Thread network on an empty dataset**
#    (unlike Home Assistant's add-on, which does): a fresh stick sits at
#    `disabled` forever and nothing can be commissioned into it. `otbr-form`
#    below is the one-time formation, idempotent, and it always exits 0.
# 3. **`--device` with a symlink source is resolved by podman** (measured
#    2026-11-27): the explicit `source:target` form is what puts the node in the
#    container at the name the radio URL uses; a bare symlink form lands it at
#    `/dev/ttyUSBn` and `otbr-agent` ENOENTs (`hdlc_interface.cpp:157`).
# 4. **a container that cannot see its device fails the activation**, and on an
#    `autoRollback` host that is a reboot five minutes later (measured on the
#    sibling router: `stat /dev/otbr-rcp: no such file`, a failed switch, then
#    an auto-rollback of a half-switched box). Hence the assertion below: the
#    table's `device` has to name the dongle before this module can be enabled
#    at all, so activation never becomes a question of "is the stick plugged in
#    today" — that question belongs at runtime, where `Restart=always`
#    (`common/containers.nix`'s never-give-up policy) answers it.
#
# WHY THE BORDER ROUTER IS A UNIT IN THIS FLAKE, NOT "AN APP"
#
# Every guide says "install the OpenThread Border Router app", and every one of
# them means Home Assistant OS's Supervisor. Apps do not exist in Home
# Assistant Container, which is what this box runs
# (`machines/services1/services/home-assistant.nix`: `ghcr.io/home-assistant/
# home-assistant:2026.11.0` under podman). Home Assistant's *Thread* integration
# is core, so it is already on this box; the border router that feeds it is not,
# and cannot be installed from inside HA. So this unit owns the process the way
# `zigbee2mqtt.nix` owns zigbee2mqtt and `mqtt.nix` owns the broker, and HA
# attaches the way it documents: the `otbr` integration at the REST address, then
# the `thread` integration, then `set as preferred network`, then Matter.
#
# WHY THIS IMAGE, AND WHAT IS PINNED INSIDE IT
#
# Upstream's own: `docker.io/openthread/border-router`, pinned **by digest** —
# the digest below is the one the sibling fleet ran on 2026-11-24 and got a
# working Thread border router out of, which makes it the only pinned image
# here whose behaviour is someone's receipt rather than a guess. That digest
# **still resolves**: `nix shell nixpkgs#skopeo --command skopeo inspect
# "docker://docker.io/openthread/border-router@sha256:ccab33a1…"` answers on this
# box on 2026-11-27, so the receipt is a live image and not a build Docker has
# since garbage-collected. Upstream's tag list **does** carry versions, unlike
# the fork's — read the same afternoon with `skopeo inspect` and `just thread-pins`:
# `stable` and `v2026.11.0` resolve to `sha256:5ae17855…`, `v2026.10.0` to
# `sha256:70d2260e…`, and the moving `latest`/`main` to `sha256:058bafa2…`. The
# pin here is deliberately the *older* digest someone's Thread radio ran on;
# the current `stable` is one `skopeo inspect` away if this box needs the newer
# build, and a re-pin changes the digest and never reaches for `:latest`, which
# is the moving one.
#
# The fork `ghcr.io/smnws/ha-otbr-docker` and Home Assistant's own app image are
# still listed in `radios/firmware-pins.json`'s sibling note and in
# `just thread-pins` (both tag lists were read off the registry API on
# 2026-11-26, and Home Assistant's app image has **no `:latest`** — its pin would
# be `:3.4.0`), and they are kept visible because they are what a reader will
# find when they search. They are not what this module runs: the fork renames the
# environment (a second vocabulary behind the same agent, which is a drift
# surface), and the app image is a Supervisor deliverable.
#
# WHY `--network=host`, AND WHY `--privileged` IS NOT COPIED
#
# Thread is IPv6-only: the border router has to *receive* ICMPv6 router
# advertisements and *send* IPv6 UDP to the mesh, and on a podman NAT bridge the
# mesh's addresses become `10.88.0.x` and stop being reachable — the same reason
# `home-assistant.nix` takes the host network. What is pinned here is that the
# REST API and web UI are bound to loopback by the environment, because HA runs
# on this box (loopback is a boundary; a LAN address is not).
#
# The sibling router runs this container `--privileged`, and says why: the image's
# agent inserts its **own** ingress filter (`OT_FORWARD_INGRESS`) and the `FORWARD`
# accepts for egress with ip6tables, and those edits have to land in the host
# namespace. This module does not copy that, deliberately: this box's `/dev` holds
# the live Zigbee coordinator (`/dev/ttyUSB0` on the host *is* the space's whole
# Zigbee mesh — the landmine `home-assistant.nix` names), and a `--privileged`
# container is one `ls /dev` away from owning it, which is how a Thread deploy
# would put a second stack on the Zigbee coordinator. What is asked for instead is
# `/dev/net/tun` plus `NET_ADMIN`/`NET_RAW`. **If** netfilter is refused on the
# first deploy, that is a measured thing to write in the doc, and the fallback is
# `--privileged` with the device list explicit — not a reflex applied here.
{
  config,
  lib,
  pkgs,
  ...
}: let
  TABLES = import ../../../common/radio-tables.nix;

  # The row this unit is built around. An empty `device` is a value, not a bug:
  # the row is what says "this network has a Thread radio planned", and the
  # assertion below is what refuses a deploy while it is unplugged.
  threadRow =
    if TABLES.thread == null
    then {
      device = "";
      mountAt = "";
      channel = null;
      name = "no thread-rcp row";
    }
    else TABLES.thread;

  # docker.io/openthread/border-router:latest as of 2026-11-24, by digest — the
  # build the sibling fleet ran (receipt: `cjdell/nixos-config`,
  # `hosts/grafton-router/services/otbr.nix` and its `docs/thread-border-router.md`).
  OTBR_IMAGE = "docker.io/openthread/border-router@sha256:ccab33a1aa3965cd7aec18bbf5869de1e88b43946e33256899df4f6e62180cba";
  OTBR_STATE = "/srv/otbr";

  cfg = config.infra.thread;
  ours = builtins.attrValues (lib.filterAttrs
    (n: _: builtins.elem n ["otbr"])
    config.virtualisation.oci-containers.containers);
in {
  options.infra.thread.enable = lib.mkOption {
    type = lib.types.bool;
    default = false;
    description = ''
      Start the OpenThread border router — the Thread radio in this box's USB
      port plus the OpenThread stack that runs beside it — for Home Assistant's
      Thread integration and for Matter-over-Thread devices.

      False by default. Flipping it is a deploy-time decision, and the three
      measured things that go into it are in docs/thread-border-router-2026-11-26.md:
      `common/radio-dongles.nix`'s thread-rcp row is unplugged on this box today
      (the assertion below refuses the deploy, so silence is not a pass), the
      CC2652P-class radio this network owns is **working with an unresolved
      upstream caveat about mesh joins**, and this box's IPv6 measures the wrong
      way for a Thread mesh to be reachable (forwarding, accept_ra and
      accept_ra_rt_info_max_plen on `vlan225`, read 2026-11-26).
    '';
  };

  config.assertions = [
    {
      assertion =
        if !cfg.enable
        then true
        else threadRow.device != "";
      message = ''
        infra.thread.enable is on while `common/radio-dongles.nix`'s thread-rcp row has no device: fill it with the /dev/serial/by-id name `just radio-plug` prints once the dongle is in a USB port. A border router aimed at an unnamed node is a Thread network nothing can commission into, and it starts happily — this is the one configuration in which silence means nothing is wrong.
      '';
    }
    {
      # The table's rules, asserted where the table is read, so a deploy of
      # this box cannot pass them even with `just check` skipped. One
      # implementation (`common/radio-tables.nix`) means this list and the one
      # `just check-eval` runs cannot disagree — AGENTS.md rule 24's shape.
      assertion = TABLES.problems == [];
      message = "common/radio-tables.nix found ${builtins.toString (builtins.length TABLES.problems)} problems with the radio table: ${builtins.concatStringsSep "; " TABLES.problems}";
    }
    {
      # `--network=host` and `ports` cannot both be asked of podman: oci-
      # containers emits `-p` for anything listed in `ports` and podman
      # refuses the pair. Same assertion, and same reason, as
      # home-assistant.nix's — a `-p` here is a container that fails to start,
      # not a typo.
      assertion = builtins.all (c: c.ports == []) ours;
      message = "an otbr container with `ports` would emit `-p` and podman would refuse the pair with `--network=host`; keep the REST API on loopback via the image's own address options.";
    }
    {
      # The radio URL and the bind mount have to name the same path, and the
      # target half of `--device` is what defines it. A `mountAt` that is a bare
      # `/dev/ttyUSB<N>` would put a plug-order number in front of a container:
      # the kernel hands those out in plug order and re-hands them on every
      # reboot, which is how a Thread radio becomes the Zigbee coordinator
      # overnight (`home-assistant.nix`'s landmine, AGENTS.md's dongle rule).
      assertion =
        if !cfg.enable
        then true
        else lib.hasPrefix "/dev/" threadRow.mountAt && (lib.match "/dev/ttyUSB[0-9]+" threadRow.mountAt == null);
      message = ''
        `common/radio-dongles.nix`'s thread-rcp row mounts at something that looks like a kernel node: the container must be handed a stable name (`/dev/otbr-rcp` today) bound with `--device=<by-id>:/dev/otbr-rcp`, because podman resolves a symlink `--device` source (measured 2026-11-27: a bare symlink form lands the node at `/dev/ttyUSBn` inside the container and `otbr-agent` ENOENTs on its radio URL) and because a bare `ttyUSB<N>` is plug order, not identity.
      '';
    }
  ];

  config.virtualisation.oci-containers.containers.otbr = lib.mkIf cfg.enable {
    image = OTBR_IMAGE;
    autoStart = true;
    hostname = "otbr";
    volumes = [
      # The image's own run script does `ln -sft /var/lib /data/thread`, so the
      # Thread network's dataset — its network key, its border-router id, its
      # on-mesh prefix — persists under ${OTBR_STATE}/thread. This is state, not
      # config, for the same reason HA's `/srv/homeassistant` is: losing it means
      # recreating the Thread network and re-commissioning every device in it.
      # The sibling fleet also has this path inside their borg `backup-srv` job;
      # here `/srv/otbr` is on the NAS-backed tree like HA's — see
      # `machines/services1/services/home-assistant.nix` and `docs/nas-and-borg.md`.
      "${OTBR_STATE}:/data"
      "/etc/localtime:/etc/localtime:ro"
    ];
    environment = {
      TZ = "Europe/London";
      # 6 = Info. 7 (Debug) is the image's default and floods the journal.
      OT_LOG_LEVEL = "6";
      # 460800 8N1, no hardware flow control: that is the contract the pinned
      # RCP artifact's own name states (`..._no_flow_460800`), and asking for
      # flow control is the `RadioSpinelNoResponse` class rather than a typo.
      OT_RCP_DEVICE = "spinel+hdlc+uart://${threadRow.mountAt}?uart-baudrate=460800";
      # This box's LAN interface, measured on the box (/proc/net/if_inet6 answers
      # `vlan225`, which carries the network's own fd99:dead:beef:: and the AAISP
      # tunnel's 2001:08b0:…); a podman `veth*` here would name a private subnet
      # the LAN cannot see.
      OT_INFRA_IF = "vlan225";
      # The interface the mesh is configured on, upstream's own name; otbr-agent
      # brings it up over `/dev/net/tun`, which is the device asked for below.
      OT_THREAD_IF = "wpan0";
      # Loopback, and a port that is this network's to allocate — 8081 is the
      # Space API's on this box (`machines/services1/services/hackspace-api.nix`:
      # `SPACE_API_PORT = 8081`, `ports = ["8081:80"]`, dialled by `hackspace-mcp`),
      # measured 2026-11-26 with `curl -i` while `ss -ltnp` showed nothing (a
      # podman-published socket is invisible to the host's view, which is the
      # reason one tool sees it and the other does not). 8196 answered
      # connection-refused the same afternoon.
      OT_REST_LISTEN_ADDR = "127.0.0.1";
      OT_REST_LISTEN_PORT = "8196";
    };
    devices = [
      # The TUN device otbr-agent uses to bring `OT_THREAD_IF` up. Not
      # `--privileged`: see that module header's paragraph on why the sibling
      # fleet needed privileged (its ip6tables edits have to land in the host
      # namespace) and why this one does not copy it (this box's `/dev` holds
      # the live Zigbee coordinator).
      "/dev/net/tun"
    ];
    extraOptions = [
      "--network=host"
      # The explicit `source:target` form, required: podman resolves a symlink
      # source, so naming only the by-id path would land the node inside the
      # container at `/dev/ttyUSBn` and not at the name
      # `OT_RCP_DEVICE` uses. (Measured 2026-11-27 on the sibling fleet;
      # `otbr-agent` died at `hdlc_interface.cpp:157 ... No such file or
      # directory`, and moving the watch to the by-id symlink changed nothing.)
      "--device=${threadRow.device}:${threadRow.mountAt}"
      "--cap-add=NET_ADMIN"
      "--cap-add=NET_RAW"
    ];
  };

  config.systemd.services = lib.optionalAttrs cfg.enable {
    # Upstream OTBR — unlike Home Assistant's add-on — does not form a Thread
    # network when the persisted dataset is empty, so a fresh stick sits at
    # state `disabled` forever and commissioning has nothing to join. Measured
    # on the sibling fleet 2026-11-27. This oneshot is idempotent (it greps a
    # `networkkey` out of the live dataset) and always exits 0: formation
    # trouble must never take the radio unit down with it, and a non-zero
    # oneshot on an autoRollback host is a reboot.
    otbr-form = {
      unitConfig.After = ["podman-otbr.service"];
      serviceConfig = {
        Type = "oneshot";
        # Explicit, and under systemd's default start timeout: the wait loop
        # below has ~60 s of headroom for the container to come up, and a unit
        # killed mid-form reads as a formation failure rather than a timeout.
        TimeoutStartSec = 150;
        ExecStart = pkgs.writeShellScript "otbr-form" ''
          set -u
          podman="${pkgs.podman}/bin/podman"
          for _ in $(seq 30); do
            sleep 2
            "$podman" exec otbr ot-ctl state >/dev/null 2>&1 && break
          done
          if ! "$podman" exec otbr ot-ctl dataset info active 2>/dev/null | grep -q networkkey; then
            "$podman" exec otbr sh -c '
              ot-ctl dataset init new >/dev/null
              ot-ctl dataset commit active >/dev/null
              ot-ctl ifconfig up >/dev/null
              ot-ctl thread start >/dev/null
            ' || true
          fi
          exit 0
        '';
      };
    };
  };

  # The state directory has to exist before the container starts, or podman
  # creates it as an empty root-owned directory and the Thread network's
  # credentials are written into a tree the next start may not read. Same shape
  # and same reasoning as `home-assistant.nix`'s: not recursive, and no `+` (a
  # `+` would clear a directory that already holds a commissioned fabric).
  config.system.activationScripts.otbr-dir = lib.mkIf cfg.enable (lib.stringAfter ["users"] ''
    mkdir -p ${OTBR_STATE}
  '');
}
