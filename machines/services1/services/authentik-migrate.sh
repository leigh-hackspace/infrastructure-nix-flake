# shellcheck disable=SC2034
#
# authentik-migrate — copy the authentik appliance (10.3.1.36, docker compose in
# /srv/authentik) into the podman stack declared by
# machines/services1/services/authentik.nix on this box.
#
# The appliance is only ever read: nothing here stops, restarts, writes or
# reconfigures anything on 10.3.1.36, so the live IdP keeps serving throughout.
# The one write is `pg_dump`, which takes a consistent snapshot and does not
# block readers.
#
# Two rules this script exists to enforce:
#
#   1. Never print a secret.  The appliance's .env holds AUTHENTIK_SECRET_KEY
#      and the database password; they are compared by hash and written to 0600
#      files, never to stdout.  There is no `set -x` and no `--verbose`.
#   2. Never restore into a stack running a different authentik version.  A
#      version-skew restore is applied on first boot and cannot be undone, so
#      `check` compares the image IDs on both sides and refuses if they differ.
#
# See docs/authentik-container-migration.md for the procedure this automates
# and for what is left over afterwards (outposts, DNS, the cutover).

set -euo pipefail

# --- configuration (every value is overridable from the environment) --------

SRC_SSH="${SRC_SSH:-root@10.3.1.36}"
SRC_SSH_KEY="${SRC_SSH_KEY:-/home/leigh-admin/.ssh/agent-hop-key}"
SRC_DIR="${SRC_DIR:-/srv/authentik}"
SRC_PG="${SRC_PG:-authentik-postgresql-1}"
SRC_SERVER="${SRC_SERVER:-authentik-server-1}"

DST_PG="${DST_PG:-authentik-postgres}"
DST_DIR="${DST_DIR:-/srv/authentik}"
WORK="${WORK:-/srv/authentik/migration}"
ENV_FILE="${ENV_FILE:-/run/secrets/authentik_env_file}"

SERVER_UNIT="${SERVER_UNIT:-podman-authentik-server.service}"
# The units that have to be down while the database underneath them is
# replaced.  Whichever of them were running are started again afterwards.
read -r -a APP_UNITS <<<"${APP_UNITS:-$SERVER_UNIT podman-authentik-worker.service}"

# The uid the server image runs as (see the comment in the module).
AK_UID="${AK_UID:-1000}"

# How many dumps to keep in $WORK.
KEEP_DUMPS="${KEEP_DUMPS:-3}"

DUMP=""
OUTPOST_OUT=""
FORCE=0

# --- helpers ----------------------------------------------------------------

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
die() {
  printf '\033[1;31merror:\033[0m %s\n' "$*" >&2
  exit 1
}

# sha256 of a secret, truncated: enough to compare two, no use to anyone who
# reads it.  Secrets are compared by hash and never printed.
hash_of() { printf '%s' "$1" | sha256sum | cut -c1-16; }

# A comparable image fingerprint.  docker prints `sha256:<hex>`, podman prints
# the bare hex, so both sides go through this.
short_id() { sed 's/^sha256://' | cut -c1-12; }

SSH_OPTS=(
  -i "$SRC_SSH_KEY"
  -o BatchMode=yes
  -o ConnectTimeout=5
  -o StrictHostKeyChecking=accept-new
)

# The remote command strings below are assembled here on purpose: every one of
# them is a fixed command with one interpolated name we control (a container
# name, a path), never user input, and quoting them for the far shell instead
# would make them unreadable.
# shellcheck disable=SC2029
src() { ssh "${SSH_OPTS[@]}" "$SRC_SSH" "$@"; }

# A query against the appliance's database.  The SQL goes in on stdin, which
# keeps the remote command line free of quoting entirely.  Read-only use only.
src_q() {
  printf '%s\n' "$1" | src "docker exec -i $SRC_PG psql -U authentik -d authentik -tA"
}

# A query against this box's authentik database.  The postgres image trusts the
# unix socket, so no password is needed (or put on a command line).
dst_q() {
  podman exec -i "$DST_PG" psql -U authentik -d authentik -tA -c "$1"
}

dst_pg_alive() {
  podman exec -i "$DST_PG" pg_isready -U authentik -d authentik >/dev/null 2>&1
}

# The newest dump in $WORK, if any.  Sorted by mtime, newest first; `find
# -printf` rather than `ls` because these are the names this script makes.
dumps_by_age() {
  [ -d "$WORK" ] || return 0
  find "$WORK" -maxdepth 1 -name 'authentik-*.dump' -printf '%T@\t%p\n' \
    | sort -rn | cut -f2-
}

latest_dump() {
  # awk, not head: `head -1` closes the pipe early and the writer dies of
  # SIGPIPE, which `set -o pipefail` correctly reports as a failure.
  dumps_by_age | awk 'NR == 1'
}

# Read KEY out of the appliance's .env.  The value goes straight into the
# caller's variable; nothing here echoes it.
src_env_val() {
  src "sed -n 's/^$1=//p' $SRC_DIR/.env" | awk 'NR == 1' | tr -d '\r'
}

# Read KEY out of this box's rendered sops env file.
dst_env_val() {
  sed -n "s/^$1=//p" "$ENV_FILE" | awk 'NR == 1'
}

require_root() {
  [ "$(id -u)" -eq 0 ] || die "run as root: the containers are system containers and $ENV_FILE is root-only"
}

require_env_file() {
  [ -r "$ENV_FILE" ] || die "$ENV_FILE is not readable.  Add the authentik_env_file sops secret and switch before migrating (docs/authentik-container-migration.md)."
}

# --- check ------------------------------------------------------------------

# The secret half of the preflight, on its own because it is the thing that
# breaks a first boot most often.
cmd_secrets() {
  require_root
  require_env_file

  log "env file $ENV_FILE"

  local key missing=0
  for key in POSTGRES_PASSWORD AUTHENTIK_SECRET_KEY AUTHENTIK_POSTGRESQL__PASSWORD; do
    if [ -z "$(dst_env_val "$key")" ]; then
      warn "$key is missing or empty in $ENV_FILE"
      missing=1
    fi
  done
  [ "$missing" -eq 0 ] || die "required keys missing from $ENV_FILE"

  # podman's --env-file keeps quotes literal, so a value copied out of the
  # quoted, systemd-style shared env_file secret would arrive with its quotes
  # as part of the password.
  local bad="" k v
  for k in $(grep -oE '^[A-Z0-9_]+=' "$ENV_FILE" | tr -d '='); do
    v="$(dst_env_val "$k")"
    case "$v" in
    \"* | *\") bad="$bad $k(quotes)" ;;
    esac
  done
  [ -z "$bad" ] || warn "podman will take these values literally, quotes and all:$bad — the env file must be unquoted KEY=value, see the module's comment"

  local pgpw akpw
  pgpw="$(dst_env_val POSTGRES_PASSWORD)"
  akpw="$(dst_env_val AUTHENTIK_POSTGRESQL__PASSWORD)"
  if [ "$(hash_of "$pgpw")" != "$(hash_of "$akpw")" ]; then
    die "POSTGRES_PASSWORD and AUTHENTIK_POSTGRESQL__PASSWORD in $ENV_FILE differ — the container's role password and authentik's login have to be the same secret"
  fi
  log "POSTGRES_PASSWORD == AUTHENTIK_POSTGRESQL__PASSWORD ($(hash_of "$pgpw"))"

  # AUTHENTIK_SECRET_KEY is what makes a copied database openable: authentik
  # encrypts stored secrets and signs with it.  A mismatch copies a database
  # that boots but cannot authenticate anything.
  local src_key dst_key
  src_key="$(src_env_val AUTHENTIK_SECRET_KEY)"
  dst_key="$(dst_env_val AUTHENTIK_SECRET_KEY)"
  [ -n "$src_key" ] || die "could not read AUTHENTIK_SECRET_KEY from $SRC_SSH:$SRC_DIR/.env"
  if [ "$(hash_of "$src_key")" = "$(hash_of "$dst_key")" ]; then
    log "AUTHENTIK_SECRET_KEY matches the appliance ($(hash_of "$src_key"))"
  else
    die "AUTHENTIK_SECRET_KEY does not match the appliance — the migrated database will not open"
  fi

  # The appliance's database password need not be reused (the target is a new
  # cluster with its own role), but say which way it went.
  local src_pgpw
  src_pgpw="$(src_env_val PG_PASS)"
  if [ -n "$src_pgpw" ] && [ "$(hash_of "$src_pgpw")" = "$(hash_of "$pgpw")" ]; then
    warn "the database password is the appliance's — a fresh one is the better choice, nothing outside this box needs it"
  else
    log "the database password is this install's own ($(hash_of "$pgpw"))"
  fi
}

cmd_check() {
  require_root
  mkdir -p "$WORK"
  chmod 700 "$WORK"

  log "source: $SRC_SSH"
  src "docker inspect -f '{{.State.Running}}' $SRC_SERVER" 2>/dev/null | grep -q true \
    || die "$SRC_SERVER is not running on $SRC_SSH"
  log "appliance container $SRC_SERVER is up"

  # Version skew is the one thing that quietly ruins a migration.
  local src_image src_id dst_id
  src_image="$(src "docker inspect -f '{{.Config.Image}}' $SRC_SERVER")"
  src_id="$(src "docker inspect -f '{{.Image}}' $SRC_SERVER" | short_id)"
  log "appliance image: $src_image ($src_id)"

  dst_id="$(podman image inspect "$src_image" --format '{{.Id}}' 2>/dev/null | short_id || true)"
  if [ -z "$dst_id" ]; then
    die "$src_image is not in this box's image store: podman pull $src_image"
  fi
  if [ "$src_id" != "$dst_id" ]; then
    die "image ID mismatch: this box has $dst_id, the appliance has $src_id.  Restore into the same version or nothing else here means anything."
  fi
  log "this box has the same image ID"

  dst_pg_alive || die "$DST_PG is not accepting connections (systemctl status podman-$DST_PG)"
  log "target database accepts connections"

  local src_ver dst_ver
  src_ver="$(src_q "show server_version" | tr -d ' ' || true)"
  dst_ver="$(dst_q "show server_version" | tr -d ' ' || true)"
  log "postgres: source $src_ver, target $dst_ver"
  [ "$src_ver" = "$dst_ver" ] \
    || warn "postgres versions differ — a same-major restore can be dumped back onto the appliance, a cross-major one cannot"

  cmd_secrets

  local avail
  avail="$(df -Pm "$WORK" | awk 'NR==2 {print $4}')"
  [ "${avail:-0}" -gt 2048 ] || die "only ${avail}MB free on $WORK"
  log "${avail}MB free in $WORK"

  log "preflight passed"
}

# --- dump -------------------------------------------------------------------

cmd_dump() {
  require_root
  mkdir -p "$WORK"
  chmod 700 "$WORK"

  local out
  out="$WORK/authentik-$(date -u +%Y%m%dT%H%M%SZ).dump"
  log "dumping $SRC_PG on $SRC_SSH to $out"

  # `docker exec -i` without a tty: stdout is the dump, stderr is stderr.
  src "docker exec -i $SRC_PG pg_dump -U authentik --format=custom --compress=6 authentik" >"$out.part"
  mv "$out.part" "$out"
  chmod 600 "$out"

  local size
  size="$(stat -c %s "$out")"
  [ "$size" -gt 1000000 ] || die "dump is only $size bytes — that is not the whole database"

  # Read the archive's own table of contents before trusting it.
  podman exec -i "$DST_PG" pg_restore --list <"$out" >/dev/null \
    || die "$out is not a readable custom-format archive"
  log "dumped $size bytes, archive lists cleanly"

  local old
  for old in $(dumps_by_age | tail -n +$((KEEP_DUMPS + 1))); do
    log "removing old dump $old"
    rm -f "$old"
  done

  DUMP="$out"
}

# --- restore ----------------------------------------------------------------

cmd_restore() {
  require_root

  if [ -z "$DUMP" ]; then
    DUMP="$(latest_dump)"
    [ -n "$DUMP" ] || die "no dump in $WORK — run '$0 dump' first"
  fi
  [ -r "$DUMP" ] || die "cannot read $DUMP"
  log "restore $DUMP into $DST_PG (this box's shadow install)"

  # Refuse to overwrite a database someone has already used.  The stack's first
  # boot creates an empty, migrated cluster, so this fires on a re-run — which
  # is exactly when it should.
  local existing
  existing="$(dst_q "select count(*) from information_schema.tables where table_schema='public'" || true)"
  # An empty answer means the query failed, not that the database is empty, and
  # this is the guard that stops a re-run from silently replacing a database
  # someone has been using — so it fails closed.
  [ -n "$existing" ] || die "cannot read the target database (is $DST_PG up?)"
  if [ "$existing" -gt 0 ] && [ "$FORCE" -ne 1 ]; then
    die "the target database already has $existing tables.  Re-run with --force to drop and replace it."
  fi

  # Whatever is running now is what gets started again: in shadow mode the
  # worker is deliberately not a boot-time service, and a restore should not
  # make it one.
  local running=() u
  for u in "${APP_UNITS[@]}"; do
    if systemctl is-active --quiet "$u"; then running+=("$u"); fi
  done
  log "stopping ${APP_UNITS[*]}"
  systemctl stop --ignore-dependencies "${APP_UNITS[@]}"

  log "recreating the authentik database"
  podman exec -i "$DST_PG" psql -U authentik -d postgres -v ON_ERROR_STOP=1 \
    -c 'DROP DATABASE IF EXISTS authentik' \
    -c 'CREATE DATABASE authentik OWNER authentik' >/dev/null

  log "restoring (a minute or two)"
  podman cp "$DUMP" "$DST_PG:/tmp/authentik.dump"
  local rc=0
  podman exec "$DST_PG" pg_restore -U authentik -d authentik \
    --no-owner --no-privileges --jobs 4 \
    /tmp/authentik.dump || rc=$?
  podman exec "$DST_PG" rm -f /tmp/authentik.dump
  if [ "$rc" -ne 0 ]; then
    die "pg_restore exited $rc — the database is half-loaded.  Fix the cause and re-run with --force."
  fi

  if [ "${#running[@]}" -gt 0 ]; then
    log "restarting ${running[*]}"
    systemctl start --ignore-dependencies "${running[@]}"
  else
    warn "nothing was running before, so nothing was started: systemctl start $SERVER_UNIT"
  fi

  log "restored from $DUMP"
  warn "the appliance is untouched — this is a copy.  Run '$0 verify', then look at the shadow vhost."
}

# --- files ------------------------------------------------------------------

cmd_files() {
  require_root

  local d
  for d in media custom-templates certs; do
    if [ -n "$(ls -A "$DST_DIR/$d" 2>/dev/null)" ] && [ "$FORCE" -ne 1 ]; then
      die "$DST_DIR/$d is not empty; --force makes it an exact copy of the appliance's (rsync --delete)"
    fi
  done

  for d in media custom-templates certs; do
    if ! src "test -d $SRC_DIR/$d"; then
      warn "no $d on the appliance, skipping"
      continue
    fi
    log "rsync $d"
    rsync -a --delete --chmod=Du=rwx,Dgo=rx,Fu=rw,Fgo=r \
      -e "ssh ${SSH_OPTS[*]}" \
      "$SRC_SSH:$SRC_DIR/$d/" "$DST_DIR/$d/"
    # The appliance's trees are owned by whoever created them; the server image
    # runs as $AK_UID and writes uploads under /media.
    chown -R "$AK_UID:users" "$DST_DIR/$d"
  done
}

# --- verify -----------------------------------------------------------------

VERIFY_TABLES="
authentik_core_user
authentik_core_group
authentik_core_application
authentik_core_provider
authentik_core_token
authentik_core_propertymapping
authentik_flows_flow
authentik_policies_policy
authentik_outposts_outpost
authentik_tenants_tenant
django_migrations
"

# '-' for a table that does not exist: the point is the comparison, not the
# schema, and a missing table is visible as a pair of dashes.
count_of() {
  case "$1" in
  src) src_q "select count(*) from $2" 2>/dev/null || printf '%s' "-" ;;
  dst) dst_q "select count(*) from $2" 2>/dev/null || printf '%s' "-" ;;
  esac
}

cmd_verify() {
  require_root

  local dump
  dump="${DUMP:-$(latest_dump)}"

  printf '%-36s %10s %10s  %s\n' TABLE SOURCE TARGET STATUS
  local t a b verdict diffs=0
  for t in $VERIFY_TABLES; do
    a="$(count_of src "$t")"
    b="$(count_of dst "$t")"
    verdict=ok
    if [ "$a" != "$b" ]; then
      verdict=DIFFERS
      diffs=$((diffs + 1))
    fi
    printf '%-36s %10s %10s  %s\n' "$t" "$a" "$b" "$verdict"
  done

  printf '\n'
  printf 'schema migrations  src %s  dst %s\n' \
    "$(src_q "select count(*) || '/' || coalesce(max(id),0) from django_migrations" || true)" \
    "$(dst_q "select count(*) || '/' || coalesce(max(id),0) from django_migrations" || true)"
  printf 'database size      src %s  dst %s\n' \
    "$(src_q "select pg_size_pretty(pg_database_size(current_database()))" || true)" \
    "$(dst_q "select pg_size_pretty(pg_database_size(current_database()))" || true)"
  printf 'tenant base_url    dst %s\n' \
    "$(dst_q "select base_url from authentik_tenants_tenant limit 1" || true)"
  if [ -n "$dump" ]; then
    printf 'dump               %s  %s  %s bytes\n' "$dump" \
      "$(stat -c %y "$dump" | cut -d. -f1)" "$(stat -c %s "$dump")"
  fi

  printf '\n'
  if [ "$diffs" -eq 0 ]; then
    log "the copy matches the appliance table for table"
  else
    warn "$diffs table(s) differ — expected if the appliance changed after the dump; re-run dump+restore to close the gap"
  fi
}

# --- outpost tokens ---------------------------------------------------------

cmd_outpost_tokens() {
  require_root
  local out="${OUTPOST_OUT:-$WORK/outpost-tokens.txt}"
  dst_pg_alive || die "$DST_PG is not accepting connections"

  # Each outpost authenticates with an API token named
  # `ak-outpost-<outpost uuid>-api` (that is what the appliance's containers
  # get as AUTHENTIK_TOKEN).  It is a credential, so it goes into a 0600 file
  # and nowhere near stdout.  The embedded proxy outpost is skipped: its token
  # is read from this same database by the server, so copying it out is pure
  # exposure.
  mkdir -p "$(dirname "$out")"
  umask 077
  podman exec -i "$DST_PG" psql -U authentik -d authentik -tA -F $'\t' -c "
      select o.name, t.key
        from authentik_outposts_outpost o
        join authentik_core_token t
          on t.identifier = 'ak-outpost-' || o.uuid::text || '-api'
       where o.type in ('ldap', 'radius')
       order by o.name" >"$out"
  chmod 600 "$out"

  local n
  n="$(grep -c . "$out" || true)"
  if [ "${n:-0}" -eq 0 ]; then
    die "no ldap/radius outpost token found in the database — the outposts migrated but their credentials did not; look at authentik_outposts_outpost and authentik_core_token by hand"
  fi
  log "wrote $n outpost token(s) to $out (0600): $(awk -F'\t' '{printf "%s ", $1}' "$out")"
  log "they belong in the authentik_radius_outpost_env / authentik_ldap_outpost_env sops secrets as AUTHENTIK_TOKEN=<token>; shred this file once they are in"
}

# --- main -------------------------------------------------------------------

usage() {
  cat <<'EOF'
authentik-migrate — copy the authentik appliance at 10.3.1.36 into this box's
podman stack (machines/services1/services/authentik.nix).  Run as root.

  authentik-migrate check             preflight: images, versions, secrets, space
  authentik-migrate secrets           just the secret checks
  authentik-migrate dump              pg_dump from the appliance into /srv/authentik/migration
  authentik-migrate restore           into this box's authentik-postgres container
  authentik-migrate files             media/, custom-templates/, certs/
  authentik-migrate verify            row counts, appliance vs this box
  authentik-migrate outpost-tokens    extract outpost config tokens to a 0600 file
  authentik-migrate all --force         check, dump, restore, files, verify

Options: --force (replace a non-empty target), --dump FILE, --out FILE,
--source user@host.  The appliance is only read; nothing on it is restarted.

--force is needed by `restore`, `files` and therefore `all` on any box where
the stack has booted once: the first boot leaves a stock, freshly migrated
database behind, and both commands refuse to overwrite something they did not
create.  The shadow is disposable — `all --force` rebuilds it from the
appliance in half a minute — which is the whole point of running it.
EOF
}

cmd="${1:-}"
shift || true

while [ $# -gt 0 ]; do
  case "$1" in
  --force)
    FORCE=1
    shift
    ;;
  --dump)
    [ $# -ge 2 ] || die "--dump needs a file"
    DUMP="$2"
    shift 2
    ;;
  --out)
    [ $# -ge 2 ] || die "--out needs a file"
    OUTPOST_OUT="$2"
    shift 2
    ;;
  --source)
    [ $# -ge 2 ] || die "--source needs user@host"
    SRC_SSH="$2"
    shift 2
    ;;
  -h | --help)
    usage
    exit 0
    ;;
  *) die "unknown option: $1" ;;
  esac
done

case "$cmd" in
check) cmd_check ;;
secrets) cmd_secrets ;;
dump) cmd_dump ;;
restore) cmd_restore ;;
files) cmd_files ;;
verify) cmd_verify ;;
outpost-tokens) cmd_outpost_tokens ;;
all)
  cmd_check
  cmd_dump
  cmd_restore
  cmd_files
  cmd_verify
  ;;
'' | help)
  usage
  exit 1
  ;;
*) die "unknown command: $cmd (see '$0 help')" ;;
esac
