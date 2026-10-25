# gw-specific system configuration.  The settings shared with the other
# machines (timezone, i18n, keyboard, sudo, nix-ld, ssh, bootloader,
# autoRollback, experimental-features) live in common/base.nix.
#
# This box is the network's router (./README.md: the Supermicro X9SCL/X9SCM at
# 10.3.1.1, cut over from the OPNsense appliance on 2026-10-21).  It is flake-
# managed like every other machine here, which is what ./README.md is for: what is
# proven on the box, what was read off the old one, and what must not move without
# reading it first.
{
  config,
  lib,
  pkgs,
  ...
}: {
  imports = [../../common/base.nix];

  # The router is a single-purpose box: one process group (dnsmasq, pppd,
  # openvpn, nft, network-status) and no operator on a console.  It still gets
  # `leigh-admin` from common/users.nix — the inter-machine ssh key is what
  # `just deploy gw` and every debugging session reach it with, and that is the
  # whole reason the login exists. Root
  # is not keyed here: connect as `leigh-admin` and `sudo`. network-status used
  # to need the key too; it now runs here and reads /proc as a DynamicUser.  Nothing here needs a GUI, so common/base.nix's
  # xserver keyboard settings are inert.

  # The router answers for the whole network, so it needs no NAS: it is not a
  # share, not a backup target, and nothing on it mounts anything.  common/nas.nix
  # is deliberately NOT in machines/gw/default.nix, so `infra.nas` is not even an
  # option here — that is what keeps the automount units and the wait-for-nas
  # dependency out of this box's boot, which matters because the router is the
  # thing the network's DNS depends on and a NAS that is still importing after a
  # power cut must not be able to stop it answering.

  # The deploy path, and the settings that make it work.
  #
  # **This box deploys itself**, like every other machine in the fleet: `just deploy gw`
  # from anywhere (or `just switch` here) fetches the commit into this checkout, runs
  # `nixos-rebuild switch --flake .#gw` *here* and `sudo nixos-confirm` here, in one shell
  # invocation.  What makes that possible without compiling anything is ./default.nix's
  # `infra.buildFarm.builderHost` + `nix.settings.max-jobs = 0`: the crate
  # ./network-status.nix adds is built on aibox and copied back, and a build aibox cannot
  # take fails instead of falling back to a local compile.  What a switch here costs is the
  # **evaluation**, on the box that answers DNS and DHCP for every VLAN.
  #
  # Until 2026-10-25 the routine path was the opposite direction — build on services1, push
  # with `nixos-rebuild --target-host --elevate=sudo`, then `nixos-confirm` over ssh.  That
  # is what this file's hand-set `nix.settings.trusted-users` existed for: a *push* needs
  # daemon **trust**, not daemon access (`allowed-users = *` admits the client, but with
  # `require-sigs` on, the daemon refuses a path built elsewhere — `cannot add path '…'
  # because it lacks a signature by a trusted key`, which is what the first deploy after
  # install did).  Nothing pushes to this box any more, so the line is gone and the grant
  # comes from `common/build-farm.nix`, which adds `leigh-admin` on any box that has a
  # builder — this one included.  What it must never be confused with is `nixbld`: this is
  # not build-user membership, and common/nixbld.nix still refuses that.  The checkout,
  # `git` and `just` here stay for reading the tree and for the case where another box is
  # the thing that is broken.

  # The old box ran OPNsense's pkg-config stack; nothing else here is a package
  # the other machines build, and the crate toolchain in the devshell is not
  # needed: the one Rust crate in this box's closure (./network-status.nix) is
  # built by the machine that pushes the deploy.  The diagnostics worth having are
  # the ones that answer a question about a packet: `dig` for the records dnsmasq
  # generates, `nft` for the rendered ruleset (the rules are loaded from a
  # generated file, and reading it back is the only way to check what the firewall
  # actually says — see ./README.md), `tcpdump` for the VLAN tags on the wire, and
  # `ip`/`ss` from the systemd set.  pppd and dnsmasq come in by their own modules;
  # `bind` is here for `dig` alone.
  #
  # git/just/jq are not diagnostics either, and they are here rather than in
  # common/tools.nix because that file is the one this machine must not import (its
  # wait-for-network unit reads the NAS host, so tools.nix and nas.nix come as a
  # pair — see flake.nix's comment on the gw system).  They are not a build toolchain:
  # the box deploys itself now (`just deploy gw` / `just switch` here — evaluation on this
  # box, compilation on aibox), and reading the tree or the tailnet from the box itself is
  # how a problem is diagnosed when the admin's laptop is the thing that cannot reach it:
  #
  #   * `git` — `--flake .` needs a git tree; without it this box cannot evaluate
  #     the flake it is running (not even the emergency local build).
  #   * `just` — so the checkout's recipes are readable and `just check` (eval and
  #     format only) works here; the build-shaped ones refuse on gw.
  #   * `jq` — `tailscale status --json` and `tailscale debug prefs` are the only
  #     way to prove this box is advertising the routes it means to (see
  #     ./uplink.nix's `extraUpFlags` comment: the prefs are what the daemon
  #     reports, and they are not re-asserted by a switch), and
  #     reaching for `nix run` to read it mid-debug takes a store path and the
  #     daemon lock, which is the wrong trade on a box whose job is answering.
  environment.systemPackages = with pkgs; [
    bind
    nftables
    tcpdump
    git
    just
    jq
  ];

  # The router keeps no time of its own that anyone should trust: it is a board in
  # a cupboard with a coin cell at best, and its idea of "now" arrives with the
  # uplink.  So it takes what NixOS enables by default (systemd-timesyncd, spelled
  # here so the next reader looks for the option rather than for a service this file
  # invented) and nothing more.  Worth saying because a router whose clock drifts
  # breaks its own TLS notary/log reads and every log line an operator reads after a
  # power cut.  And it must not become the network's NTP source: a box that answers
  # DNS for a hundred devices is the wrong thing for them to ask the time from,
  # because when it reboots they have no name to ask (the old box was not a time
  # source either — its dnsmasq conf answers DNS and nothing else).
  services.timesyncd.enable = true;

  # GOLDEN RULE: common/base.nix sets system.autoRollback.enable for every deployable
  # deployed machines; this one inherits it, so a switch here needs nixos-confirm
  # in the same shell invocation like everywhere else.

  # The NixOS release this system's stateful data was first laid out under:
  # 26.05, the installer the disk was partitioned and formatted from (see
  # ./README.md's install notes).  It is *not* the release of the nixpkgs this
  # flake evaluates, and it must not be copied from another machine either — the
  # value is a fact about this disk, and the thing it protects (file locations,
  # service defaults that moved at a release boundary) is a fact about what is
  # already on it.
  system.stateVersion = "26.05";
}
