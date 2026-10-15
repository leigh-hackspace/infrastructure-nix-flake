# The 3D print servers (hackspace LAN, 10.3.14.0/24)

Moved out of `AGENTS.md` on 2026-10-14. `AGENTS.md` keeps the rules: these targets
are **fixed IPv4 on purpose** (never make the exporter depend on DNS here), the SSH
user is `hackspace`, and the two Pis share credentials because they were cloned.
This file is the access detail and the identity history.

## The two boxes

`3d-blue` (**10.3.14.62**) and `3d-lime` (**10.3.14.61**) are Raspberry Pi 3 print
servers running **Klipper + Moonraker** — Mainsail on nginx `:80`, the Moonraker
API on `:7125`.

`moonraker-exporter` polls their APIs; the targets are the "blue"/"lime" entries in
`machines/services1/services/printer-monitoring.nix`, and
`common/printer-states.nix` / `common/check-printer-states.nix` hold the expected
per-printer state assertions that `just check` runs.

**Targets are fixed IPs so the exporter never depends on DNS** — historically
necessary because the two Pis were cloned from one SD card (see the identity note
below), which made the `3d-*` names advertise each other's IPv6 addresses. Lime's
machine-id and SSH host keys were regenerated on 2026-10-03 (blue still carries the
old identity), but fixed IPv4 is deliberately kept regardless.

## The printers and their reservations

The two boxes above are *print servers* (a Pi bolted to a printer). The printers
with their own network interface are reserved in the router's dnsmasq config too,
and the ones on WiFi are on the **WIFI VLAN (10.3.2.0/24), not the wired
10.3.14.0/24** — a reservation in the wrong VLAN is inert, so check which one the
device is on before adding one (`arp -an | grep -i <mac>` on the router answers it
by the interface name):

| Name | Address | Note |
| --- | --- | --- |
| `voron` | 10.3.2.50 | Voron, WiFi |
| `elegoo` | 10.3.2.55 | Elegoo resin printer, WiFi |
| `snapmaker` | 10.3.2.56 | **Snapmaker U1**, MAC `28:2d:06:1d:bc:96`, WiFi (announces itself as `U1`) |
| `3d-lime` / `3d-blue` | 10.3.14.61 / 10.3.14.62 | the Pi print servers, wired |

The U1 is a closed appliance (Snapmaker's own app/cloud, no Moonraker), so it is
**not** a `moonraker-exporter` target and never will be; the reservation exists so
the slicer can find it at a fixed address. Adding one: `docs/router-and-dns.md` §
"Adding one by hand".

## SSH access (the permanent method)

The Pi's user is **`hackspace`** (not `pi`/`root`) and the host is keyed with the
machine-hop key:

```bash
ssh -i ~/.ssh/agent-hop-key hackspace@10.3.14.62   # 3d-blue
ssh -i ~/.ssh/agent-hop-key hackspace@10.3.14.61   # 3d-lime
```

Klipper config and logs live under `/home/hackspace/printer_data/`
(`config/printer.cfg`, `logs/klippy.log`, `logs/moonraker.log`). The MCU is an
Arduino Mega 2560 clone on a **CH340** USB-serial adapter
(`/dev/serial/by-id/usb-1a86_USB_Serial-if00-port0`). Logs can also be read
unauthenticated via Moonraker on the trusted LAN:
`http://10.3.14.62:7125/server/files/logs/klippy.log`.

## Cloned SD identity (lime fixed, blue pending)

The two Pis were cloned from one SD card, so they shared `/etc/machine-id` (⇒ the
same NetworkManager DHCPv6 DUID) and the same SSH host keys, and the `3d-*` names
advertised each other's IPv6 addresses.

Lime's identity was regenerated on 2026-10-03:

```bash
sudo rm /etc/machine-id && sudo systemd-machine-id-setup
sudo rm /etc/ssh/ssh_host_* && sudo ssh-keygen -A
# reboot
```

It now has its own DUID and a `…::135b` lease, and `3d-lime.int` no longer answers
as blue. **Blue was off-network at the time and still carries the original cloned
identity** — harmless now that lime changed, but regenerate it too if blue is ever
re-imaged.

**The `hackspace` user's password and `authorized_keys` are also shared by the
clone.** Rotate them if the two boxes should not be interchangeable.

## Write-ups

- `docs/3d-blue-klipper-log-review-2026-10-03.md` — the klippy.log investigation.
- `docs/3d-lime-link-stress-harness-2026-10-03.md` — the link stress harness.
