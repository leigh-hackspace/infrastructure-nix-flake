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

    # The Sonoff spare the network was asked about on 2026-11-26: a ZBDongle-P,
    # i.e. a Texas Instruments CC2652P. It is `spare`, and it stays `spare`: it
    # can be a second Zigbee coordinator, and it is NOT a Thread radio. Why is
    # measured, not folklore — see `docs/thread-border-router-2026-11-26.md` and
    # the pins in `radios/firmware-pins.json`, which carry this row's firmware.
    zbdongle-p = {
      name = "Sonoff ZBDongle-P (TI CC2652P)";
      role = "spare";
      device = "";
      mountAt = "/dev/ttyUSB1";
      chip = "texas-instruments CC2652P (352 KB flash / 80 KB RAM)";
      channel = null;
      note = "a Zigbee spare for the row above; the Thread route is a measured dead end for it, docs/thread-border-router-2026-11-26.md";
    };

    # The Thread border router's radio. `device` is empty on 2026-11-26 because
    # the dongle that can do it is not on this network yet — the row is a
    # placeholder with a price tag in the doc, and `common/check-radio-tables.nix`
    # plus `thread-router.nix`'s own assertion refuse anything that would start a
    # border router on an unnamed radio. Fill `device` with what `just radio-plug`
    # prints once the MG24-class dongle is in a USB port, and `mountAt` stays
    # /dev/ttyACM0: that is the path the container is handed, so nothing else in
    # the flake has to know the node.
    thread-rcp = {
      name = "Thread border router radio (EFR32MG24-class)";
      role = "thread-rcp";
      device = "";
      mountAt = "/dev/ttyACM0";
      chip = "";
      channel = 26;
      note = "OpenThread RCP radio for the Matter-over-Thread mesh; 2026-11-26";
    };
  };
}
