# MODULE — the hackspace Space API, moved off `apps1` onto this box on 2026-11-16.
#
# What it is, measured rather than assumed: a small Python (uvicorn) service implementing the
# community **SpaceAPI** spec (`api: "0.13"`), published publicly as `api.leighhack.org` through
# the front door. It is a thin aggregation layer — Home Assistant for every sensor and for
# `state.open`, Prometheus for the rest, static config for the identity fields — and its whole
# mutable state is `/srv/hackspaceapi/sensors.yaml`, 62 lines mapping HA entity ids to named
# locations. It also serves `/events?calendar=public|members` (the calendar proxy
# `hackspace-mcp` uses for "what's on", which the Monday 09:00 digest shares) and the published
# `/events.ics`. `docs/home-assistant-on-services1-2026-11-16.md` §"…and that one client is the
# hackspace's public machine-readable presence" owns the receipts, including why deleting it is
# not an option: 7,939 external `/space.json` requests in 40 hours, self-identifying pollers
# (`spacer-fetch`, `MapsOfMaking-heartbeat`, `mapall.space`, `SpacePush`, `HackerspacesGlobe`,
# `HackerspaceNewsFeed`), plus 448 `/events.ics` subscriptions.
#
# WHY THE ENVIRONMENT FILE IS A RUNTIME PATH AND NOT A `sopsSecretText`
#
# `/var/lib/hackspace-api/env` holds `HOMEASSISTANT_TOKEN`, a long-lived Home Assistant token.
# It is therefore **not** allowed anywhere near the Nix store: `sopsSecretText` returns a
# world-readable store path present on every box that builds or substitutes, and AGENTS.md's
# "What build-time decryption costs, in one place" is the argument for why that is the wrong
# shape for a credential nothing needs at build time. So the file is copied once, by hand, from
# the box that had it — root:root `0600`, never through a terminal — and the container is handed
# `--env-file` at start. That is the same posture this repo already takes for the other services
# whose config is runtime state the flake only mounts (`/srv/frigate/config/config.yaml`,
# `/srv/zigbee2mqtt/configuration.yaml`), and it is why the activation script below creates the
# directories but never the file: an activation that could create it would be an activation that
# contains the secret.
#
# Deliberately outside `/srv/hackspaceapi`, which is the container's `/data`: the app's content
# root and its credential must not share a directory, whatever the app happens to serve today.
#
# WHY THE MISSING FILE MUST FAIL LOUDLY
#
# With an empty `HOMEASSISTANT_TOKEN` this service does not crash — it serves a `space.json` with
# the sensors quietly missing, which is the "looks alive, is wrong" class this repo keeps getting
# bitten by (a frozen lease table, a dead MQTT link that 404s over HTTP). So there is a
# `test -s` gate in front of the container: no credential, no start, and the reason in the
# journal, on the never-give-up retry loop every container here runs on.
#
# WHY BRIDGE + `-p` RATHER THAN THE HOST NETWORK HOME ASSISTANT USES
#
# The space API needs no multicast and no inbound by-address clients: it makes two outbound HTTP
# calls (HA and Prometheus) and answers HTTP. So it gets the house pattern. The consequence is
# that every request arrives from podman's bridge gateway, **`10.88.0.1`**, which is why that
# address is in `FORWARDED_ALLOW_IPS` in the env file — without it uvicorn's `--proxy-headers`
# ignores `X-Forwarded-For` and the service logs the gateway as every client, which is the same
# silent degradation in a different place. `10.3.1.1` (the door) and `10.3.1.20` were already
# trusted by the file as it arrived.
{
  config,
  lib,
  pkgs,
  ...
}: let
  SPACE_API_IMAGE = "ghcr.io/leigh-hackspace/hackspace-api:1.3.2";
  SPACE_API_STATE = "/srv/hackspaceapi";
  SPACE_API_ENV = "/var/lib/hackspace-api/env";
  SPACE_API_PORT = 8081;
in {
  virtualisation.oci-containers.containers.hackspace-api = {
    image = SPACE_API_IMAGE;
    autoStart = true;
    # Container listens on 80 internally (`uvicorn --port 80` on `apps1`); 8081 is the address
    # the front door, `hackspace-mcp` and `apps1`'s Prometheus scrape target all already use,
    # so the move changes who answers it and nothing else about it.
    ports = ["${toString SPACE_API_PORT}:80"];
    volumes = ["${SPACE_API_STATE}:/data"];
    extraOptions = ["--env-file=${SPACE_API_ENV}"];
  };

  # The never-give-up override in common/containers.nix applies to this unit by name, so a
  # missing credential produces a restart loop with a legible cause rather than a dead service.
  systemd.services.podman-hackspace-api.serviceConfig.ExecStartPre = lib.mkAfter [
    "+${pkgs.bash}/bin/bash -c 'test -s ${SPACE_API_ENV} || { echo \"${SPACE_API_ENV} missing or empty: the Space API would serve a space.json with no sensors rather than fail; refusing to start.\"; exit 1; }'"
  ];

  # Directories only — see the header for why the credential file itself is never created here.
  system.activationScripts.hackspace-api-dir = lib.stringAfter ["users"] ''
    mkdir -p ${SPACE_API_STATE}
    mkdir -p -m 0700 /var/lib/hackspace-api
  '';
}
