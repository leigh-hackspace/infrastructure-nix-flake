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
  # `just switch-gw` (run from services1, against this box) and every debugging
  # session reach it with, and that is the whole reason the login exists. Root
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

  # The deploy path, and the one setting that makes it work.
  #
  # The router is deployed by building *elsewhere* and pushing: `just switch-gw`
  # from services1 (or aibox) runs `nixos-rebuild switch --flake .#gw
  # --target-host leigh-admin@10.3.1.1 --elevate=sudo`, which builds there, copies
  # the closure over and activates over ssh. That is still the way, and it is no longer
  # the only safe way: this box now delegates to aibox and runs its daemon with
  # `nix.settings.max-jobs = 0` (./default.nix), so a local `nixos-rebuild` cannot compile the
  # crate ./network-status.nix adds here — that crate is built on aibox and copied back.
  # What a local switch still costs is the **evaluation**, on the box that answers DNS
  # and DHCP for every VLAN, which is why the checkout, `git` and `just` here stay for
  # reading the tree and for the case where services1 is the thing that is broken, not
  # for routine deploys.
  #
  # The push path needs daemon *trust*, not daemon *access*: `allowed-users = *`
  # already admits the client, but with `require-sigs` on (the default) the daemon
  # refuses paths built elsewhere and the copy dies with
  #
  #   cannot add path '…' because it lacks a signature by a trusted key
  #
  # which is exactly what it did, on the first deploy after install. It is the same
  # answer common/build-farm.nix gives on services1 and services2, for the same
  # reason.  Without this line `just switch-gw` cannot work at all.
  #
  # What it costs is nothing that sudo does not already cost: `leigh-admin` has
  # passwordless sudo on every machine in this network (common/users.nix), so a
  # client holding the hop key is already root here. Trusted-user rights — adding
  # unsigned paths, setting nix.conf at build time — are a weaker grant than the one
  # it already has. What it must never be confused with is `nixbld`: this is not
  # build-user membership, and common/nixbld.nix still refuses that. Since this box got
  # a builder, common/build-farm.nix adds the same string for its own reason, so the
  # rendered list names `leigh-admin` twice — left doubled on purpose: the grant belongs
  # here, because the push path needs it whether or not the builder half is configured.
  nix.settings.trusted-users = ["leigh-admin"];

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
  # pair — see flake.nix's comment on the gw system).  They are no longer the
  # deploy path — `just switch-gw` builds on services1/aibox and pushes — but the
  # box still carries a checkout as a mirror, and reading the tree or the tailnet
  # from the box itself is how a problem is diagnosed when the admin's laptop is
  # the thing that cannot reach it:
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
