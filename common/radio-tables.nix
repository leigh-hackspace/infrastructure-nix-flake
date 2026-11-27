# HELPER — the predicates over `radio-dongles.nix`, in one place.
#
# `common/check-radio-tables.nix` turns `problems` into a thrown verdict for
# `just check-eval` (and `flake.nix` wraps that same file as a flake check), and the
# three modules that read the table — `machines/services1/services/zigbee2mqtt.nix`,
# `machines/services1/services/thread-router.nix`,
# `machines/services1/services/matter-server.nix` — assert the same list is empty,
# so a deploy of the box that runs the radios cannot pass them with `just check`
# skipped. One implementation, five entry points: the shape
# machines/gw/lib/check-network-tables.nix and ./assert-network-tables.nix already
# use, and the reason a table cannot have its own copy of the rules in any of them.
#
# WHY A GUARD IS NEEDED AT ALL — the failure this refuses is invisible in every
# other way. zigbee2mqtt and the Thread border router each hand a dongle to a
# different stack, and the kernel hands out `/dev/ttyUSB<N>` in plug order, so a
# reboot or a re-plug can make the **Thread radio the Zigbee coordinator**. Nothing
# fails: zigbee2mqtt starts, its coordinator answers, and every Zigbee entity in
# Home Assistant arrives over MQTT from a mesh that is now half-dead. That is the
# class of fault `machines/services1/services/home-assistant.nix`'s header calls
# "two Zigbee stacks on one coordinator", and it is why that file names the device
# instead of passing `/dev`, and why this table's `device` values are `by-id` paths
# rather than numbers. Measured on services1 2026-11-26: one dongle on the box
# (`lsusb` answers `10c4:ea60 Silicon Labs CP210x UART Bridge`, `iManufacturer
# SMLIGHT`), and it is the live coordinator for the space's Zigbee mesh.
#
# The channel rules are here for the same reason a service on a busy channel is a
# service nobody can debug: Zigbee and Thread are both IEEE 802.15.4 on 2.4 GHz,
# and channels 11-26 overlap with the Wi-Fi this network is full of. Thread and
# Zigbee sharing a channel is only the same channel under MultiPAN — one radio
# chip running both, which Home Assistant marks deprecated and this network does
# not use — so with two radios the two networks belong as far apart as the
# standard allows. The measured pairing (Zigbee on 11, from `advanced: channel: 11`
# in /srv/zigbee2mqtt/configuration.yaml) is why the Thread row carries 26, and
# docs/thread-border-router-2026-11-26.md owns the receipts behind both numbers.
let
  RADIOS = import ./radio-dongles.nix;
  rows = builtins.attrValues RADIOS.radios;
  roles = ["zigbee-coordinator" "thread-rcp" "zigbee-router" "spare"];
  byRole = role: builtins.filter (r: r.role == role) rows;
  plugged = builtins.filter (r: r.device != "") rows;
  served = byRole "zigbee-coordinator" ++ byRole "thread-rcp";

  # A channel difference; nix has no `builtins.abs`.
  gap = a: b:
    if a > b
    then a - b
    else b - a;

  uniq =
    builtins.foldl'
    (acc: x:
      if builtins.elem x acc
      then acc
      else acc ++ [x])
    [];

  dupes = ls: uniq (builtins.filter (x: builtins.length (builtins.filter (y: y == x) ls) > 1) ls);

  # A device is either unplugged ("") or named the only way that survives a reboot.
  byIdPath = d: d == "" || builtins.substring 0 18 d == "/dev/serial/by-id/";

  # Null rather than a throw when the row does not exist: the row is what says
  # "this network has a Thread radio planned", and a network that has not planned
  # one is a question `thread-router.nix` answers with an empty node, not with an
  # eval that dies in someone else's message.
  threadRows = byRole "thread-rcp";
  thread =
    if threadRows == []
    then null
    else builtins.head threadRows;
  zigbeeChannels = map (r: r.channel) (byRole "zigbee-coordinator");

  problems =
    builtins.filter
    (p: p != "")
    [
      (
        if builtins.all (r: builtins.elem r.role roles) rows
        then ""
        else ''a radio row whose role is not one of zigbee-coordinator / thread-rcp / zigbee-router / spare: ${builtins.toJSON (builtins.filter (r: ! builtins.elem r.role roles) rows)}''
      )
      (
        if builtins.length (byRole "zigbee-coordinator") == 1
        then ""
        else "exactly one radio may play Zigbee coordinator; the table has ${builtins.length (byRole "zigbee-coordinator")}"
      )
      (
        if builtins.length (byRole "thread-rcp") <= 1
        then ""
        else "at most one thread-rcp row: /srv/otbr keeps one Thread network's credentials"
      )
      (
        if dupes (map (r: r.device) plugged) == []
        then ""
        else "two radio rows claim one device node: ${builtins.toJSON (dupes (map (r: r.device) plugged))}"
      )
      (
        if dupes (map (r: r.mountAt) served) == []
        then ""
        else "two stacks are handed the same mountAt: ${builtins.toJSON (dupes (map (r: r.mountAt) served))}"
      )
      (
        if builtins.all byIdPath (map (r: r.device) rows)
        then ""
        else "a radio named by a kernel number rather than by /dev/serial/by-id/...: ${
          builtins.toJSON (builtins.filter (r: ! byIdPath r.device) rows)
        }"
      )
      (
        if
          thread.channel
          != null
          && builtins.all (c: gap thread.channel c >= 8) zigbeeChannels
        then ""
        else "Thread must be on 802.15.4 (11-26) and at least 8 channels from every Zigbee channel — two 2.4 GHz meshes on adjacent channels noise each other, and the upstream picks are the two ends (Zigbee 11, Thread 26; the guide that starts Zigbee at 25 puts Thread at 15). 26 is not a law, the distance is: ${
          builtins.toJSON (map (r: {
              name = r.name;
              value = r.channel;
            })
            rows)
        }"
      )
      (
        # A Zigbee **router/repeater** (the Zigbee-Router firmware, no Z2M
        # instance behind it) joins the mesh it is a part of, so its channel is
        # that mesh's channel and its PAN is not its own; the rule this closes
        # is the one upstream states for it: a repeater is on the network, so
        # it answers on the network's channel, or it is a different network and
        # is not a repeater.
        if
          byRole "zigbee-router"
          == []
          || (
            zigbeeChannels
            != []
            && builtins.all (r: builtins.elem r.channel zigbeeChannels) (byRole "zigbee-router")
            && builtins.all (r: r.mountAt == null) (byRole "zigbee-router")
          )
        then ""
        else "a zigbee-router row must answer on one of the mesh's own channels (it joins that network, it does not start one) and must have no mountAt (no stack talks to a Zigbee-Router firmware): ${
          builtins.toJSON (byRole "zigbee-router")
        }"
      )
      (
        # The Thread row's `mountAt` is the name the container is *handed* — it ends
        # up inside `otbr-agent`'s own radio URL — so it must not be a kernel node
        # name. Two measured reasons, one file over: the kernel hands `/dev/ttyUSB<N>`
        # out in plug order and re-hands it on every reboot (which is why `device` is
        # a by-id path), and podman resolves a symlink `--device` **source**, so naming
        # only the by-id path lands the node inside the container at `/dev/ttyUSBn`
        # while the URL still says the by-id name (measured 2026-11-27: `otbr-agent`
        # ENOENTing at `hdlc_interface.cpp:157` in a crash loop whose journal named the
        # URL and therefore read as a device problem). The explicit
        # `source:target` form in `thread-router.nix` is what makes the pair agree.
        if
          thread
          == null
          || thread.mountAt
          == null
          || thread.mountAt
          == ""
          || builtins.match "/dev/ttyUSB[0-9]+" thread.mountAt
          == null
        then ""
        else "the thread-rcp row mounts at a kernel node name (a plug-order number in front of a container, and inside `otbr-agent`'s own radio URL): ${thread.name}. Mount at a stable container-side name like /dev/otbr-rcp and bind it with `--device=<by-id>:/dev/otbr-rcp`"
      )
      (
        if builtins.all (r: r.channel == null || (r.channel >= 11 && r.channel <= 26)) rows
        then ""
        else "a channel outside 802.15.4's 11-26: ${
          builtins.toJSON (builtins.filter (r: r.channel != null && (r.channel < 11 || r.channel > 26)) rows)
        }"
      )
    ];
in {
  inherit rows byRole problems thread zigbeeChannels dupes uniq gap byIdPath;
  radio = byRole;
}
