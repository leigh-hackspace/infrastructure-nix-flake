# gw-specific system configuration.  The settings shared with the other
# machines (timezone, i18n, keyboard, sudo, nix-ld, ssh, bootloader,
# autoRollback, experimental-features) live in common/base.nix.
#
# THIS MACHINE HAS NO HARDWARE YET.  Read ./README.md before touching anything
# here: it says what this replicates from the old OPNsense box, what it cannot,
# and which values have to come off the real box before this can deploy.  The
# config is written to be *evaluated* (and is checked by `just check`), not to
# be deployed.
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

  # The old box ran OPNsense's pkg-config stack; nothing here is a package the
  # other machines build, so the crate toolchain in the devshell is not needed.
  # The diagnostics worth having are the ones that answer a question about a
  # packet: `dig` for the records dnsmasq generates, `nft` for the rendered
  # ruleset (the rules are loaded from a generated file, and reading it back is
  # the only way to check what the firewall actually says — see ./README.md),
  # `tcpdump` for the VLAN tags on the wire, and `ip`/`ss` from the systemd set.
  # pppd and dnsmasq come in by their own modules; `bind` is here for `dig` alone.
  environment.systemPackages = with pkgs; [
    bind
    nftables
    tcpdump
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

  # This value determines the NixOS release from which the default settings for
  # stateful data (file locations, database versions) on this system were taken.
  # It is a placeholder until the box exists: set it to the release version of
  # the *first install* of this system when the hardware arrives, and do not
  # copy one of the other machines' values across.
  system.stateVersion = "25.05";
}
