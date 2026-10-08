# Authentik — OIDC provider notes (moved out of AGENTS.md 2026-10-08)

The facts an agent needs day to day stay in `AGENTS.md` (admin access, the
deployed integrations, the client-side contract). This file holds the detail that
only matters when creating or debugging a provider — read it **before** minting a
new OIDC app, every part of it has bitten once.

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
