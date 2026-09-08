# services1.int.leighhack.org

## Runnings Services

- NGINX Reverse Proxy (with Let's Encrypt)
- Frigate CCTV with Coral TPU
- Zigbee2MQTT
- Door Entry Management System
- Tailscale/Headscale with Headplane admin panel
- Mattermost (Slack clone)
- Outline (Notes/Wiki)
- PostgreSQL server
- Redis servers
- MQTT server
- WireGuard VPN (currently only Chris Dell has access, ask for more information)

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


## Frigate CCTV

https://frigate.int.leighhack.org/

## Door Entry Management System

Authentication is management by Authentik.

https://doors.leighhack.org/

### Updating

```bash
nix flake update door-entry-management-system
```

## Required Secrets

Not included in Git:

    $ sudo ls -la /var/lib/secrets/
    -r--r-----  1 root secrets   41 Nov 10 15:41 headplane_api_key.key
    -r--r-----  1 root secrets  129 Nov 10 15:39 headplane_client_secret.key
    -r--r-----  1 root secrets   49 Nov 10 16:33 headplane_pre_authkey.key
    -r--r-----  1 root secrets   30 Apr 16  2025 http_basic_auth
    -r--r-----  1 root secrets  128 Oct 21 00:29 mattermost_authentik_secret.key
    -r--r-----  1 root secrets   61 Nov 14 20:56 nginx_sso_auth.key
    -r--r-----  1 root secrets  129 Nov 14 20:55 nginx_sso_client_secret.key
    -r--r-----  1 root secrets  129 Nov 13 16:02 outline_client_secret.key
    -r--r-----  1 root secrets 1021 Oct 20 12:32 synapse-authentik.yaml
    -r--r-----  1 root secrets   44 Apr  2  2025 wg.key
