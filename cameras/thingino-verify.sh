#!/usr/bin/env bash
# thingino-verify.sh — check a thingino images directory against
# cameras/thingino-manifest.sh: bytes, md5 and sha256 per artifact, and the
# flash geometry out of the build's own uenv.txt rather than out of prose.
#
#   usage: cameras/thingino-verify.sh [images-dir]
#
# Default images-dir is the bench tree's current output
# ($THINGINO_TREE/<FW_IMAGES_SUBDIR>).  Exit 0 only when every line is ok; a
# FAIL names the artifact and which check disagreed.
#
# Why three checks: a rebuild changes all of them together (that is the point of
# pinning), and the fleet has already been bitten by "a green build with the old
# binary inside" (bench AGENTS tip 13) and by a read-back hashed at the wrong
# length (tip 22).  Size is the check that catches a truncated copy; the hashes
# are the ones the rescue-card and migration flows repeat on the camera side.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=thingino-manifest.sh
. "$here/thingino-manifest.sh"

IMAGES=${1:-${THINGINO_TREE:-$FW_TREE_DEFAULT}/${FW_IMAGES_SUBDIR}}
[ -d "$IMAGES" ] || { echo "no images dir: $IMAGES (build it: just cam-fw-build)" >&2; exit 1; }

fails=0
check() { # name bytes md5 sha256 — ok only when all three agree
    local f="$IMAGES/$1" want_sz=$2 want_md5=$3 want_sha=$4 got bad=0
    if [ ! -f "$f" ]; then
        echo "FAIL  $1  missing from $IMAGES"
        fails=$((fails + 1)); return
    fi
    got=$(stat -c %s "$f")
    [ "$got" = "$want_sz" ] || { echo "FAIL  $1  size $got != $want_sz"; bad=1; }
    got=$(md5sum "$f" | cut -d' ' -f1)
    [ "$got" = "$want_md5" ] || { echo "FAIL  $1  md5 $got != $want_md5"; bad=1; }
    got=$(sha256sum "$f" | cut -d' ' -f1)
    [ "$got" = "$want_sha" ] || { echo "FAIL  $1  sha256 ${got:0:16}… != ${want_sha:0:16}…"; bad=1; }
    if [ "$bad" = 1 ]; then
        fails=$((fails + 1))
    else
        echo "ok    $1  ${want_sz} B"
    fi
}

while IFS='|' read -r name sz md5 sha; do
    [ -n "${name:-}" ] || continue
    check "$name" "$sz" "$md5" "$sha"
done <<<"$ARTIFACTS"

# Geometry: uenv.txt is what phase 2 of the migration writes into the env, and
# it is what makes a payload land at the address the flashed u-boot expects.  A
# drift here between manifest and build means this table no longer describes the
# flashed units, which is exactly the class of error that costs a camera.
if [ -f "$IMAGES/uenv.txt" ]; then
    for kv in kernaddr:$GEOM_KERNADDR kernsize:$GEOM_KERNSIZE rootaddr:$GEOM_ROOTADDR \
              rootsize:$GEOM_ROOTSIZE dataaddr:$GEOM_DATAADDR datasize:$GEOM_DATASIZE; do
        k=${kv%%:*}; want=${kv#*:}
        got=$(sed -n "s/^$k=//p" "$IMAGES/uenv.txt" | head -1)
        if [ "$got" = "$want" ]; then
            echo "ok    uenv.txt $k=$got"
        else
            echo "FAIL  uenv.txt $k=$got != manifest $want"
            fails=$((fails + 1))
        fi
    done
    # The rescue gate is the payload-update mechanism (RESCUE.KRN/RESCUE.RFS +
    # S98 eating the markers).  An env without it is a unit that cannot be
    # updated by card, so it is checked, not assumed.
    if grep -q 'fatload mmc 0 ${baseaddr} RESCUE.KRN' "$IMAGES/uenv.txt"; then
        echo "ok    uenv.txt bootcmd carries the SD rescue gate"
    else
        echo "FAIL  uenv.txt bootcmd has no RESCUE.KRN gate — this env cannot take a rescue card"
        fails=$((fails + 1))
    fi
else
    echo "FAIL  uenv.txt missing from $IMAGES"
    fails=$((fails + 1))
fi

if [ "$fails" != 0 ]; then
    echo "NOTHING STAGED: $fails finding(s). This images dir does not match the manifest;" >&2
    echo "if the tree moved past FW_COMMIT $FW_COMMIT_SHORT, rebuild and move the manifest with it." >&2
    exit 1
fi
echo "PASS  every artifact and the geometry match cameras/thingino-manifest.sh (FW_COMMIT $FW_COMMIT_SHORT)." >&2
