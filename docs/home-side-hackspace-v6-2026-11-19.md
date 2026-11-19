# TEMP — home-side changes that retire the ACL stopgap (2026-11-19)

Handover note for `~/nixos-config` (grafton-router). **Delete this file** when the boxes
below are done and the prove-it curl returns non-403; the durable version of the lesson is
the "What `*.int` answers with" section of `docs/router-and-dns.md`.

## Corrected 2026-11-19, same evening

The first version of this file said *Zen re-delegated*. **Wrong** — grafton-router cannot ask
Zen for a different prefix delegation, so `2a02:8010:6680::/48` is permanent on your side and
everything pinned to it (`DELEGATED_PREFIX`, `LAN_IPV6_PREFIX = …:49::`, the static v6 lease
entries, the two `pg_hba` rows) is still correct. That sweep item is withdrawn.

What is measured, and therefore still stands: between 14:26:55 and 19:57 on 2026-11-19, 794
requests to the hackspace door arrived from `2a02:8011:d000:672::/64` — **not** your PD — and
none from `2a02:8010:6680::/48` after 13:48:58. So the open question is yours to answer:
**which interface egresses that /64?** Candidates, in the order I would check them: a second
PD (VLAN10? the friend-router's `:8472` split of your /48 is *not* it — different /32), the
laptop not actually on the house network at the time, or an exit node. Until it is answered,
this side allows both /48s, so IPv6 from either works.

## The fault that is not in question

Every `*.int` name answers with **three** addresses — `10.3.1.1`, `2001:8b0:1d14:225::1`
(public AAISP), `fd99:dead:beef:225d::1` (ULA) — and the route from the house into this
network is **IPv4-only** (`hosts/grafton-router/networking/interface.nix`'s `100-lan` route
list: `Destination = 10.3.0.0/16, Gateway = 192.168.49.15`). So a dual-stack client whose
tunnel carries no IPv6 dials the AAAA **over whatever WAN it happens to have**, and the door
judges it as a WAN client. That is why 29 `*.int` vhosts answered 403 while `id.` and
`tailscale…/admin` answered 200/302 from the same tab, seconds apart, and why the door has
**never** seen a request from `100.64.0.19` — the address the tailnet path carries.

## Changes, in order

1. `hosts/grafton-router/networking/interface.nix` — add to the `100-lan` route list beside
   the `HACKSPACE_SUBNET` entry: `Destination = "2001:8b0:1d14::/48"` (and optionally
   `"fd99:dead:beef::/48"`), `Gateway = 192.168.49.15`, `GatewayOnLink = true`. The bridge VM
   already holds both: it runs `--accept-routes=true` and the hackspace's `gw-1` advertises
   `10.3.0.0/16`, `192.168.2.0/24` and `2001:8b0:1d14::/48`
   (`sudo headscale nodes routes list` on gw).
2. `hosts/grafton-router/networking/firewall.nix` — the "LAN to Hackspace" forward rule is
   `ip daddr`, so it matches no IPv6 at all; add the `ip6 daddr 2001:8b0:1d14::/48`
   counterpart, plus the masquerade counterpart if the v4 path uses one.
3. `services/hackspace-tunnel.nix` — its three layers (route installed, VM answers ping,
   `10.3.1.1:53`) were all green through the entire 403 streak. A fourth — `curl -6 -o /dev/null
   -w '%{http_code}' https://network-info.int.leighhack.org/healthz`, reporting `403` as "our
   source is not in their ACL" rather than "the tunnel is down" — is the one that would have
   named the fault while it was happening.

## Prove it, then say so here

```
curl -6 -sS -o /dev/null -w '%{http_code}\n' \
  --resolve switch.int.leighhack.org:443:2001:8b0:1d14:225::1 \
  https://switch.int.leighhack.org/
```

`403` = the packet is still leaving the house over a WAN. `200`/`302` = it arrives on
`tailscale0`, and this side can then **delete** the two `Chris Zen (IPv6)` rows in
`common/config.nix`'s `LOCAL_NETWORK` (and the `^2a02:801[01]:` rule in
`common/nginx-sso-config.nix`) instead of maintaining them — which is the point: those rows
also grant port 22 and any forwarded destination (`machines/gw/firewall.nix:460,509,601,627`,
`machines/gw/README.md`, `docs/follow-ups.md`).
