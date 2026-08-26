# DigitalOcean DNS (`leighhack.org`)

How to read (and, if needed, write) DNS records for the `leighhack.org` zone in
DigitalOcean — the same credentials the ACME DNS-01 challenge uses.

## Where the token lives

The zone is hosted at DigitalOcean and the SSL certs are issued with the
`digitalocean` DNS provider (see `machines/services1/http.nix`):

```nix
security.acme.defaults.dnsProvider = "digitalocean";
security.acme.certs."leighhack.org".environmentFile = CONFIG.ENV_FILE;
```

`CONFIG.ENV_FILE` is `/var/lib/secrets/.env` (see `machines/services1/config.nix`),
which contains the token as:

```
DO_AUTH_TOKEN=dop_v1_...
```

**Security rules:**

- Never print the token or copy it off services1. Run every API call on the box
  via `sudo`, sourcing the token into a shell variable. The project rule is
  "reference the secret path, don't read the contents" — the token never needs
  to appear in any output.
- The token has read **and** write scope (verified 2026-08-26). Treat it as
  full access to the zone.

## Accessing services1

The dev machine needs the machine-hop key and, on first connect, the host key:

```bash
ssh -i ~/.ssh/agent-hop-key -o StrictHostKeyChecking=accept-new \
    leigh-admin@10.3.1.20    # passwordless sudo, never root
```

`leigh-admin` has passwordless sudo, so remote scripts run with `sudo -n sh -s`
reading a heredoc over stdin. All examples below follow this pattern — the
`$(...)`/`$token` substitutions happen **on services1**, not locally, and the
token is never echoed.

## Listing all records

`machines/services1/http.nix` already carries the one-liner form; the full
listing (paged, 95 records as of 2026-08-26) is:

```bash
ssh -i ~/.ssh/agent-hop-key leigh-admin@10.3.1.20 'sudo -n sh -s' <<'REMOTE'
token=$(sed -n 's/^DO_AUTH_TOKEN=//p' /var/lib/secrets/.env | tr -d '"' | tr -d "'")
base="https://api.digitalocean.com/v2/domains/leighhack.org/records"
page=1
while :; do
  resp=$(curl -s -H "Authorization: Bearer $token" "$base?per_page=200&page=$page")
  [ "$page" = 1 ] && echo "total: $(printf '%s' "$resp" | grep -oE '"total":[0-9]+' | head -n1 | cut -d: -f2)"
  printf '%s' "$resp" | sed 's/},{/}\n{/g' | grep -E '^\{"id"' \
    | sed -E 's/\{"id":([0-9]+),"type":"([A-Z0-9]+)","name":"([^"]*)","data":"([^"]*)".*/\1|\2|\3|\4/'
  next=$(printf '%s' "$resp" | grep -oE '"next":"[^"]*"' | head -n1)
  [ -z "$next" ] && break
  page=$((page+1))
done
REMOTE
```

Output format: `id|type|name|data`. Names are relative to the zone unless they
already contain `leighhack.org`.

**Gotcha — the `?name=` filter needs the full name.** Filtering by the relative
label finds nothing:

```bash
curl -s -H "Authorization: Bearer $token" "$base?name=gw.int"              # total: 0
curl -s -H "Authorization: Bearer $token" "$base?name=gw.int.leighhack.org" # total: 1
```

The `http.nix` comment (`?name=gw.int.leighhack.org`) is the working form.

**Gotcha — the SOA record hides from the paged loop above.** The first
"record" of page 1 shares a line with `{"domain_records":[` so the
`^\{"id"` grep drops it. There is always exactly one SOA (DO-managed, not
editable): `{"id":1688627212,"type":"SOA","name":"@","data":"1800",...}`.

## One-off lookups

Single record or name-filter lookup (no paging needed):

```bash
ssh -i ~/.ssh/agent-hop-key leigh-admin@10.3.1.20 'sudo -n sh -s' <<'REMOTE'
token=$(sed -n 's/^DO_AUTH_TOKEN=//p' /var/lib/secrets/.env | tr -d '"' | tr -d "'")
curl -s -H "Authorization: Bearer $token" \
  "https://api.digitalocean.com/v2/domains/leighhack.org/records?name=gw.int.leighhack.org"
REMOTE
```

## Create → confirm → delete (test cycle)

Verified end-to-end 2026-08-26 (TXT so a brief existence has no routing impact):

```bash
ssh -i ~/.ssh/agent-hop-key leigh-admin@10.3.1.20 'sudo -n sh -s' <<'REMOTE'
token=$(sed -n 's/^DO_AUTH_TOKEN=//p' /var/lib/secrets/.env | tr -d '"' | tr -d "'")
base="https://api.digitalocean.com/v2/domains/leighhack.org/records"
name="do-api-test"
data="do-api-test-$(date +%s)"

# 1. Create
code=$(curl -s -o /tmp/do_create.json -w '%{http_code}' -X POST \
  -H "Authorization: Bearer $token" -H "Content-Type: application/json" \
  -d "{\"type\":\"TXT\",\"name\":\"$name\",\"data\":\"$data\",\"ttl\":60}" "$base")
echo "create: HTTP $code"
id=$(grep -oE '"id":[0-9]+' /tmp/do_create.json | head -n1 | cut -d: -f2)
echo "record id=$id"

# 2. Confirm by id
code=$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $token" "$base/$id")
echo "get by id: HTTP $code (200 = exists)"

# 3. Delete
code=$(curl -s -o /dev/null -w '%{http_code}' -X DELETE -H "Authorization: Bearer $token" "$base/$id")
echo "delete: HTTP $code (204 = ok)"

# 4. Confirm gone
code=$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $token" "$base/$id")
echo "get by id after delete: HTTP $code (404 = gone)"
REMOTE
```

Expected results: `201` → `200` → `204` → `404`.

## Syncing with the nginx vhosts (`dns-sync`)

The `*.int.leighhack.org` CNAMEs are maintained by the `dns-sync` tool
(Rust, deployed to services1 via `machines/services1/dns-sync.nix`; run it
with `just dns-sync-check` / `just dns-sync-sync` / `just dns-sync-prune`
from the repo, or directly with `sudo dns-sync check|sync|prune` on
services1). It reads the expected names from
`/etc/dns-sync/expected-int-names` (generated from the nginx config) and
creates any missing CNAMEs (`<label>.int` → `nginx.int.leighhack.org.`,
ttl 60). `sync` is strictly additive — it never deletes or rewrites existing
records, since many `*.int.leighhack.org` names legitimately point
elsewhere. `prune` is the explicit exception: it deletes only CNAMEs whose
data is `nginx.int.leighhack.org` that were expected previously but are no
longer (rename leftovers, tracked via `/var/lib/dns-sync/last-expected`);
records that predate dns-sync are reported and left as-is. See ROUTER.md.

## Environment notes

- services1 has **no `python3` and no `jq`** — parse with `grep`/`sed`, or dump
  raw JSON. (curl is at `/run/current-system/sw/bin/curl`.)
- API docs: https://docs.digitalocean.com/reference/api/api-reference/#operation/domains_list_records
- Zone overview (as of 2026-08-26, 95 records): apex A/AAAA → GitHub Pages;
  `*.int` CNAMEs → `nginx.int` (`10.3.1.20`); public apps CNAME →
  `nginx.leighhack.org`; `yunohost` A `81.187.195.18` + `*.yunohost` wildcard.
- The test host key for `10.3.1.20` was added to the dev machine's
  `~/.ssh/known_hosts` on first connect.
