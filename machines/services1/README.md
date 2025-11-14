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
