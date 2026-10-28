# DATA — plain attrset read by `import`; not a module, defines no options.
# Every name the **front door** (`gw`) answers, and what it does with it.
#
# This is the front door's half of `docs/router-front-door-migration.md`: the router
# terminates TLS, holds the certificate, runs the `nginx-sso` gate and `anubis`, decides
# which network may have which name, and routes by SNI. What it does **not** do is know
# anything about the apps. So a row says exactly two things — *where this name goes* and
# *what the door requires before it goes there* — and every fact about the application
# behind an origin stays on the box that runs it.
#
# ## The two kinds of row, and why both exist
#
#   * `upstream = "origin"` — the name is served by **that box's origin nginx**
#     (`common/nginx/origin-vhost.nix`), which keeps its `proxy_pass`, its extra locations,
#     its `.well-known` rewrites and its static roots, and holds no certificate, no gate
#     and no network ACL. Used for the apps whose web config is app-shaped rather than
#     one-line-shaped: `matrix` (three locations plus the client-discovery well-knowns),
#     `element` (a built SPA served from a store path), `ha` (an auth endpoint with an
#     injected `sub_filter` redirect), `ai` (per-location routing with a file root),
#     `mattermost`, `outline`, `doors`. Moving those here would mean transcribing each
#     app's nginx into a row — one more place two boxes can disagree about an app, which
#     is the bug this migration exists to remove.
#   * `upstream = "http://10.3.1.32:4200"` — the door proxies **directly** to the box that
#     runs the app. This is every name whose upstream is not the origin box, and it is a
#     row precisely because it *is* one line: an address, and the client-facing limits the
#     door has to apply because it is the box holding the client's connection.
#
#   * `upstream = "local"` — served by this box itself, with no upstream at all: the
#     `bar` canary page, and `login` (the gate, which is `127.0.0.1:8082` *here*).
#
# ## What the door owns, and why each field is here and not on the origin
#
# `bodySize`, `timeouts`, `proxyBuffering`, `websockets` are **client-facing limits**: a
# 2 GiB upload is refused by whichever box applies the limit first, and a WebSocket
# upgrade is lost by whichever box forgets `Connection: upgrade`. That box is the door, so
# the door has them. Several are also on the origin (the same value, applied again on the
# internal hop); that duplication is deliberate and cheap, and the alternative — an origin
# silently capping a limit it no longer owns — is the drift this file is meant to end.
#
# `acl` is the network half of "who may have this name", and it is a **vhost** decision,
# not the router's nftables decision: `"lan"` is `CONFIG.LOCAL_NETWORK`, `"router"` is
# reachable from the front door alone, `"none"` is the public internet. The last place
# `$remote_addr` still names the client is this box, which is why the ACL is *here*.
#
# `sso` and `anubis` are the two gates the network has one of each. Both talk to a
# loopback listener on this box, which is the security fix hidden inside the move: the
# gate's auth endpoint answers `*:8082` on services1 today and decides LAN-or-tailnet from
# an `X-Real-IP` field only our nginx sets honestly (finding 4 of the migration doc).
#
# Fields (all optional except `upstream`, unless the row serves a `page`):
#   upstream   "origin" | a proxy_pass URL (address, never a name — see below)
#   acl        "lan" (default for *.int) | "none" (default for public) | "router"
#   sso        front with the nginx-sso auth_request + the 401→login redirect
#   anubis     proxy through the anubis instance on this box, whose TARGET becomes this
#              row's `upstream` — which is why the row names the app and not the socket
#   websockets proxyWebsockets
#   bodySize   client_max_body_size ("2048M")
#   timeouts   proxy_{connect,send,read}_timeout + send_timeout ("3600s")
#   proxyBuffering        proxy_buffering on/off
#   proxyHeaderBuffers    bigger proxy_buffer_size for upstreams with fat headers
#   serverAliases  other names this one row answers (one server block, one ACL — the public
#                  door names are aliases of their `*.int` twins and inherit that row's acl)
#   lanToken   inject the status-dashboard LAN token (dashboard names only)
#   strataBearer   inject the Strata engine key, overwriting the client's Authorization
#   page       a page this box serves itself ("bar")
#   note       why the row is shaped like this, when it is not obvious
#
# **Every upstream is an address, never a name.** dnsmasq runs on this box: an nginx that
# resolves its own upstreams at startup is asking the service it is starting to start
# itself, and a front door that waits on its own DNS is a boot-order loop with no timeout.
# Read the addresses from `common/hosts.nix` rather than typing them.
{hosts}: let
  # filestore's upload cap, mirrored from machines/services1/services/filestore.nix's
  # `maxUploadBytes` (2 GiB). It has to be here because the door refuses the body first:
  # a 900 MiB upload that only the origin knew about would die at nginx's 1 MiB default.
  filestoreBodySize = "2048M";
in {
  # The origin box. One address, read once, so `upstream = "origin"` cannot mean two
  # things on two boxes.
  originHost = hosts.services1;

  # The cert this door terminates with, and the SAN set it has to carry. Deliberately
  # **not** the union of what services1's cert happened to include: `*.ai.int` cannot even
  # be resolved (this box is authoritative for `int.leighhack.org` and no two-label name is
  # in `common/int-dns.nix`) and `*.test` is served by nothing, and every extra SAN is
  # another `_acme-challenge` TXT to win at every renewal for 60 days.
  #
  # Three apex+wildcard pairs, therefore three TXT names, and each pair is **one** TXT
  # shared by two domains — the race in the migration doc's findings. `dnsResolver` names
  # the resolvers; `--dns.propagation.wait` is what stops us losing the race.
  #
  # **No `int.leighhack.org`, and that is not an oversight**: the first deploy listed it and
  # Let's Encrypt refused the order —
  #
  #   400 :: urn:ietf:params:acme:error:malformed :: Error creating new order :: Domain name
  #   "int.leighhack.org" is redundant with a wildcard domain in the same request.
  #
  # `*.leighhack.org` matches every single-label name under the zone, so the apex one level
  # down is *redundant* to the CA rather than belt-and-braces. Services1's cert looks like it
  # does the same thing and does not: `leighhack.org` is not matched by `*.leighhack.org` (a
  # wildcard never matches the label it sits under), so that pair is legal. Nothing serves
  # the bare `int.leighhack.org` over TLS and every `legacy` name in `common/int-dns.nix` is
  # a single label under it, so the wildcard covers them and the apex costs nothing.
  certDomains = [
    "leighhack.org"
    "*.leighhack.org"
    "*.ai.leighhack.org"
    "*.int.leighhack.org"
  ];

  names = {
    # ---------------------------------------------------------------------------
    # Served by this box itself.
    # ---------------------------------------------------------------------------

    # The DNS canary. It has no upstream on purpose: a 200 from it is proof of the record,
    # dnsmasq, this nginx, this certificate and this ACL and nothing else, and the page
    # says which box answered — the question an operator actually has while both doors
    # exist.
    "bar.int.leighhack.org" = {
      page = "bar";
    };

    # The router's own dashboard (machines/gw/network-status.nix). Reachable from the
    # router itself via `lo`, so the JSON API keeps its one-address firewall rule.
    "network-info.int.leighhack.org" = {
      upstream = "http://${hosts.router}:8091";
    };

    # The gate. `login` is a front-door name (it is in `common/int-dns.nix`'s `names`), so
    # once `*.int` answers here the gate has to be here too — a login flow served by a box
    # that no longer answers the name is the incoherence the front-door move exists to
    # remove. `127.0.0.1` is *this* box by definition: the `nginx-sso` unit runs here
    # (machines/gw/http.nix), so the OIDC callback is loopback again and the gate's listener
    # may finally be loopback. Public, because anyone who is not signed in must reach it.
    "login.int.leighhack.org" = {
      upstream = "http://127.0.0.1:8082";
      serverAliases = ["login.leighhack.org"];
      acl = "none";
      proxyHeaderBuffers = true; # the SSO cookie exceeds nginx's 4k header buffer
    };

    # The bot gate. The row names the **app** — the librespeed container, published on the
    # origin box's wildcard 8004 — and `anubis = true` makes nginx talk to the gate's socket
    # here instead, anubis's TARGET being set from this address. Moving the gate moves the
    # socket, and `users.users.nginx.extraGroups = [ anubis ]` moves with it or every
    # request through it is a 502 (services1's comment on that line names unix socket
    # permissions as the reason it exists).
    "speed.leighhack.org" = {
      upstream = "http://${hosts.services1}:8004";
      anubis = true;
      websockets = true;
      bodySize = "100M";
    };

    # ---------------------------------------------------------------------------
    # Direct rows: the door proxies to the box that runs the app.
    # ---------------------------------------------------------------------------

    # aibox (7 direct rows, counting strata2; the `ai.*` pair below are origin rows
    # whose vhost is on services1 and whose upstream is llama-server on *this* box).
    "alexandria.int.leighhack.org" = {
      upstream = "http://${hosts.aibox}:4200";
      websockets = true;
      bodySize = "1024M";
      timeouts = "3600s";
      proxyBuffering = false;
      note = "book uploads + audio generation that outlives a default read timeout";
    };
    "ai.int.leighhack.org" = {
      upstream = "origin";
      bodySize = "1024M";
      timeouts = "3600s";
      note = ''
        multi-location (an image-upload path and a `/srv/ai-resources` file root), so it is
        an origin row: the routing stays with the app (machines/services1/ai.nix)'';
    };
    "ai.leighhack.org" = {
      upstream = "origin";
      acl = "none";
      sso = true;
      bodySize = "1024M";
      timeouts = "3600s";
      note = ''
        gated, and easy to miss: `machines/services1/ai.nix` builds it with `lib.mkMerge` of
        an `mkSSOVirtualHost` over an `mkIntVhost`, so the gate is not visible at the vhost
        literal. The rendered-config comparison caught it (`GATE ai.leighhack.org
        services1=sso door=`) — deploying the row without `sso` would have put aibox's
        inference endpoint on the internet with no login, which is the one class of mistake
        this migration must not make.'';
    };
    "mcp.int.leighhack.org" = {
      upstream = "http://${hosts.aibox}:8000";
      websockets = true;
    };
    "whisper.int.leighhack.org" = {
      upstream = "http://${hosts.aibox}:8083";
      websockets = true;
      timeouts = "3600s";
      note = "WebSocket sessions sit idle while a model thinks";
    };
    "frigate-monitor.int.leighhack.org" = {
      upstream = "http://${hosts.aibox}:8090";
    };
    "aibox-status.int.leighhack.org" = {
      upstream = "http://${hosts.aibox}:8088";
      lanToken = true;
    };
    "aibox.status.leighhack.org" = {
      upstream = "http://${hosts.aibox}:8088";
      sso = true;
    };
    "immich.leighhack.org" = {
      upstream = "http://${hosts.aibox}:2283";
      websockets = true;
      bodySize = "50000M";
      timeouts = "600s";
      note = ''
        the row that used to be built with the *internal* vhost helper and `acl = ""`
        (finding: `immich.leighhack.org` is a public name wearing an internal vhost). The
        row says what it is: public, big uploads, WebSockets.'';
    };
    # The second Strata engine, on aibox's Radeon iGPU (machines/aibox/strata.nix):
    # the full 125B IQ2_XS, where `strata` below is services2's Coder IQ1_M on the
    # RTX 3060.  Two names because there are two engines with different models; the
    # engine is reached on :8080 with an address, not a name, like every row here
    # (dnsmasq runs on this box and nginx must not ask it to start itself).
    #
    # `strataBearer = true` because the engine has an `api_key` and the key is one
    # secret (`strata_api_key`, shared by both engines by necessity — the door
    # injects one header): the name is LAN-only, so a client on the LAN should not
    # have to hold the inference key to use it, and taking the injected Bearer away
    # would 401 every request rather than open anything.  The engine's own
    # `allowed_hosts` (machines/aibox/strata.nix) lists this name, because nginx
    # forwards the client's Host header upstream.
    "strata2.int.leighhack.org" = {
      upstream = "http://${hosts.aibox}:8080";
      bodySize = "1024M";
      timeouts = "3600s";
      proxyBuffering = false;
      strataBearer = true;
      note = ''
        first token can take minutes (the engine reads ~40 GB and fills its tiers), so
        the timeouts are the row and SSE must not be buffered; 1024M because a chat
        request carrying a base64 picture or a long transcript is not a 1 MiB body.'';
    };

    # The 10.3.1.30 VM (5).
    "access-api.int.leighhack.org" = {
      upstream = "http://10.3.1.30:8083";
      acl = "none";
      note = "public today via its DO record; the door keeps the ACL off it rather than widening a rule";
    };
    "api.int.leighhack.org" = {
      upstream = "http://10.3.1.30:8081";
      serverAliases = ["api.leighhack.org"];
      acl = "none";
    };
    "jenkins.int.leighhack.org" = {
      upstream = "http://10.3.1.30:8082";
    };
    "user-tweaker.int.leighhack.org" = {
      upstream = "http://10.3.1.30:8084";
      serverAliases = ["user-tweaker.leighhack.org"];
    };
    "ha.int.leighhack.org" = {
      upstream = "origin";
      websockets = true;
      note = ''
        two locations, one of them an /auth/authorize rewrite with an injected
        `sub_filter` meta-refresh for the authentik round-trip — app-shaped, so it is an
        origin row and stays in machines/services1/http.nix'';
    };

    # The other single-box appliances.
    "id.int.leighhack.org" = {
      upstream = "origin"; # the authentik stack on services1, since the 2026-10-28 cutover
      serverAliases = [
        "id.leighhack.org"
        "authentik.int.leighhack.org"
      ];
      websockets = true;
      proxyBuffering = false;
      acl = "none";
      note = ''
        An origin rather than a one-line proxy because the app is loopback-bound on the box
        that runs it (`127.0.0.1:9000` behind that box's nginx) and because both hops need
        the websocket upgrade and unbuffered responses to be set twice for either to mean
        anything.  `acl = "none"` is not an oversight: `id.leighhack.org` is a public name
        and the IdP has always answered the internet on it — that is how members off-site
        sign in.

        `authentik.int` stays in `common/int-dns.nix`'s `exempt`, and keeps answering with
        10.3.1.36, and that is *not* a second box any more: the address moved onto
        services1 as a second interface address when the appliance was retired, because the
        switch logins' `radius-server host 10.3.1.36` and the LDAP clients'
        `ldaps://authentik.int.leighhack.org` could not be reconfigured.  So the door
        answers HTTPS for the alias here, and the same name answers RADIUS/LDAPS directly
        at the address.  docs/authentik-container-migration.md.'';
    };
    "monster.int.leighhack.org" = {
      upstream = "https://10.3.1.11:8006"; # Proxmox
      websockets = true;
      acl = "none";
      note = "deliberately not LAN-ACL'd (it was `acl = \"\"` before the move); reachable from the tailnet ranges either way";
    };
    "truenas.int.leighhack.org" = {
      upstream = "https://10.3.1.6:443";
      websockets = true;
    };
    "firewall.int.leighhack.org" = {
      upstream = "http://${hosts.router}:60443";
      note = ''
        a **dead upstream, carried forward on purpose**. Nothing has listened on
        10.3.1.1:60443 since the retired appliance's admin UI was deliberately not
        reopened (the migration doc's first finding), so this vhost is a 502 today and is
        a 502 here. Moving it unchanged is the point: the migration changes no answer, and
        a name that stops resolving fails more confusingly than one that answers wrong.
        Delete it and its `common/int-dns.nix` row in the change that decides 60443 is
        never coming back — `docs/follow-ups.md`.'';
    };

    # services2 (2 + strata's public name).
    "services2-status.int.leighhack.org" = {
      upstream = "http://${hosts.services2}:8088";
      lanToken = true;
    };
    "strata.int.leighhack.org" = {
      upstream = "http://${hosts.services2}:8080";
      bodySize = "1024M";
      timeouts = "3600s";
      proxyBuffering = false;
      strataBearer = true;
      note = "the LAN name gets the engine key injected; the public one forwards the client's own Authorization";
    };
    "strata.leighhack.org" = {
      upstream = "http://${hosts.services2}:8080";
      bodySize = "1024M";
      timeouts = "3600s";
      proxyBuffering = false;
      acl = "none";
    };

    # A non-box: the yunohost appliance at 10.3.1.39, behind three names.
    "webhooks.leighhack.org" = {
      upstream = "https://10.3.1.39:443";
      timeouts = "300";
      acl = "none";
    };
    "web-test.leighhack.org" = {
      upstream = "https://10.3.1.39:443";
      acl = "none";
    };
    "retro.leighhack.org" = {
      upstream = "https://10.3.1.39:443";
      acl = "none";
      note = ''
        `forceSSL = false` on services1 (it was the one public vhost that answered plain
        HTTP). The door terminates, so it answers HTTPS and redirects the HTTP hop; that
        is a behaviour change, and it is the intended shape of a front door rather than an
        accident of this move.'';
    };

    # SSO-gated upstreams that are not on the origin box.
    "voron.leighhack.org" = {
      upstream = "http://10.3.2.50"; # the Voron LCD/streaming box on CS_VLAN
      sso = true;
      bodySize = "1024M";
      timeouts = "3600";
    };
    "frigate.leighhack.org" = {
      upstream = "http://${hosts.services1}:5000";
      sso = true;
      note = ''
        the *frigate* container publishes 5000 on services1's wildcard address, so this is
        a direct row to a port rather than an origin row through port 80 — and the reason
        the origin-reachability rule in `machines/gw/firewall.nix` has to name 5000 and not
        just 80/443.'';
    };
    "frigate.int.leighhack.org" = {
      upstream = "http://${hosts.services1}:5000";
      websockets = true;
    };

    # ---------------------------------------------------------------------------
    # Origin rows: services1 keeps the vhost, the door keeps the name.
    # ---------------------------------------------------------------------------

    "filestore.int.leighhack.org" = {
      upstream = "origin";
      bodySize = filestoreBodySize;
    };
    "gocardless.int.leighhack.org" = {
      upstream = "origin";
    };
    "gatus.int.leighhack.org" = {
      upstream = "origin";
    };
    "grafana.int.leighhack.org" = {
      upstream = "origin";
      websockets = true;
    };
    "prometheus.int.leighhack.org" = {
      upstream = "origin";
    };
    "services1-status.int.leighhack.org" = {
      upstream = "origin";
      note = "the LAN token header is injected by the origin, where the dashboard runs";
    };
    "switch.int.leighhack.org" = {
      upstream = "origin";
      note = ''
        the switch configurator. Its trust in `X-WEBAUTH-USER` used to rest on binding
        loopback — measured on services1, `127.0.0.1:8097`, so it still does: the only
        path to that socket is this box → origin → socket. Do not "helpfully" republish
        it on a routable address; see `crates/switch-gui/src/routes.rs`.'';
    };
    "unifi-admin.int.leighhack.org" = {
      upstream = "origin";
      websockets = true;
      timeouts = "3600s";
      note = "the UniFi OS appliance UI on 127.0.0.1:11443";
    };
    "zigbee2mqtt.int.leighhack.org" = {
      upstream = "origin";
      websockets = true;
    };
    "zigbee2mqtt.leighhack.org" = {
      upstream = "origin";
      sso = true;
    };
    # door-entry: one vhost pair today, each with its public name as an *alias*. Both are
    # **open on services1 today — no `LOCAL_NETWORK` on either block** (measured: 0 `allow`
    # lines), which is what lets the door hardware and members' phones dial it from outside
    # the LAN. `acl = "none"` is that fact, written down: the first draft of this table let
    # these two rows take the `*.int` default and would have locked the doors out, and the
    # only reason it is not in production is that the rendered configs were diffed by name
    # against services1's live `/etc/nginx/nginx.conf` before anything was deployed.
    "doors.int.leighhack.org" = {
      upstream = "origin";
      serverAliases = ["doors.leighhack.org"];
      acl = "none";
    };
    "api-doors.int.leighhack.org" = {
      upstream = "origin";
      serverAliases = ["api-doors.leighhack.org"];
      acl = "none";
    };
    "status.leighhack.org" = {
      upstream = "origin";
      sso = true;
    };
    "retro.int.leighhack.org" = {
      upstream = "origin";
      note = ''
        `root = /sas-16tb/ds-downloads/Retro` — a directory that exists because services1
        mounts the NAS, and the router deliberately mounts nothing. This is the row that
        makes the filesystem root stay where the mount is: the door only needs the name
        (the reason the origin design keeps the filesystem root where the mount is). Was
        plain HTTP; the door answers HTTPS.'';
    };
    "robot.ai.leighhack.org" = {
      upstream = "origin";
      acl = "none";
      note = "a static file root on services1 (/srv/ai-resources/resources/robot)";
    };
    "matrix.leighhack.org" = {
      upstream = "origin";
      websockets = true;
      # No `bodySize`/`timeouts` here, and the three rows below likewise: services1 sets none
      # on these four, and the first draft of this table invented 1024M/100M for them because
      # they are "obviously upload-heavy apps". The rendered comparison flagged it as drift.
      # A door that is more generous than its origin changes nothing (the origin still caps
      # the hop) and teaches the next reader nothing true, so the table says what services1
      # says and stops.
      note = "three synapse locations plus the client-discovery well-knowns: app-shaped, so an origin row";
    };
    "element.leighhack.org" = {
      upstream = "origin";
      note = "a built SPA (pkgs.element-web) served from a store path on the origin box";
    };
    "mattermost.leighhack.org" = {
      upstream = "origin";
      websockets = true;
    };
    "outline.leighhack.org" = {
      upstream = "origin";
    };
    "headplane.leighhack.org" = {
      upstream = "http://127.0.0.1:8086";
      websockets = true;
      note = "the headscale admin UI, kept as its own name; the daemons moved here with the front door";
    };
    "tailscale.leighhack.org" = {
      upstream = "http://127.0.0.1:8085";
      websockets = true;
      # headscale's own console. Two paths, which is why the row carries `locations`: the
      # root is headscale's 404 page and redirects to the admin UI, and `/admin` is
      # headplane on 8086. Everything else (the control protocol's `/key`, `/register`,
      # `/machine/...`) stays on 8085 — pointing the whole name at headplane would leave
      # every tailnet node unable to register.
      locations = {
        # `= /` rather than `/`: an exact match redirects the root, while every other
        # path (including `/key` and `/machine/...`) keeps the default proxy to 8085.
        "= /" = {
          extraConfig = ''
            rewrite ^/$ https://tailscale.leighhack.org/admin permanent;
          '';
        };
        "/admin" = {
          proxyPass = "http://127.0.0.1:8086";
          recommendedProxySettings = true;
          proxyWebsockets = true;
        };
      };
      note = "headscale's control server + `/admin` (headplane) — the name its OIDC issuer URLs point at; the daemons moved here with the front door";
    };
  };
}
