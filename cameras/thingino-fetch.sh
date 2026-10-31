#!/usr/bin/env bash
# thingino-fetch.sh — restage the fleet's bytes of record from the SSOT release
# into a tree's images dir, then hand them to the manifest verifier.
#
#   usage: cameras/thingino-fetch.sh [target-images-dir]
#
# The release assets are the flashed bytes; a rebuild from the same commit
# produces content-identical equivalents but never these bytes (docs §2), so
# anything that must flash exactly what cam5/cam8 run — the rescue card and the
# changeover — starts here when no staged build is around.  Integrity is NOT
# taken from the release's own checksum files (self-signed): this script only
# moves bytes into place and runs cameras/thingino-verify.sh, and the manifest
# remains the single authority.  Needs authenticated `gh`.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=thingino-manifest.sh
. "$here/thingino-manifest.sh"

TARGET=${1:-${THINGINO_TREE:-$FW_TREE_DEFAULT}/${FW_IMAGES_SUBDIR}}
command -v gh >/dev/null || { echo "needs the GitHub CLI authenticated for $FW_REPO" >&2; exit 1; }

tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
echo "=== downloading ${FW_RELEASE} assets (payload files only)"
gh release download "$FW_RELEASE" --repo "$FW_REPO" --pattern "uImage" \
    --pattern "rootfs.squashfs" --pattern "u-boot-ssc333-nor.bin" \
    --pattern "u-boot-env.bin" --pattern "thingino-*.bin" \
    --pattern "uenv.txt" --pattern "sigmastar-mtdparts.env" --dir "$tmp"
mkdir -p "$TARGET"
# Move only after the full set is present and matches sizes; a half-staged
# images dir is the one state a camera must never be flashed from.
for f in uImage rootfs.squashfs u-boot-ssc333-nor.bin u-boot-env.bin \
         "thingino-${FW_CAMERA}.bin" uenv.txt sigmastar-mtdparts.env; do
    [ -f "$tmp/$f" ] || { echo "release held no $f — aborting, nothing staged" >&2; exit 1; }
    mv -f "$tmp/$f" "$TARGET/$f"
done
echo "=== staged into $TARGET; verifying against the manifest"
exec "$here/thingino-verify.sh" "$TARGET"
