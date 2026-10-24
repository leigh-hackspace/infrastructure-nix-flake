# services1.int.leighhack.org

The services box (10.3.1.20). What runs here is the module list, not this file:
`services/default.nix` imports one module per service under `services/`, and
`docs/services.md` describes what each of them does. This README keeps only what is not
in the flake.

## An origin, not a front door

Since the 2026-10-24 front-door migration (`docs/router-front-door-migration.md`), `gw`
terminates TLS, holds the certificate, runs `nginx-sso` and `anubis`, and answers the
`*.int` zone. This box is an **origin**: nginx still runs, but only for the app-shaped
vhosts in `common/front-door.nix` whose `upstream = "origin"`, on plain HTTP :80 behind
`CONFIG.ROUTER_ONLY`. `machines/services1/int-dns.nix` fails the evaluation if its vhosts
and that table's origin rows disagree. There is deliberately no `security.acme`, no
`nginx-sso` unit and no anubis here — adding one back creates a second front door.

## Tailscale (client)

The tailscale **client** is installed manually (not part of this flake):
`/etc/systemd/system/tailscaled.service` is a symlink into the nix store,
with a NixOS drop-in from the deployed system. The **server** (headscale +
headplane) moved to `machines/gw/headscale.nix`; this box no longer runs it.
The client is **inactive** here as of 2026-10-24 — the network's tailnet client is
`machines/gw/uplink.nix`, and nothing on this box depends on this one.

Grafton site-to-site (grafton LAN 192.168.49.0/24 reachable from the hackspace
via the `grafton-hackspace-client` VM on the grafton router) is **gw's** job now:
that VM advertises the subnet route, headscale approves it
(`headscale nodes approve-routes -i <node> -r 192.168.49.0/24`), and gw accepts
routes, so tailscaled installs it in table 52 and every LAN client reaches it
through the default gateway. This box used to be the acceptor, and its WireGuard
peer to the site (`wg0`, which listed `192.168.49.0/24` in `allowedIPs`) never
handshook (`0 B received`) — that claim is what black-holed the subnet for this
box, and it is gone from `networking.nix`. The command below is only needed if
this client is put back into service:

```bash
sudo tailscale up --accept-routes --accept-dns=false \
  --advertise-routes=10.3.0.0/16,2001:8b0:1d14::/48,fd99:dead:beef:225::/64 \
  --login-server=https://tailscale.leighhack.org
```

(Headscale side: approve the route with
`headscale nodes approve-routes -i <node> -r 192.168.49.0/24`.)

## Secrets

Secrets are sops-encrypted in the repo (`secrets/`) and rendered to
`/var/lib/secrets` at activation; the paths services read are listed in
`config.nix`. Never commit plaintext here — the old habit of dropping
`*.key` files in `/var/lib/secrets` by hand is what sops replaced.
