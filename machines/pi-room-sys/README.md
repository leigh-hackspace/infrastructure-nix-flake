# `pi-room-sys` — the image the Pi Room computers netboot

A machine in every sense this flake cares about: one `nixosConfiguration`, its own
modules, its own hardware assumptions.  What makes it odd is that **nobody ever
runs a command on it and it has no disk.**  The Pi Room's PCs PXE-boot this image
off aibox and mount their entire Nix store, read-only, back off aibox over NFS.
So it is *built* by aibox rather than deployed to a host, and `nixos-rebuild
--flake .#pi-room-sys` would build an image and then do nothing with it.

## How it is wired

```
flake.nix  piRoomSys = nixosSystem { modules = [ configuration.nix (netboot.nix {nfsServer = …;}) … ]; }
   ├── nixosConfigurations.pi-room-sys = piRoomSys;      # so `nix flake check` evaluates it
   └── specialArgs.piRoomSys ──────────┐                 # (an attrset cannot pass a sibling)
                                       ▼
machines/aibox/netboot.nix   sys = specialArgs.piRoomSys; sys.config.system.build.{toplevel,kernel,netbootRamdisk,squashfsStore}
   ├─ /etc/netboot.ipxe       the iPXE menu: kernel + initrd URLs + `init=/nix/store/.netboot-closure/init` + kernelParams
   ├─ nginx  /boot/bzImage, /boot/initrd, /boot/logo.png
   ├─ NFS    /exports/netboot-squashfs  ← ${build.squashfsStore}   (the store, read-only)
   └─ NFS    /exports/nix-store         ← /nix/store (bind; the commented-out alternative mount)
```

At boot the client: iPXE → `bzImage` + `initrd` from aibox → in the initrd, mount
`10.3.1.32:/netboot-squashfs` (NFSv4.2, `rsize=1 MiB`, `hard`, `actimeo=86400`),
loop-mount `squashfs.squashfs` out of it, and overlay `/nix/store` on top of a
tmpfs.  Root is tmpfs; everything persistent is either the store on NFS or
whatever the user mounts at login (`configuration.nix`'s PAM script mounts the
NAS filestore into the user's home).

**The menu names no generation, on purpose.** The image is swapped by
`nixos-rebuild switch` on aibox about a second after `/etc/netboot.ipxe` is
replaced, and the client mounts it a minute later, so the two are not from the
same generation reliably.  So `init=` is the constant
`/nix/store/.netboot-closure/init`, and the image carries a `.netboot-closure`
symlink to *its own* closure (`netboot.nix`'s `pseudoFiles`); whichever image the
client mounted resolves.  On 2026-10-21 the menu named a store path instead: a
client fetched the menu at 18:28:05, mounted the next generation's store at
18:29:36, and stage 1 died in `initrd-find-nixos-closure` — `Failed to resolve
path /nix/store/<old-init>/init in prefix /sysroot: No such file or directory` —
which reads at the console like a failed NFS root (it was not; the mount had
succeeded).  Do not put a store path back in the menu; the same failure comes
back the next time aibox is switched while a client boots.

`nfsServer` is an IP literal in `flake.nix` and the hostname is used for the iPXE
URLs in `machines/aibox/netboot.nix`.  That split is inherited and deliberate: the
comment that came with the tree says *"DNS is too slow so use IP address"*, and it
applies to the store mount, which is on the boot path, not to the menu.

**A name on the boot path is a promise about *both* address families, and that promise
broke the netboot for five days.**  The client resolves `aibox.int.leighhack.org`
itself, through the router, and dnsmasq answers it with `common/dns-records.nix`'s
whole `ipv6` list along with the A record.  Since the 2026-10-21 cutover nothing
*assigns* those addresses — the retired appliance honoured them as per-host static
DHCPv6 bindings (fourteen in all, no `range6`, so never a pool) and
`machines/gw/dns.nix` runs neither that nor the NPT66 the `*d::/64` ones depended on
(`option6:23,[::]`), so aibox SLAACed into
its EUI-64 address and kept answering nothing on `2001:8b0:1d14:225::32`.  iPXE SLAACs
too, prefers the AAAA, and has no fallback to try: it retransmits a SYN into a /64
nobody answers until the transfer times out (`Connection timed out`,
https://ipxe.org/4c0a6092) and the boot dies with no menu on screen.  A browser shrugs
and goes back to IPv4; a PXE ROM does not.  The last line in aibox's nginx access log
is 2026-10-21 18:45 and the first of the fix is
`machines/aibox/networking.nix` configuring the two addresses the record advertises —
a fact about the box that used to be true only of the DHCP server that is gone.  The
network-wide version of the same lie (every `::N` in the table that no interface now
holds) is an open item in `docs/follow-ups.md`.

## Testing a boot without a board

`just pxe-client-bios` / `just pxe-client-uefi` do not run: `qemu-bridge-helper` is not
in aibox's system, so `helper=$(type -p qemu-bridge-helper)` expands to nothing and
qemu refuses the netdev (open item in `docs/follow-ups.md`).  What works is a tap
slaved into `br227` — the bridge aibox already has on VLAN 227, which is why the
clients' DHCP, TFTP and HTTP all cross the router for real — plus the `OVMF.fd`
`FV/OVMF_CODE.fd` + `FV/OVMF_VARS.fd` pflash pair for the EFI path.  Run it on aibox, headless, and read the serial log and aibox's
nginx access log:

```bash
sudo ip tuntap add dev tap-pxe mode tap
sudo ip link set tap-pxe master br227
sudo ip link set tap-pxe type bridge_slave state forwarding   # it is `disabled` until you say so
sudo ip link set tap-pxe up
sudo timeout 130 qemu-system-x86_64 -m 2048 -cpu host -accel kvm -smp 2 \
  -machine q35,smm=on -global driver=cfi.pflash01,property=secure,value=on \
  -netdev tap,id=net0,ifname=tap-pxe,script=no,downscript=no \
  -device virtio-net-pci,netdev=net0,mac=52:54:00:aa:bb:dd \
  -drive file=$OVMF/FV/OVMF_CODE.fd,if=pflash,format=raw,unit=0,readonly=on \
  -drive file=$OVMF/FV/OVMF_VARS.fd,if=pflash,format=raw,unit=1 \
  -boot n -nographic -no-reboot
sudo ip link del tap-pxe
```

A BIOS client (`-boot n` with no OVMF, which uses SeaBIOS's own iPXE and has no IPv6)
walks straight past a broken AAAA, so **the BIOS VM passing is not evidence**: boot the
EFI one, because the boards are UEFI and it is the one that resolves the name over
IPv6.  A run that gets as far as the menu prints `Filename:
http://aibox.int.leighhack.org/boot/netboot.ipxe` and then goes quiet, because the menu
script's own `console --picture` moves the console off the serial port — finish the
judgement from `/var/log/nginx/access.log`, which should then show `netboot.ipxe`,
`logo.png`, `bzImage` and `initrd` from the client's address.  The client gets its lease
from the *dynamic* pool (`10.3.14.150-.253`), so the test costs a lease and nothing else;
capture on `gw`'s `vlan227` rather than on `br227` if you want the server's replies too,
because the switch only floods the broadcasts to aibox's port.

## Files

| File | What it is |
| --- | --- |
| `configuration.nix` | the system: users, PAM mount-on-login, display/inputs, the packages these boxes need |
| `netboot.nix` | the netboot half: initrd network + NFS + overlay, the squashfs store image, initrd SSH, kernel modules for the client NICs.  A parameterised module — `(import ./netboot.nix) { inherit nfsServer; }` |
| `make-squashfs.nix` | builds the store image (`squashfsStore`) aibox exports; `zstd -Xcompression-level 10` |
| `leigh-logo.png` | served as `/boot/logo.png` and pulled by the iPXE menu |
| `ssh_host_ed25519_key` | **a private key, in the open.**  See below |

## Two things to know before you touch it

* **`boot.initrd.network.ssh` is on, with `emergencyAccess = true`.**  Anyone who
  can PXE gets an initrd with an SSH server, the two `authorizedKeys` in
  `netboot.nix`, a root shell on the emergency console with no password, and a
  read mount of the store.  On the Pi Room VLAN that is a lab convenience, not an
  incident, but it is not nothing: these keys are the access list.
* **`ssh_host_ed25519_key` is a private host key committed to the repo.**  It came
  with the tree (it was committed in `leigh-hackspace/pi-room-sys` before the move)
  and it has to be a real file at build time, so it moved as-is rather than
  breaking every eval that reads aibox.  It should become a sops secret rendered
  through `config.sopsSecretText` (`common/sops.nix`) and then be rotated —
  anyone who can netboot can read the initrd.  Tracked in `docs/follow-ups.md`.

## Provenance and what the move changed

This tree was `~/Projects/pi-room-sys`, a separate git repo (`main`, remote
`github:leigh-hackspace/pi-room-sys`, one commit) pulled in as
`git+file:///home/leigh-admin/Projects/pi-room-sys`.  It moved here on
2026-10-15, which changed three things:

1. Its `flake.nix` is gone: the single `nixosConfiguration` it exported is now
   `nixosConfigurations.pi-room-sys` in this flake, and `resources.leigh-logo`
   became a path.  `nix-software-center` moved up with it as an input of *this*
   flake.
2. Its own `nixpkgs` pin (`nixos-25.05`) never applied.  `flake.lock` carried
   `nixpkgs follows input 'nixpkgs'` for this input, so the image has always been
   built against the fleet's nixpkgs — which is what it still is.
3. `nix flake update pi-room-sys` (the old `just update-netboot-input`) no longer
   exists.  The tree is the checkout: edit it and rebuild aibox.

Uncommitted at the time and carried over: `netboot.nix`'s `authorizedKeys` are
two ed25519 keys (`admin@leighhack.org`, `leigh-admin@aibox`) rather than the
`cjdell@gmail.com` RSA key the single commit recorded — so the image aibox was
building before the move had the old key.  Left behind as unused: an untracked
`default.nix` (a hand-callable duplicate of the flake's output) and a
`.vscode/settings.json` that set `nixfmt`, which is not this repo's formatter.

## Building it

It is inside aibox's closure already — `${build.squashfsStore}` is an export
directory and `${build.kernel}/bzImage` is served by nginx — so `just switch` on
aibox builds it, and `just check-build` covers it without naming it.  To rebuild
just the image: `nix build .#nixosConfigurations.pi-room-sys.config.system.build.toplevel`.
After a switch, `just switch-netboot` (on aibox) remounts the export and reboots
the clients; they are `10.3.14.101`–`10.3.14.110`.
