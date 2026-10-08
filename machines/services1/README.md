# services1.int.leighhack.org

The services box (10.3.1.20). What runs here is the module list, not this file:
`services/default.nix` imports one module per service under `services/`, and
`AGENTS.md` describes what each of them does. This README keeps only what is not
in the flake.

## Tailscale (client)

The tailscale **client** is installed manually (not part of this flake):
`/etc/systemd/system/tailscaled.service` is a symlink into the nix store,
with a NixOS drop-in from the deployed system. The **server** (headscale +
headplane) is in `services/headscale.nix`.

Grafton site-to-site (grafton LAN 192.168.49.0/24 reachable from the hackspace
via the `grafton-hackspace-client` VM on the grafton router) depends on this
client ACCEPTING the `192.168.49.0/24` subnet route advertised by that VM.
The acceptance is stored in the tailscaled state file
(`/var/lib/tailscale/tailscaled.state`) and survives reboots.

If the node is ever re-registered / state reset, re-run:

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
