# gw-specific system configuration.  The settings shared with the other
# machines (timezone, i18n, keyboard, sudo, nix-ld, ssh, bootloader,
# autoRollback, experimental-features) live in common/base.nix.
#
# This box exists (./README.md: the Supermicro X9SCL/X9SCM installed at
# 10.3.1.254) and it is deployed *beside* the live router, not in its place —
# ./coexist.nix is that, and ./README.md says what is proven on the box, what is
# still read off the old one, and what has to move at the cutover.
{
  config,
  lib,
  pkgs,
  ...
}: {
  imports = [../../common/base.nix];

  # The router is a single-purpose box: one process group (dnsmasq, pppd,
  # openvpn, nft) and no operator on a console.  It still gets `leigh-admin`
  # from common/users.nix — the inter-machine ssh key is what dns-sync and
  # network-status use to reach it, and that is the whole reason it exists.
  # Nothing here needs a GUI, so common/base.nix's xserver keyboard settings
  # are inert.

  # The router answers for the whole network, so it needs no NAS: it is not a
  # share, not a backup target, and nothing on it mounts anything.  common/nas.nix
  # is deliberately NOT in machines/gw/default.nix, so `infra.nas` is not even an
  # option here — that is what keeps the automount units and the wait-for-nas
  # dependency out of this box's boot, which matters because the router is the
  # thing the network's DNS depends on and a NAS that is still importing after a
  # power cut must not be able to stop it answering.

  # The fallback deploy path, and the one setting that makes it work.
  #
  # This box deploys itself like the other three: it has the checkout at
  # ~/Projects/infrastructure-nix-flake, `git`/`just` above, and root's
  # `safe.directory` set for the repo (the imperative prerequisite
  # docs/network-upgrade-runbook.md checks for on every machine), so `just switch`
  # there means what it means here. That is the way to use it, and `just switch-gw`
  # is the one-command form of it from services1.
  #
  # The fallback matters on a router specifically: if the thing that is broken is
  # this box's ability to evaluate or build its own next generation, a system built
  # elsewhere still has to be installable. That is
  # `nixos-rebuild switch --flake .#gw --target-host leigh-admin@10.3.1.254
  # --elevate=sudo`, and it needs daemon *trust*, not daemon *access*:
  # `allowed-users = *` already admits the client, but with `require-sigs` on (the
  # default) the daemon refuses paths built elsewhere and the copy dies with
  #
  #   cannot add path '…' because it lacks a signature by a trusted key
  #
  # which is exactly what it did, on the first deploy after install, before this
  # line existed. It is the same answer common/build-farm.nix gives on services1 and
  # services2, for the same reason.
  #
  # What it costs is nothing that sudo does not already cost: `leigh-admin` has
  # passwordless sudo on every machine in this network (common/users.nix), so a
  # client holding the hop key is already root here. Trusted-user rights — adding
  # unsigned paths, setting nix.conf at build time — are a weaker grant than the one
  # it already has. What it must never be confused with is `nixbld`: this is not
  # build-user membership, and common/nixbld.nix still refuses that.
  nix.settings.trusted-users = ["leigh-admin"];

  # The old box ran OPNsense's pkg-config stack; nothing here is a package the
  # other machines build, so the crate toolchain in the devshell is not needed.
  # The diagnostics worth having are the ones that answer a question about a
  # packet: `dig` for the records dnsmasq generates, `nft` for the rendered
  # ruleset (the rules are loaded from a generated file, and reading it back is
  # the only way to check what the firewall actually says — see ./README.md),
  # `tcpdump` for the VLAN tags on the wire, and `ip`/`ss` from the systemd set.
  # pppd and dnsmasq come in by their own modules; `bind` is here for `dig` alone.
  #
  # The last three are not diagnostics, they are the box's ability to deploy
  # itself, and they are here rather than in common/tools.nix because that file is
  # the one this machine must not import (its wait-for-network unit reads the NAS
  # host, so tools.nix and nas.nix come as a pair — see flake.nix's comment on the
  # gw system). A router that cannot build its own next generation can only be
  # pushed to, and the push path is the one that breaks when the router is the
  # thing that is broken:
  #
  #   * `git` — `--flake .` needs a git tree; without it this box cannot evaluate
  #     the flake it is running.
  #   * `just` — so `just switch` here means the same thing it means on the other
  #     three machines (it resolves `--flake .` against the hostname, which is
  #     `gw`, so it cannot build the wrong machine by forgetting an argument).
  #   * `jq` — `tailscale status --json` is the only way to prove this box is not
  #     advertising routes it has no business advertising (./coexist.nix), and
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

  # GOLDEN RULE: common/base.nix sets system.autoRollback.enable for all three
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
