#!/usr/bin/env bash
# openipc-migration.sh — move a SEALED OpenIPC camera to Thingino with
# nothing but network access (two phases, design in openipc-migration.md).
#
#   usage: MIG_PASS=... ./openipc-migration.sh --target 10.3.2.17 \
#            --images .../images --backup ./backup-01 [--phase 1|2|all] --go
#
# Phase 1 runs while the unit still boots its own U-Boot (no hush parser):
#   backup every mtd to the host and verify sha256 -> rewrite the env to the
#   Thingino geometry with a SIMPLE-CLI bootcmd (their parser executes every
#   `;` segment unconditionally, so an `if`-gated rescue line would erase
#   partitions on the classic parser — never write one) -> carve the Thingino
#   kernel+rootfs against the TARGET's map: Thingino rootfs lives at its own
#   rootaddr, which can overlap the tail of the target's kernel partition, so
#   the kernel-partition image is [uImage | FF pad | rootfs head] and the
#   rootfs partition gets the remainder -> flash_eraseall the overlay ->
#   reboot into Thingino (Wi-Fi credentials baked; SD uenv.txt as fallback).
# Phase 2 runs from the booted Thingino and delivers the "last opening ever":
#   flashcp our u-boot to mtd0 + the real env image (rescue bootcmd) to mtd1,
#   read back and hash-verify AT SOURCE LENGTH (tip 22), reboot. The window
#   where power-loss means clip-recovery is the seconds of the mtd0 write.
#
# Writes happen inside a detached on-target script whose interpreter chain
# (loader, libc, applets) is staged to tmpfs and exec'd via the loader with
# --library-path — dynamically-linked busybox means copying the binary
# alone is NOT RAM-residency (see phase 1 comments). Rehearse on a
# serial-attached unit before any sealed install (prerequisite 4).
#
# Exit codes: 2 usage, 3 preflight/backup, 4 write/verify, 5 unreachable.
set -euo pipefail

TARGET=""; IMAGES=""; BACKUP=""; PHASE="all"; GO=0; SSHPASS="${MIG_PASS:-}"

usage(){ sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }
while [ $# -gt 0 ]; do case "$1" in
  --target) TARGET=$2; shift 2;; --images) IMAGES=$2; shift 2;;
  --backup) BACKUP=$2; shift 2;; --phase) PHASE=$2; shift 2;;
  --go) GO=1; shift;; --help|-h) usage;; *) usage;; esac; done
[ -n "$TARGET" ] && [ -n "$IMAGES" ] && [ -n "$BACKUP" ] || usage
[ "$GO" = 1 ] || { echo "refusing to write without --go"; exit 2; }
[ -d "$IMAGES" ] || { echo "images dir missing: $IMAGES"; exit 2; }
for f in uImage rootfs.squashfs u-boot-ssc333-nor.bin u-boot-env.bin uenv.txt; do
  [ -f "$IMAGES/$f" ] || { echo "missing $IMAGES/$f"; exit 2; }; done
mkdir -p "$BACKUP"; LOG="$BACKUP/migration-$(date +%Y%m%d-%H%M%S).log"; exec > >(tee -a "$LOG") 2>&1

# ---- transport: password ssh without sshpass (host has none) ----
ASKPASS=$(mktemp); trap 'rm -f "$ASKPASS"' EXIT
printf '#!/bin/sh\necho "%s"\n' "$SSHPASS" > "$ASKPASS"; chmod +x "$ASKPASS"
SSH_OPTS=(-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null
          -o ConnectTimeout=10 -o LogLevel=ERROR -o NumberOfPasswordPrompts=1)
tsh(){ SSH_ASKPASS="$ASKPASS" SSH_ASKPASS_REQUIRE=force setsid -w \
       ssh "${SSH_OPTS[@]}" "root@$TARGET" "$1"; }
tpush(){ SSH_ASKPASS="$ASKPASS" SSH_ASKPASS_REQUIRE=force setsid -w \
       ssh "${SSH_OPTS[@]}" "root@$TARGET" "cat > $2" < "$1"; }
tpull(){ SSH_ASKPASS="$ASKPASS" SSH_ASKPASS_REQUIRE=force setsid -w \
       ssh "${SSH_OPTS[@]}" "root@$TARGET" "$2" > "$1"; }
say(){ echo "[$(date +%H:%M:%S)] $*"; }

# ---- Thingino geometry + env from the build's uenv.txt (no hardcoding) ----
getvar(){ sed -n "s/^$1=//p" "$IMAGES/uenv.txt" | head -1; }
K_ADDR=$(getvar kernaddr); K_SIZE=$(getvar kernsize)
R_ADDR=$(getvar rootaddr); R_SIZE=$(getvar rootsize)
SIMPLE_BOOTCMD='sf probe 0; setenv setargs setenv bootargs ${bootargs}; run setargs; sf read ${baseaddr} ${kernaddr} ${kernsize}; bootm ${baseaddr}'

# expected OpenIPC-side map (sealed fleet as reconnoitred 2026-10-30)
declare -A WANT=([mtd0]=00040000 [mtd1]=00010000 [mtd2]=00200000 [mtd3]=00500000 [mtd4]=000b0000)

phase1(){
  say "== phase 1: preflight against $TARGET =="
  tsh 'echo PONG' | grep -q PONG || { say "unreachable"; exit 5; }
  local os; os=$(tsh 'cat /etc/os-release 2>/dev/null | head -4; uname -r')
  echo "$os" | grep -qi buildroot || { echo "target is not OpenIPC/buildroot — abort"; exit 3; }
  tsh 'cat /etc/fw_env.config' | grep -Eq '^/dev/mtd1[[:space:]]+0(x)?[[:space:]]+0x10000[[:space:]]+0x10000' \
    || { echo "unexpected fw_env.config — env_offset assumption unverified, abort"; exit 3; }
  local mtd; mtd=$(tsh 'cat /proc/mtd')
  echo "$mtd"
  for d in "${!WANT[@]}"; do
    echo "$mtd" | grep -q "^[[:space:]]*$d:[[:space:]]*${WANT[$d]}" \
      || { echo "partition $d != ${WANT[$d]} — abort (map drift?)"; exit 3; }
  done
  tsh 'for t in flashcp flash_eraseall sha256sum dd; do command -v $t >/dev/null || echo MISSING:$t; done' \
    | grep -q MISSING && { echo "missing mtd tools on target"; exit 3; }

  say "== full-chip backup (hard gate) =="
  for d in mtd0 mtd1 mtd2 mtd3 mtd4; do
    [ -f "$BACKUP/$d.bin" ] && { echo "$BACKUP/$d.bin exists, refusing to overwrite"; exit 3; }
    tpull "$BACKUP/$d.bin.tmp" "dd if=/dev/$d bs=64k 2>/dev/null"
    local want have; want=$(tsh "sha256sum /dev/$d" | cut -d' ' -f1)
    have=$(sha256sum "$BACKUP/$d.bin.tmp" | cut -d' ' -f1)
    [ "$want" = "$have" ] || { echo "$d backup hash mismatch — abort before any write"; exit 3; }
    mv "$BACKUP/$d.bin.tmp" "$BACKUP/$d.bin"; say "  $d backed up, sha256 verified"
  done
  tsh 'fw_printenv' > "$BACKUP/fw_printenv.theirs.txt"
  echo "$TARGET" > "$BACKUP/target.txt"; date > "$BACKUP/when.txt"

  say "== carve Thingino payloads against the target map =="
  local U_K RALL IMG_K IMG_R
  U_K=$(stat -c%s "$IMAGES/uImage"); RALL=$(stat -c%s "$IMAGES/rootfs.squashfs")
  [ "$U_K" -le $((0x1F0000)) ] || { echo "uImage $U_K does not fit Thingino kernel offset headroom"; exit 4; }
  [ $((RALL - 0x10000)) -le $((0x500000)) ] || { echo "rootfs tail exceeds their 5120k rootfs partition (slack gone)"; exit 4; }
  IMG_K="$BACKUP/carve-IMG-K.bin"; IMG_R="$BACKUP/carve-IMG-R.bin"
  python3 - "$IMAGES/uImage" "$IMAGES/rootfs.squashfs" "$IMG_K" "$IMG_R" <<'PYEOF'
import sys
img, rfs, kout, rout = sys.argv[1:5]
u = open(img,'rb').read(); r = open(rfs,'rb').read()
assert len(u) <= 0x1F0000 and len(r) > 0x10000
open(kout,'wb').write(u + b'\xff'*(0x1F0000-len(u)) + r[:0x10000])   # exactly 2 MiB
open(rout,'wb').write(r[0x10000:])
PYEOF
  say "  IMG-K $(stat -c%s "$IMG_K") (uImage ${U_K} + rootfs head @0x1F0000), IMG-R $(stat -c%s "$IMG_R")"

  say "== env -> Thingino geometry, SIMPLE parser bootcmd =="
  local memlx memsz
  memlx=$(tsh 'fw_printenv -n memlx 2>/dev/null' | tr -dc '0-9a-fA-Fx') || memlx=""
  memsz=$(tsh 'fw_printenv -n memsz 2>/dev/null' | tr -dc '0-9a-fA-Fx') || memsz=""
  [ -n "$memlx" ] || { memlx=0x3FE0000; say "  WARN memlx absent, defaulting"; }
  [ -n "$memsz" ] || { memsz=0x2000000; say "  WARN memsz absent, defaulting (their kernel owned this before)"; }
  cat > "$BACKUP/setenv.sh" <<ENVEOF
#!/bin/sh
set -e
fw_setenv baseaddr $(getvar baseaddr)
fw_setenv kernaddr $K_ADDR
fw_setenv kernsize $K_SIZE
fw_setenv rootaddr $R_ADDR
fw_setenv rootsize $R_SIZE
fw_setenv dataaddr $(getvar dataaddr)
fw_setenv datasize $(getvar datasize)
fw_setenv mtdparts '$(getvar mtdparts)'
fw_setenv bootargs '$(getvar bootargs)'
fw_setenv bootcmd '$SIMPLE_BOOTCMD'
fw_setenv memlx $memlx
fw_setenv memsz $memsz
fw_setenv bootdelay 1
for v in sensor rootmtd uknor urnor ubnor bootcmdnor setsdcard updatetool; do fw_setenv \$v >/dev/null 2>&1 || true; done
echo ENV-DONE
ENVEOF
  tpush "$BACKUP/setenv.sh" /tmp/setenv.sh
  tsh 'sh /tmp/setenv.sh; echo rc=$?; fw_printenv bootcmd' | tee "$BACKUP/env-after.txt"
  grep -q ENV-DONE "$BACKUP/env-after.txt" || { echo "env rewrite failed"; exit 4; }

  say "== RAM-resident writer: kernel+rootfs+overlay, then reboot =="
  cat > "$BACKUP/writer.sh" <<'WEOF'
#!/tmp/miglib/sh
# Runs under /tmp/miglib/loader (see staging block); child tools must be
# exec'd through the loader too, else the kernel maps /lib/ld-musl from the
# very squashfs being erased. $(date)/sleep/sync are busybox builtins.
LDR=/tmp/miglib/loader; LP="--library-path /tmp/miglib"
run(){ $LDR $LP "$@"; }
exec >>/tmp/mig-writer.log 2>&1; echo "writer start $(date)"
run /tmp/miglib/flashcp -v /tmp/IMG-K.bin /dev/mtd2 && \
run /tmp/miglib/flashcp -v /tmp/IMG-R.bin /dev/mtd3 && \
run /tmp/miglib/flash_eraseall /dev/mtd4 && { echo WROTE-OK; sync; sleep 1; run /tmp/miglib/reboot -f; }
echo "WRITER FAILED rc=$?"
WEOF
  tpush "$IMG_K" /tmp/IMG-K.bin
  tsh 'sha256sum /tmp/IMG-K.bin' | cut -d' ' -f1 > "$BACKUP/.h1"
  [ "$(cut -d' ' -f1 "$BACKUP/.h1")" = "$(sha256sum "$IMG_K" | cut -d' ' -f1)" ] || { echo IMG-K push corrupt; exit 4; }
  tpush "$IMG_R" /tmp/IMG-R.bin
  tsh 'sha256sum /tmp/IMG-R.bin' | cut -d' ' -f1 > "$BACKUP/.h2"
  [ "$(cut -d' ' -f1 "$BACKUP/.h2")" = "$(sha256sum "$IMG_R" | cut -d' ' -f1)" ] || { echo IMG-R push corrupt; exit 4; }
  tpush "$BACKUP/writer.sh" /tmp/writer.sh
  # applets MUST be invocable via argv[0]: busybox on these units is
  # FEATURE_INDIVIDUAL (`bb flashcp` -> "applet not found"; a copy named
  # flashcp works — proven on cam8 2026-10-30).
  #
  # THE TRAP IS MUSL: busybox is *dynamically* linked (PT_INTERP
  # /lib/ld-musl-armhf.so.1), so copying the binary to tmpfs is NOT
  # RAM-residency — its libc pages are demand-paged from the squashfs.
  # Phase 1 on cam8 2026-10-31 died exactly there: flashcp erasing mtd3
  # yanked the writer's own libc pages out from under it (SIGBUS mid-rootfs,
  # zombie unit). Fix: stage loader + libs + applets in /tmp/miglib (tmpfs)
  # and exec every child through the loader with an explicit --library-path,
  # so no mapping is ever rooted in flash. uClibc loaders ignore
  # --library-path but honor LD_LIBRARY_PATH; pass both.
  tsh 'grep -q " /tmp tmpfs" /proc/mounts || echo NO-TMPFS' | grep -q NO-TMPFS \
    && { echo "/tmp is not tmpfs on the target — writer would erase its own filesystem; abort"; exit 4; }
  tsh 'BB=$(readlink -f /bin/busybox 2>/dev/null || echo /bin/busybox);
       rm -rf /tmp/miglib; mkdir -p /tmp/miglib;
       for L in /lib/ld-musl-*.so.* /lib/ld-uClibc* /lib/libc.so* /lib/libm.so* /lib/libgcc_s.so*; do
         [ -e "$L" ] && cp -Lf "$L" /tmp/miglib/ 2>/dev/null; done;
       LDR=$(ls /tmp/miglib/ld-musl-*.so.* /tmp/miglib/ld-uClibc* 2>/dev/null | head -1);
       [ -n "$LDR" ] && cp "$LDR" /tmp/miglib/loader;
       for a in sh flashcp flash_eraseall reboot head; do cp "$BB" /tmp/miglib/$a; done;
       chmod -R a+x /tmp/miglib; ls /tmp/miglib | tr "\n" " "; echo; echo STAGED'
  tsh '/tmp/miglib/loader --library-path /tmp/miglib /tmp/miglib/sh -c "echo LDR-SANITY"' \
    | grep -q LDR-SANITY \
    || { echo "staged loader cannot exec staged sh — refusing unsafe writer"; exit 4; }
  tsh 'LD_LIBRARY_PATH=/tmp/miglib nohup /tmp/miglib/loader --library-path /tmp/miglib /tmp/miglib/sh /tmp/writer.sh </dev/null >/dev/null 2>&1 & echo WRITER-LAUNCHED' | grep -q WRITER-LAUNCHED \
    || { echo "could not launch writer"; exit 4; }
  say "writer launched; unit should reappear as THINGINO on the baked network(s)"
}

phase2(){
  say "== phase 2 (from Thingino): take over the boot region =="
  tsh 'grep -qi thingino /etc/os-release && echo IS-THINGINO' | grep -q IS-THINGINO \
    || { echo "phase 2 requires the unit already running Thingino"; exit 3; }
  tsh 'grep -q "if fatload" /etc/fw_env.config 2>/dev/null; fw_printenv bootcmd' | grep -q "if fatload" \
    && { say "  already on the rescue bootcmd — skipping"; return 0; }
  tpush "$IMAGES/u-boot-ssc333-nor.bin" /tmp/uboot.bin
  tpush "$IMAGES/u-boot-env.bin" /tmp/env.img
  tsh 'BB=$(readlink -f /bin/busybox); for a in sh flashcp head reboot; do cp $BB /tmp/$a; done;
       chmod +x /tmp/'{sh,flashcp,head,reboot}'; /tmp/flashcp 2>&1 | head -1 || true' >/dev/null
  local US ES; US=$(stat -c%s "$IMAGES/u-boot-ssc333-nor.bin"); ES=$(stat -c%s "$IMAGES/u-boot-env.bin")
  local CU CE; CU=$(sha256sum "$IMAGES/u-boot-ssc333-nor.bin" | cut -d' ' -f1)
  CE=$(sha256sum "$IMAGES/u-boot-env.bin" | cut -d' ' -f1)
  # verify-readback AT SOURCE LENGTH (tip 22: trailing erased FF is not a mismatch)
  local RES
  RES=$(tsh "/tmp/flashcp -v /tmp/uboot.bin /dev/mtd0 >/dev/null && /tmp/flashcp -v /tmp/env.img /dev/mtd1 >/dev/null && sync &&
    R1=\$(dd if=/dev/mtd0 bs=65536 2>/dev/null | /tmp/head -c $US | sha256sum | cut -d' ' -f1) &&
    R2=\$(dd if=/dev/mtd1 bs=65536 2>/dev/null | /tmp/head -c $ES | sha256sum | cut -d' ' -f1) &&
    echo \"\$R1/\$R2\"" || echo WRITE-FAIL)
  # flashcp progress noise (phase 2 run 2 2026-10-31: hashes were equal yet
  # the comparison failed because \\r-spinner text rode along in $RES)
  RES=$(printf '%s' "$RES" | tr -d '\r' | grep -oE '[0-9a-f]{64}/[0-9a-f]{64}' | tail -1)
  if [ "$RES" != "$CU/$CE" ]; then
    say "READBACK MISMATCH — boot region NOT confirmed; unit may be on new u-boot already or not; check serial before retry"
    echo "got: $RES"; echo "want: $CU/$CE"; exit 4
  fi
  say "boot region + env written and read-back verified; rebooting"
  tsh '/tmp/reboot' || true
  say "phase 2 done; after reboot verify: U-Boot banner from our build, 'if fatload' in bootcmd, SD rescue works"
}

wait_and_verify(){
  # Measured on cam8 2026-10-31: the writer needs several minutes (erase +
  # program ~7.3 MB of NOR) and the unit answers nothing until it reboots.
  # 180 s produced a false TIMEOUT on a perfectly successful phase 1.
  say "waiting for the unit to come back as Thingino (up to 900 s)"
  for i in $(seq 1 90); do
    sleep 10
    if tsh 'grep -qi thingino /etc/os-release && fw_printenv bootcmd 2>/dev/null | grep -q sf && echo BACK' 2>/dev/null | grep -q BACK; then
      say "unit is back after ~$((i*10)) s. env bootcmd:"; tsh 'fw_printenv bootcmd; fw_printenv -n sensor 2>/dev/null || echo "sensor: (autodetect)"'
      return 0
    fi
    [ $((i % 6)) = 0 ] && say "  not yet ($((i*10)) s)"
  done
  say "TIMED OUT — serial console time. Do not panic: the env-first ordering means the payload chain is already Thingino unless power died mid-write."
  exit 5
}

case "$PHASE" in
  1) phase1; wait_and_verify;;
  2) phase2; wait_and_verify;;
  all) phase1; wait_and_verify; phase2; wait_and_verify; say "MIGRATION COMPLETE — run one SD-rescue rehearsal on this unit to prove the last-opening claim";;
  *) usage;;
esac
