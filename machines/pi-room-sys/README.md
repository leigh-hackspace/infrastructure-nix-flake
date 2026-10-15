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
   ├─ /etc/netboot.ipxe       the iPXE menu: kernel + initrd + ${build.toplevel}/init + kernelParams
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

`nfsServer` is an IP literal in `flake.nix` and the hostname is used for the iPXE
URLs in `machines/aibox/netboot.nix`.  That split is inherited and deliberate: the
comment that came with the tree says *"DNS is too slow so use IP address"*, and it
applies to the store mount, which is on the boot path, not to the menu.

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
