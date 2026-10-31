#!/usr/bin/env bash
# thingino-build.sh — build the fleet's thingino firmware image from the pinned
# port branch, then verify it against cameras/thingino-manifest.sh.
#
#   usage: cameras/thingino-build.sh            # build + verify the manifest commit
#   usage: THINGINO_ALLOW_OTHER_HEAD=1 ...      # build what the tree happens to hold
#
# The build is a containerised Buildroot run (ghcr.io/themactep/thingino-builder-image
# via podman/docker); this recipe builds no Nix anything, so it works on any box
# that holds the tree — in practice the bench, because the branch is local to it
# (docs/camera-thingino-build-deploy-2026-10-31.md "What a build needs").
#
# Every refusal below is a bench lesson turned into a gate; the receipts are that
# doc and the bench execution log:
#   * `container-make` + `MAKECMDGOALS=br-all`, never a bare `all` — with no goal
#     Makefile.container just pulls the builder image and exits 0 having built
#     nothing, and `MAKECMDGOALS=all` on a SigmaStar camera dies at the end on the
#     Ingenic-only `uboot-dirclean` rule after an hour of real work (bench tips 11–12).
#   * the submodule lands on $BUILDROOT_PIN via a plain `--init --recursive`;
#     the fork's own Makefile rule would run `--remote` and move it (tip 4).
#   * a dirty or moved tree builds something the manifest does not describe —
#     which is only wrong if you do not know it, so the gate prints the override.
#   * a rebuild at ONE commit does not reproduce the flashed bytes (squashfs
#     timestamps, u-boot container padding — measured 2026-10-31, the header of
#     docs/camera-thingino-build-deploy-2026-10-31.md §2 has the receipts), so a
#     non-empty images dir is moved to trash/ instead of being written over.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=thingino-manifest.sh
. "$here/thingino-manifest.sh"

TREE=${THINGINO_TREE:-$FW_TREE_DEFAULT}
[ -d "$TREE/.git" ] || {
    echo "no thingino tree at $TREE" >&2
    echo "  the port lives on branch $FW_BRANCH of a clone of $FW_GIT_URL," >&2
    echo "  and that branch is on the bench only — clone the tree there or point THINGINO_TREE at it." >&2
    exit 1
}

cd "$TREE"
head=$(git rev-parse HEAD)
branch=$(git rev-parse --abbrev-ref HEAD)
if [ "$head" != "$FW_COMMIT" ] && [ -z "${THINGINO_ALLOW_OTHER_HEAD:-}" ]; then
    echo "HEAD is $head ($branch), manifest pins $FW_COMMIT ($FW_BRANCH)." >&2
    echo "Building anything else is fine — it is just not the image this repo's hashes," >&2
    echo "$FW_COMMIT_SHORT, and the flashed state of cam5/cam8 describe. Rerun with" >&2
    echo "THINGINO_ALLOW_OTHER_HEAD=1 and move cameras/thingino-manifest.sh with the result." >&2
    exit 1
fi
if [ -n "$(git status --porcelain)" ] && [ -z "${THINGINO_ALLOW_DIRTY:-}" ]; then
    echo "working tree is dirty: the build would not correspond to $FW_COMMIT_SHORT." >&2
    echo "Commit or stash it, or rerun with THINGINO_ALLOW_DIRTY=1 to build a known-dirty tree" >&2
    echo "(the verify step will still refuse to stage artifacts the manifest does not know)." >&2
    exit 1
fi
if [ ! -f buildroot/Makefile ]; then
    echo "buildroot submodule not populated. Run (plain, never --remote):" >&2
    echo "  git -C $TREE submodule update --init --recursive" >&2
    exit 1
fi
# git prefixes the sha with ' ', '+' or '-' depending on sync state; awk field
# splitting eats a leading space but not a +/-, so strip one non-hex character.
sub=$(git submodule status | awk '$2 == "buildroot" {s = $1; sub(/^[^0-9a-f]/, "", s); print s}')
if [ "$sub" != "$BUILDROOT_PIN" ]; then
    echo "buildroot submodule is ${sub:-missing}, manifest pins $BUILDROOT_PIN." >&2
    echo "A firmware built against other Buildroot is not this firmware (bench tip 4)." >&2
    exit 1
fi

# Host shape: the build hardcodes /bin/bash, which NixOS does not have, and wants
# ~10 GB free for a cold build (bench tips 1/3).
if ! [ -x /bin/bash ] && ! command -v bash >/dev/null; then
    echo "no bash on this host — the build hardcodes /bin/bash." >&2; exit 1
fi
if command -v podman >/dev/null 2>&1 || command -v docker >/dev/null 2>&1; then :; else
    echo "neither podman nor docker: the builder image is ghcr.io/themactep/thingino-builder-image." >&2
    echo "  on this fork CONTAINER_ENGINE picks podman first (Makefile.container)." >&2
    exit 1
fi
free_kb=$(df -Pk "$TREE" | awk 'NR==2 {print $4}')
if [ "${free_kb:-0}" -lt $((10 * 1024 * 1024)) ]; then
    echo "under 10 GB free on $TREE ($((free_kb / 1024 / 1024)) GB) — a cold build dies mid-rootfs." >&2
    exit 1
fi

log="$HOME/camera-images/thingino-build-$(date -u +%Y%m%dT%H%M%SZ).log"
mkdir -p "$HOME/camera-images"

# Never overwrite the current images dir in place. A rebuild of the SAME commit
# is not byte-identical (measured 2026-10-31 by exactly this recipe's smoke test:
# squashfs embeds build timestamps — rootfs md5 f14ae863 vs the flashed ce3b7ef9 —
# and the u-boot container came out 244,676 B where the flashed one is 244,668),
# and the bytes cam5/cam8 run exist nowhere else on this machine except the units
# themselves. The rebuild therefore goes to a fresh directory; the old one moves
# to trash/ and the flashed bytes survive an accidental `cam-fw-build`.
# (bench AGENTS: never delete generated files irreversibly, move to trash/)
IMAGES_DIR="$TREE/$FW_IMAGES_SUBDIR"
if [ -d "$IMAGES_DIR" ] && [ -n "$(ls -A "$IMAGES_DIR" 2>/dev/null)" ] && [ -z "${THINGINO_REBUILD_IN_PLACE:-}" ]; then
    aside="$TREE/trash/images-$(date -u +%Y%m%dT%H%M%SZ)"
    mkdir -p "$TREE/trash"
    echo "=== moving the existing images dir aside: $IMAGES_DIR -> $aside"
    echo "    (a rebuild at one commit is not the flashed bytes; THINGINO_REBUILD_IN_PLACE=1 overrides)"
    mv "$IMAGES_DIR" "$aside"
fi

echo "=== building $FW_CAMERA from $branch @ $(git rev-parse --short HEAD); log: $log"
echo "=== cold build ~40 min; the line 'Running make ... in container' is the proof work started"

# SHELL= because Makefile.container shells out and NixOS has no /bin/bash in PATH
# assumptions; WORKFLOW=1 for non-interactivity (bench AGENTS, "Build tips").
WORKFLOW=1 make -f Makefile.container container-make \
    SHELL="$(command -v bash)" \
    CAMERA="$FW_CAMERA" \
    MAKECMDGOALS=br-all 2>&1 | tee "$log"
rc=${PIPESTATUS[0]}
if [ "$rc" != 0 ]; then
    # First `***` error is the one to read; the last line is where Buildroot dies
    # for unrelated reasons (bench working style).
    echo "build failed (exit $rc). First error lines from $log:" >&2
    grep -m 10 '\*\*\*' "$log" >&2 || tail -20 "$log" >&2
    exit "$rc"
fi

echo "=== build finished; verifying artifacts against the manifest"
exec "$here/thingino-verify.sh"
