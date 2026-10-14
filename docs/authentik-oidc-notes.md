# Authentik — OIDC provider notes (moved out of AGENTS.md 2026-10-08)

The rules an agent needs day to day are in `AGENTS.md` (the Authentik section: admin
access, the PKCE contract, "read this **before** minting a new OIDC app"). This file
holds the detail that only matters when creating or debugging a provider, and the
deployed-integrations table (provider pks, client ids, where each secret lives) — read
it **before** minting a new OIDC app, every part of it has bitten once.

## Minting an admin API token

The default `akadmin` user is disabled; real admins are in the superuser groups
`Infra` (or `pgina`), e.g. `cjdell`.

```bash
docker exec -i authentik-server-1 ak shell <<'PY'
from authentik.core.models import Token, User
t = Token(identifier="setup", user=User.objects.get(username="cjdell"),
          intent="api", expiring=False)
t.save(); print(t.key)   # shown once — delete the token when done
PY
```

The admin REST API is **not** usable from the box (the box's nginx proxies
`/api/v3/...` collection endpoints to the UI SPA — they 404 with HTML), so manage
everything through the ORM shell above. `ak shell` prints a banner and swallows
tracebacks unless you keep stderr and filter the log noise:

```bash
docker exec -i authentik-server-1 ak shell 2>&1 <<'PY' | grep -viE "imported related module" | grep -vE '"level": "(debug|info)"'
```

## Creating an OIDC provider that actually works

All three of these bites are invisible until login is tried.

1. Copy `authentication_flow` / `authorization_flow` / `invalidation_flow` /
   `signing_key` from a known-good provider (Grafana, pk 3). Redirect URIs match
   **strictly**.
2. Duration fields (`access_code_validity`, `access_token_validity`,
   `refresh_token_validity`) must be the `key=value` format, e.g. `"minutes=5"`
   (what `authentik.lib.utils.time.timedelta_from_string` parses). ISO-8601
   values like `"5m"` save fine but crash `/application/o/authorize/` with a 500
   (`ValueError` in `timedelta_from_string`).
3. Custom scopes need a **ScopeMapping child row** (multi-table inheritance). A
   plain `PropertyMapping` attached to the provider silently does not advertise
   its scope — the token is issued without it (log line: "Application requested
   scopes not configured, setting to overlap"). Create it with:

   ```python
   from authentik.core.models import PropertyMapping
   from authentik.providers.oauth2.models import ScopeMapping
   pm = PropertyMapping.objects.create(name="... groups scope",
                                       expression='''return {
       "groups": [g.name for g in request.user.ak_groups.all()],
   }''')
   sm = ScopeMapping(pk=pm.pk, scope_name="groups")  # child reuses parent pk!
   sm.save()
   # then include pm in provider.property_mappings.set([...])
   ```

   `get_or_create(propertymapping_ptr=...)` does NOT work (it tries to insert a
   second parent row → IntegrityError). The stock OpenID mappings
   (openid/email/profile) are regular scope mappings already present.

There is no `code_challenge_methods` field on this version's `OAuth2Provider`
(AttributeError) — S256 PKCE is just available.

## Client-side OIDC contract

What `gocardless-dashboard` (and `filestore`) implement; keep new clients
consistent: authorization-code flow with **S256 PKCE**, where the challenge must
be **unpadded** base64url — authentik recomputes
`urlsafe_b64encode(sha256(verifier)).replace("=","")` and compares equality, so a
padded challenge fails at token time with `invalid_grant` ("Code challenge not
matching"). Scopes `openid profile email groups`; group claims come from the
groups scope mapping, not a user profile attribute.

## Debugging

`docker logs authentik-server-1` is JSON; `system_exception` events carry full
tracebacks, `authentik.asgi` lines carry request + status. OIDC endpoints:
`/application/o/authorize/` (browser GET), `/application/o/token/` (client POST).
A POST to `/application/o/authorize/` with an API token 403s on CSRF — expected,
not a bug.

## Database

`docker exec authentik-postgresql-1 psql -U authentik -d authentik`

## ORM names on 2026.8.1 (several things renamed since the snippets above)

`ak shell` on 2026.8.1 differs from the older snippets in this file. Hit while
creating the `switch-gui` provider (pk 35) on 2026-10-12:

- `OAuthProvider` → **`OAuth2Provider`** (still in
  `authentik.providers.oauth2.models`; `Provider.__subclasses__()` is the quick
  way to find a renamed model).
- `redirect_urls` → **`redirect_uris`**, and it is a list of `RedirectURI`
  dataclasses, not strings:
  `RedirectURI(url=…, matching_mode=RedirectURIMatchingMode.STRICT,
  redirect_uri_type=RedirectURIType.AUTHORIZATION)`.
- `Event.type` → **`Event.action`** (filter with `action__in=["login",
  "login_failed"]`; `type` raises `FieldError` listing the real fields).
- `Flow.binding_set` → **`Flow.bindings`**.
- `Application` has **no `launch_url`** — it is `meta_launch_url` (plus
  `meta_description`, `meta_icon`, …).
- `User.active` → `User.is_active`; `RadiusProvider` has no `slug`.
- Duration fields still must be `key=value` (`minutes=5`), and copying them from
  a working provider is the safe move.

**`client_secret` is readable through the ORM** even though the UI treats it as
write-only. So never dump a whole provider (`values()` / `print(obj)`) into a
terminal or a log you will keep — read the specific fields you need. A provider
dump is a secret leak; rotate anything you have exposed.

**A fresh `groups` ScopeMapping cannot be made the way this file used to say.**
`PropertyMapping.objects.create(name=…)` followed by `ScopeMapping(pk=pm.pk,
scope_name="groups").save()` dies on
`duplicate key value violates unique constraint "authentik_core_propertymapping_name_…_uniq"`
with `Key (name)=() already exists` — the child write lands an empty name, and
empty-named rows already exist. Reusing an existing `groups` ScopeMapping row is
fine (providers already share the OpenID default mappings), so that is what
`switch-gui` does. Creating one properly means the web UI or the real API.

## Debugging RADIUS (the switch/AP logins, not the web UI)

The outpost is its own container: `docker logs ak-outpost-radius-outpost`. It
logs every packet at **info**, which is the fastest way to tell "the NAS/switch
never reached us" from "we refused it":

```
{"code":"Access-Request","event":"Radius Request","ip":"10.3.1.2", …}
```

The outcome is not in that line — look for the matching `login_failed` event
(`Event.objects.filter(action="login_failed", client_ip="10.3.1.2")`); its
`context` names the stage that rejected, and `model_name:
"identificationstage"` means plain bad credentials. Note that a RADIUS login
never produces the interactive flow's `rad_*` context keys, so grepping events
for `rad` finds nothing.

`RadiusProvider` (pk 7, application `Switches`) has `authentication_flow = None`
yet the flow used is `radius-authentication-flow` — resolved from the outpost,
not the provider, so do not read that `None` as "no flow is running".

## Deployed integrations

Provider pks, where each secret lives, and what it gates.  Client ids are
identifiers, not secrets; the secrets themselves are never in this repo — the sops
key names are.

| App | Provider pk | Client id | Secret location | Gate |
| --- | --- | --- | --- | --- |
| Grafana (services1) | 3 | `8TMM2mYHBV2YQCovNbgGKbuEp0LJcxo2TjpqNN9s` | sops: `grafana_oidc_client_secret` (runtime secret on services1) | role mapping: `Infra` → GrafanaAdmin, else Viewer |
| gocardless-dashboard | 33 (name "gocardless-dashboard", slug `gocardless-dashboard`) | `UonE3N21HsJH5ia12zxUSqo8wzoG19Hk6LQbOfBw` | shared env-file secret: `GOCARDLESS_DASHBOARD_OIDC_CLIENT_SECRET` | `Infra` group, from the `groups` scope claim |
| filestore | 34 (slug `filestore`) | in the shared env-file secret as `FILESTORE_OIDC_CLIENT_ID` | shared env-file secret: `FILESTORE_OIDC_CLIENT_SECRET` | `Infra` |
| switch-gui | 35 | in `switch_gui_env_file` as `SWITCH_GUI_OIDC_CLIENT_ID` | `switch_gui_env_file`: `SWITCH_GUI_OIDC_CLIENT_SECRET` | `Infra` |
| Switches (RADIUS) | `RadiusProvider` pk 7 | — | — | see "Debugging RADIUS" above |

The three client-side apps all implement the same contract (above) through
`crates/common-rs/oidc`, so a bug in one is a bug in three — and a new one should
reuse that crate rather than re-implementing the flow.

`switch-gui` (pk 35) was copied from gocardless-dashboard's provider on 2026-10-12:
strict redirect `https://switch.int.leighhack.org/auth/callback`, the three OpenID
mappings plus a `groups` scope, and the same flows and `signing_key`.  Its `groups`
scope **reuses gocardless-dashboard's ScopeMapping row** rather than owning one —
creating a fresh child row through the ORM trips the unique-violation described
above.
