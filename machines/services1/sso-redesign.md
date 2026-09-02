# SSO redesign: `mkSSOVirtualHost` → auth-gate that stops fighting browsers

- **Date:** 2026-09-02
- **Status:** proposal / investigation write-up — nothing implemented yet
- **Scope:** `machines/services1/` nginx + Authentik SSO
- **Author:** investigation session (see also repo AGENTS.md "Infrastructure facts")

## 1. Problem

`mkSSOVirtualHost` (`lib/nginx-sso-helper.nix`) makes an app reachable only after
an Authentik sign-in via the `nginx-sso` daemon. It works, but only by piling
workarounds on top of the apps it protects:

- every SSO vhost serves a **fake no-op service worker** that overrides the
  app's real one;
- PWA manifests have to be hand-exempted per app;
- unauthenticated XHR/asset requests get bounced to the login page (or to `/`)
  instead of receiving a clean 401;
- identity only exists as an injected `X-WEBAUTH-USER` header.

The ask: *apply Authentik SSO to any web app without lots of workarounds* —
in particular without breaking apps that ship real service workers / PWAs.

## 2. How it works today

`nginx-sso` (L7-Media, v0.27.8) runs as a daemon on services1 at
`127.0.0.1:8082` (`systemd.services.nginx-sso`, `http.nix`). It is an OIDC
client of Authentik (`providers.oidc.issuer_url` → `.../application/o/nginx-login/`)
and owns a session cookie on the whole `.leighhack.org` domain. It exposes:

- `/auth` — the auth_request upstream. Returns 200 (valid session, or ACL says
  anonymous is OK), 401 (must log in) or 403; on success it may return a
  renewed `Set-Cookie` and an `x-username` header.
- `/login` (and friends) — the login web UI served at `login.leighhack.org`.

`mkSSOVirtualHost` per vhost:

- puts `auth_request /sso-auth;` on `location /` (plus cookie renewal,
  `X-WEBAUTH-USER` injection, no-cache headers);
- defines internal `location /sso-auth` → `127.0.0.1:8082/auth`;
- turns 401s into `302 https://login.leighhack.org/login?go=…` via
  `error_page 401 = @error401` (with a regex that instead sends "asset-looking"
  URIs to `/`);
- serves a **fake service worker** at `~ /sw\.js$` so SW registration/update
  doesn't 401 through the gate.

The ACL in `lib/nginx-sso-config.nix` already lets anonymous traffic in from the
Hackspace LAN, Tailscale and a couple of home ranges. So in practice the gate
only bites **off-LAN public** traffic.

SSO vhosts today (6): `ai` (llama.cpp chat webui, aibox), `voron`
(10.3.2.50), `frigate`, `zigbee2mqtt`, `status` + `aibox.status` (house Rust
dashboards). Apps with native Authentik OIDC (mattermost, outline, grafana,
Home Assistant, door-entry) deliberately do **not** use this helper.

### Workaround inventory

| # | Workaround | Location | Cost |
|---|---|---|---|
| 1 | Fake `sw.js` no-op (`skipWaiting`+`claim`) served for every SSO vhost | `lib/nginx-sso-helper.nix` `locations."~ /sw\.js$"` (added 2026-07-02, commit `5847aba`; widened from `= /sw.js` because the real path differed) | Every SSO app loses its real service worker. The blanket regex also blocks an app from re-adding its own SW via a merged location. |
| 2 | Unauthenticated `/manifest.webmanifest` pass-through, hand-added per app | `ai.nix` (after CORS debugging, 2026-08-22) | Repeated per-app boilerplate that must be remembered for every future PWA. |
| 3 | "Asset-looking URI → redirect to `/` instead of login" | helper `@error401` | Fragile guess: misses hashed bundles, `/api/*`; XHR/fetch receives login-page HTML instead of a JSON/401-able response. |
| 4 | 32k `proxy_buffer_*` bumps for large session cookie | helper `/sso-auth` + `http.nix` `login.*` vhost | Symptom of a very large session cookie from nginx-sso (the "upstream sent too big header" 502, fixed 2026-08-22). |
| 5 | Identity as an injected header | helper injects `X-WEBAUTH-USER`; read by `status-dashboard/src/main.rs` | Only works for apps written to read it. Frigate has its own user DB (double login); Z2M / llama webui / Mainsail have no accounts at all, so for them the gateway is a membership doorman, not SSO. |

## 3. Root causes

The model is **"gate absolutely everything; redirect on failure"**. Browsers,
however, deliberately fetch three classes of resource without a usable session
context:

1. **Service-worker scripts.** Registration — and especially periodic *update
   checks*, which happen outside any page/user gesture — fetch the script
   without the session cookie. A 401/redirect there fails the install/update
   with CORS errors. You cannot "log the service worker in".
2. **PWA manifests** (and icons). Fetched credential-less.
3. **Non-navigation fetches** (XHR/`fetch()` from SPA JS). They follow the 302
   to the login page and receive HTML where they expected JSON/their own error.

None of these need protecting: they are static code/config, not user data. The
gateway treats them like everything else, so apps that use them need
workarounds.

**Key insight:** the SW/manifest problem is *not* an nginx-sso quirk. Every
auth-request-style proxy (Authentik proxy outpost forward-auth, oauth2-proxy,
Authelia, …) has the same constraint, and its integration docs all end up
telling you to exclude `sw.js` from auth. Any "for any web app" answer has to
either (a) stop gating bootstrapping/static resources, or (b) push auth into
the app so the app can serve static files publicly by design.

## 4. Design space

### Option A — Native OIDC in the app (Authentik as IdP)

The house style already for apps that support it: mattermost, outline, grafana,
Home Assistant, door-entry each have their own Authentik provider and handle
the authorization-code flow themselves. No nginx gate at all.

- Pros: real per-user identity, per-app sessions, real logout; SWs/PWAs work
  because the app serves static assets publicly and only protects data/API;
  group/scope mapping via Authentik property mappings; works on any host, not
  just through services1 nginx.
- Cons: requires app support. Not available for Frigate / Z2M / Mainsail /
  llama webui (no OIDC or no account system).
- **Does it fix SWs?** Yes — the app's own model is already correct.
- Notable candidate: **Immich supports OIDC natively** and is currently
  exposed publicly with no protection at all (`immich.leighhack.org` in
  `http.nix`). Adding an Authentik provider fixes both the exposure and any
  future SW/PWA friction.

### Option B — Redesigned gateway: gate what matters, leave bootstrap assets public

Keep the doorman for apps that can't do OIDC, but change what it gates:

1. **Drop the fake service worker.** Serve the app's real SW/manifest/icon
   files unauthenticated (declarative default + per-app `publicLocations`).
2. **Only top-level navigations bounce to the login page.** Requests with
   `Sec-Fetch-Mode` ≠ `navigate` get a clean `401` so SPA JS can react.
3. Delete the "asset → `/`" regex hack and the per-app manifest boilerplate.

- Pros: one declarative vhost that works for any app; kills workarounds 1–3;
  keeps `X-WEBAUTH-USER` for the dashboards; small, localised diff.
- Cons: shared-domain session, header-only identity, and the extra daemon
  remain.
- **Does it fix SWs?** Yes for the gated apps — the SW file is public, and SW
  network requests still cross the gate with the session cookie.

### Option C — Swap nginx-sso for an Authentik proxy outpost (forward-auth)

Create Authentik proxy providers/outpost; nginx keeps `auth_request`, now
against the outpost.

- Pros: consolidation — deletes the nginx-sso unit, its YAML, the
  `nginx-login` OIDC client, two sops secrets and the `login.*` vhost;
  per-app access policies + SSO logout managed in Authentik.
- Cons: **identical 401 gate** → Option B's location exemptions are still
  required. It is a replacement, not a fix. Decide where the outpost runs:
  10.3.1.36 means every request subrequests across the network (SPOF, latency);
  services1 (podman container) keeps it local.
- **Does it fix SWs?** Only combined with Option B.

### Option D — Topology: tailnet/LAN-only for the no-account apps

Most SSO apps already have `.int` twins behind `CONFIG.LOCAL_NETWORK`
(tailscale + LAN + home ranges), and the ACL already trusts the tailnet. The
public SSO copy exists only for remote users who are not on the tailnet.

- If remote access goes over Tailscale (Headscale is already run), llama webui,
  Z2M, Mainsail, Frigate can be `.int`-only: no gateway, no SW problems, no
  doorman. Real SSO (native OIDC) is then reserved for apps with real accounts.
- Pros: least moving parts; PWA/SW behaviour is unconstrained.
- Cons: forces remote users onto the tailnet; public-host names stop working
  for non-tailnet users.

## 5. Recommendation

Layered, in order:

1. **Native OIDC wherever the app supports it.** Do Immich first: it fixes an
   open public exposure and is SW/PWA-clean by construction.
2. **Redesign the helper per Option B** for the residual no-account apps
   (llama webui, Z2M, Mainsail, Frigate). This is the change that removes the
   workarounds: the SW/manifest stop being fought over and pass through as
   ordinary static files, and the login redirect stops firing on API calls.
3. **Keep the gateway + `X-WEBAUTH-USER` for the status dashboards.** That is
   the right shape there (house Rust code, stdlib-only, genuinely needs the
   username) — it just inherits Option B's cleaner 401 semantics on `/api/*`.

Option C is worth doing later if Authentik-managed policies/logout for proxied
apps are wanted; Option D is a good simplification for apps nobody needs off
the tailnet.

## 6. Sketch: redesigned helper

```nix
# machines/services1/lib/nginx-sso-helper.nix (direction of travel)
{
  proxyPass,
  # Extra locations served WITHOUT auth (app SW/manifest/static). Regex
  # locations allowed. Defaults cover the standard PWA bootstrap files.
  publicLocations ? { },
}:
let
  # Files browsers fetch without (or independent of) a session:
  # service-worker scripts (update checks are credential-less), PWA manifests,
  # favicon. Gating these is what forced the fake-SW workaround.
  defaultPublic = {
    "= /sw.js"                = { };
    "= /manifest.webmanifest" = { };
    "= /site.webmanifest"    = { };
    "= /favicon.ico"         = { };
  };
  mkPublic = name: _: {
    inherit proxyPass;
    recommendedProxySettings = true;
    # deliberately NO auth_request here
  };
in {
  useACMEHost = "leighhack.org";
  forceSSL = true;

  extraConfig = ''
    error_page 401 = @sso-login;
    # ...timeouts / client_max_body_size as today...
  '';

  locations = lib.mkMerge [
    {
      # Everything else goes through the auth check
      "/" = {
        inherit proxyPass;
        recommendedProxySettings = true;
        proxyWebsockets = true;
        extraConfig = ''
          auth_request /sso-auth;
          auth_request_set $cookie $upstream_http_set_cookie;
          add_header Set-Cookie $cookie;
          auth_request_set $username $upstream_http_x_username;
          proxy_set_header X-WEBAUTH-USER $username;
          add_header Cache-Control "no-cache, no-store, must-revalidate";
          add_header Pragma "no-cache";
          add_header Expires "0";
        '';
      };
      "/sso-auth" = { /* internal auth subrequest, unchanged */ };
    }
    (lib.mapAttrs mkPublic (defaultPublic // publicLocations))
  ];

  # Navigations go to the login page; everything else gets a clean 401.
  locations."@sso-login" = {
    extraConfig = ''
      set $redirect_uri $scheme://$http_host$request_uri;
      if ($http_sec_fetch_mode != "navigate") { return 401; }
      return 302 https://login.leighhack.org/login?go=$redirect_uri;
    '';
  };
}
```

## 7. Things to verify when prototyping

- Exact nginx behaviour of `return 401` inside the `error_page`-named location
  (avoid re-entry/looping of `error_page`). Test on a scratch vhost first.
- Whether `Sec-Fetch-Mode` absent (curl, older browsers) should be treated as a
  navigation (probably yes, to preserve today's curl/CLI behaviour).
- The real SW/manifest path of each gated app so the default public list stays
  tight (llama webui's actual paths are the ones currently hidden by the fake).
- Re-check rendered units after `nixos-rebuild build` (per AGENTS.md gotchas)
  before deploying.

## 8. Suggested rollout

1. Prove Option B on `ai.leighhack.org` (re-enable the llama webui's real SW +
   manifest; confirm SW registers over the SSO vhost from a logged-in session
   and after session expiry).
2. Land the helper change for the remaining `mkSSOVirtualHost` vhosts.
3. Add Authentik OIDC for Immich (separate change; also removes its current
   unauthenticated public exposure).
4. Decide per app: native OIDC, redesigned gateway, or `.int`-only.
5. (Optional, later) Authentik proxy outpost migration per Option C.
