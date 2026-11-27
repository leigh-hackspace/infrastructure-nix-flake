# DATA — every 802.15.4 radio this network owns, which role it plays, and the
# device node it is plugged into.
#
# One table because a dongle is the one fact in this flake that two stacks fight
# over. `machines/services1/services/zigbee2mqtt.nix` and
# `machines/services1/services/thread-router.nix` both read it and neither of
# them writes a `/dev/ttyUSB<N>` of its own: a bare ttyUSB number is handed out
# in plug order and is re-handed on every reboot, so naming a dongle by number
# is how the Thread radio becomes the Zigbee coordinator overnight and the
# whole mesh goes dark with nothing failed anywhere — the fault `home-assistant.nix`'s
# header warns about ("winning the serial port is how HA moves and the whole
# mesh goes dark") and the reason the guide in
# `docs/thread-border-router-2026-11-26.md` gives for using the `by-id` name.
#
# `device` is `/dev/serial/by-id/<vendor>_<model>_<serial>-if00-port0`, read off
# the box (`ls -l /dev/serial/by-id`) — and `just radio-plug` is the verb that
# re-reads it, so replacing a physical dongle has a way back that is not a guess.
# An empty `device` means "not plugged in"; it is a value rather than a missing
# key because `common/check-radio-tables.nix` asserts about it: a Thread border
# router whose radio has no name is refused, and silence is not a pass.
#
# `mountAt` is the node the container is handed. zigbee2mqtt's runtime config
# (`/srv/zigbee2mqtt/configuration.yaml`, `serial: port: /dev/ttyUSB0`) is
# runtime state the flake does not edit, so the Zigbee row keeps mounting at the
# path that file already names; what changes is that the flake now says which
# dongle answers that name instead of trusting the kernel's ordering.
#
# `channel` is the IEEE 802.15.4 channel a network runs on. Zigbee and Thread
# share the 2.4 GHz band and channels 11-26 overlap with Wi-Fi, so this number
# is a fact about the whole network rather than a knob in either service: zigbee2mqtt
# runs the space's Zigbee mesh on **11** (`advanced: channel: 11` in its
# configuration.yaml, `pan_id: 12345`), so Thread takes **26** — the channel
# least likely to collide, and far enough from 11 to keep the signal-to-noise
# margin. A Thread network on channel 11 next to the Zigbee network on 11 is not
# a harmless choice: the two radios drown each other, and every forum post that
# says both networks must share a channel is talking about MultiPAN, i.e. one
# radio chip for both, which is the arrangement Home Assistant calls deprecated
# and this network does not use. One radio per job, per
# docs/thread-border-router-2026-11-26.md.
#
# The roles are what the guard reads. Exactly one `zigbee-coordinator` (two
# stacks on one coordinator is a race, not a configuration), at most one
# `thread-rcp`, `zigbee-router` rows are the *repeaters* — a dongle flashed with a
# Zigbee-Router firmware, which has no stack behind it (`mountAt = null`) and
# therefore answers on the channel of the mesh it belongs to rather than starting
# its own — and `spare` rows are inert and expected to have no device.
#
# What a role is for, specifically: a flash changes what a dongle *does* and changes
# nothing about how it identifies itself. A Sonoff flashed to OpenThread RCP, or to a
# Zigbee-Router repeater, still answers as a Sonoff Zigbee dongle in `/dev/serial/by-id`
# — the vendor and product ids are the hardware's, not the firmware's, and this repo has
# measured that shape on the live coordinator (`usb-SMLIGHT_SMLIGHT_SLZB-06M_…` is the
# by-id name of a dongle that has been flashed and re-flashed). So the device name cannot
# tell a Thread radio from a Zigbee stick, and **this table's `role` is the fact that
# survives a flash.** A row whose role lies about a dongle in a USB port is how a
# repeater becomes a coordinator overnight and the mesh gets a second network key.
{
  radios = {
    # The live Zigbee coordinator: SMLIGHT's SLZB-06M PoE dock in the USB slot on
    # this box. Measured on services1 2026-11-26: `lsusb` answers `10c4:ea60
    # Silicon Labs CP210x UART Bridge` with `iManufacturer SMLIGHT`, and the only
    # node on `/dev/serial/by-id` is the row below. It carries the space's Zigbee
    # mesh — the sensors and switches named in /srv/zigbee2mqtt/configuration.yaml —
    # so a flash that takes this radio away takes every one of those devices with
    # it. This row is the reason nobody may.
    slzb-06m = {
      name = "SMLIGHT SLZB-06M";
      role = "zigbee-coordinator";
      device = "/dev/serial/by-id/usb-SMLIGHT_SMLIGHT_SLZB-06M_be6f53687971ed11affe22f3fdf7b791-if00-port0";
      mountAt = "/dev/ttyUSB0";
      chip = "silicon-glass EFR32 (CP210x bridge)";
      channel = 11;
      note = "the space's Zigbee mesh: rack_sensor, main_space_sensor, workshop_sensor, main_space_heater, fabrication_floodlight_switch, … — 2026-11-26";
    };

    # The dongle this network's Thread radio is built on: the spare Sonoff ZBDongle-P,
    # a Texas Instruments **CC2652P10** (shield silkscreen "C2652P"). It is the
    # `thread-rcp` row, and it is here because the route was **measured to work** on
    # the sibling fleet (2026-11-26, `cjdell/nixos-config`: flashed with the launchpad
    # `ot_rcp` image below, and `otbr-agent --radio-version` answered
    # `OPENTHREAD/1.4.0-Koenkk-2025.3.1; CC13XX_CC26XX`), which retires the "this
    # dongle can never be a Thread radio" verdict this repo wrote on 2026-11-26 — see
    # that doc §13 for what was believed before and what was measured instead. What
    # survives from the old read is the *caveat*: upstream's own tracker reports a
    # CC2652P-class RCP losing the mesh when a Thread device **joins**
    # (`RadioSpinelNoResponse`), and that is what `just thread-status` has to answer
    # with node counts and link qualities before `infra.thread.enable` is worth flipping.
    # `device` is still empty — this box has one dongle plugged in, and it is the
    # Zigbee coordinator above — and `just radio-plug` prints the name to fill once
    # this stick is in a port; the assertion in `thread-router.nix` is what refuses a
    # deploy while it is empty, so an unnamed radio is silence, not a pass.
    # `mountAt` is the **container's** name, not the kernel's: `thread-router.nix`
    # binds `--device=<by-id>:/dev/otbr-rcp`, and the explicit target half is
    # required — measured 2026-11-27 on the sibling fleet that podman resolves a
    # *symlink* `--device` source, so a bare `--device=/dev/otbr-rcp` landed the
    # node inside the container as `/dev/ttyUSBn` and `otbr-agent` ENOENT'd on its
    # radio URL (`hdlc_interface.cpp:157`, a crash loop). Naming the kernel's node
    # instead would put a plug-order number in front of a container again.
    zbdongle-p = {
      name = "Sonoff ZBDongle-P (TI CC2652P10)";
      role = "thread-rcp";
      device = "";
      mountAt = "/dev/otbr-rcp";
      chip = "texas-instruments CC2652P10 (352 KB flash / 80 KB RAM; CP2102N bridge)";
      channel = 26;
      note = "the Thread radio candidate on this network's own spare; RCP firmware pinned in radios/firmware-pins.json, flash via just thread-flash, docs/thread-border-router-2026-11-26.md §13";
    };

    # The upgrade path, not the requirement: the CC2652P-class radio above works
    # with a caveat, and the silicon that does not carry that caveat is EFR32MG24-class
    # (256 KB RAM / 1536 KB flash — the numbers in `radios/README.md`, which are
    # measured off the fetched artifacts, not off a guide). `spare`, no device,
    # no channel: it is a row so the guard still sees one Thread radio (this
    # network has one Thread network, hence one credential set), and the row
    # that gets a device when such a dongle is bought is this one, not the row
    # above, because at most one `thread-rcp` row may exist.
    efr32-mg24 = {
      name = "EFR32MG24-class Thread radio (the upgrade, not bought yet)";
      role = "spare";
      device = "";
      mountAt = null;
      chip = "silicon-glass EFR32MG24 (256 KB RAM / 1536 KB flash)";
      channel = null;
      note = "pinned firmware: silabs-mg24-rcp in radios/firmware-pins.json; a purchase, and un-parking it means making THIS row the thread-rcp and the ZBDongle-P a spare";
    };
  };
}
