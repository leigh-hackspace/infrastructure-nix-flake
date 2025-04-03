# services1.int.leighhack.org

## Runnings Services

- NGINX Reverse Proxy (with Let's Encrypt)
- Frigate CCTV with Coral TPU
- Zigbee2MQTT
- Door Entry Management System
- PostgreSQL server
- WireGuard VPN (currently only Chris Dell has access, ask for more information)

## Frigate CCTV

https://frigate-int.int.leighhack.org:8971/

## Door Entry Management System

Authentication is management by Authentik.

- (Internal) https://doors.int.leighhack.org/
- (External) https://doors.leighhack.org/

### Updating

```bash
nix flake lock --update-input door-entry-management-system
nix flake lock --update-input door-entry-bluetooth-web-app
```

## Required Secrets

Not included in Git:

-   `.env`
-   `http_basic_auth`
-   `wg.key`
