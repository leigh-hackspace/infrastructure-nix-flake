# PACKAGE — the ROCm 10.0.0 (TheRock) *runtime* the Strata engine can load instead of
# nixpkgs' ROCm 7.2, as a store path.  Consumed by ./strata.nix (through the run
# config's `lib_dirs`) and buildable on its own as `nix build .#rocm10-runtime`.
#
# Why a runtime outside nixpkgs at all
# ------------------------------------
# nixpkgs has exactly one ROCm: `pkgs.rocmPackages` is `callPackage
# ../development/rocm-modules {}`, its rocm-core is 7.2.4 at this flake's nixpkgs,
# and there is no `rocmPackages_8/_9/_10` to fall back on and no second scope to
# overlay.  So "run on ROCm 10" cannot be a package attribute here; it has to be a
# runtime fetched from where AMD publishes one and handed to the engine through
# `LD_LIBRARY_PATH`.
#
# That is what this file is, and the mechanism is small: the engine binary stays a
# nixpkgs build (compiled against 7.2.4's headers, RUNPATH intact); ./strata.nix
# puts this directory in the run config's `lib_dirs`, and serve/server.py's
# `child_env()` **prepends** it to the engine's `LD_LIBRARY_PATH`, which beats the
# RUNPATH — so the same binary loads ROCm 10's `libamdhip64.so.7` and
# `libhsa-runtime64.so.1`.  `child_env()` silently DROPS a missing directory, which
# is exactly why this is a store path and not a hand-laid `/opt/rocm-10.0`: a store
# path cannot be missing, so there is no silent fall back to the runtime we were
# trying to replace and no `ExecStartPre test -e` standing in for that.
#
# Why anyone would want it: the non-finite / poisoned-context class (upstream #879)
# is a *runtime* fault, not an engine fault.  On the network's other Strata box —
# cjdell's R9700/gfx1201 host, `cjdell/nixos-config`
# `hosts/zen3-nixos/ai/rocm10-runtime.nix`, which this file is a port of — the SAME
# strata binary and source fired it 2 times in 1 399 requests on ROCm 7.2.3 and 0
# in 1 911 on ROCm 10.0.0 the same day, and the 7.2.1 libraries fire again.  aibox
# runs the same 7.2.x series, so it is exposed to the same class.
#
# What is different here, and why it is opt-in
# --------------------------------------------
# The RDNA2 story is not the RDNA4 story.  Strata's own `docs/AMD_HIP.md` ships its
# Linux `rx6000` package (gfx1030/gfx1031 — this card, via the HSA override in
# ./strata-package.nix) on TheRock **7.13.0** wheels and says of it "the gfx1030
# wheels are unvalidated; a system ROCm 7 is the tested path there", while RDNA2
# gets a ROCm 10.x package only on **Windows**.  So on this card this is an
# experiment, not a port of a proven config: `services.strata.rocm10Runtime`
# (./strata.nix) turns it on and off, ./STRATA.md carries the measured verdict on
# real hardware, and the `$out/RUNTIME-CODE.txt` written below names the
# architectures the tree actually carries — so "does ROCm 10 hold gfx1030 code" is
# a `nix build .#rocm10-runtime && cat` question and never a guess again.
#
# Cost, stated up front
# ---------------------
# 8.05 GB down from Docker Hub, ~16 GB extracted, `allowSubstitutes = false`
# because nothing mirrors a blob this size.  Every box that *builds* this system
# rather than substituting it pays that once — so build aibox here (this is the
# network's builder, common/build-farm.nix) and remember that `just check-build`
# somewhere else is a 24 GB-shaped first run.  This box's `nix.gc` runs
# `--delete-older-than 30d` (common/build-farm.nix), which reclaims the 16 GB again
# the moment the service stops referencing it.
#
# Updating: set `imageDigest` + `layerDigest`/`layerSize` from the new image with
#   skopeo inspect --raw docker://docker.io/rocm/dev-ubuntu-24.04@<digest> \
#     | jq '.layers[] | {size, digest}'
# and re-derive the LAYER digest, never the tag — `10.0.0-full` is mutable, the
# digests are not.
{
  stdenvNoCC,
  runCommand,
  curl,
  cacert,
  gnused,
  gnutar,
  gzip,
}: let
  version = "10.0.0";
  image = "rocm/dev-ubuntu-24.04";
  # The tag this digest was also published as.  Documentation only: the build
  # resolves the digest and never the tag.
  tag = "10.0.0-full";
  imageDigest = "sha256:a90cf047f615abe70fbef83c64def0a2d549ef37a39c8ea545430aba4981b374";
  # The 5th of the image's 5 layers is the ROCm install; the other four are the
  # ubuntu base (30 MB, 7 KB, 1 KB) and a 140 MB apt layer, and the engine dlopens
  # nothing from the distro.  Re-read from the registry's own manifest on
  # 2026-10-28 — and equal to the R9700 box's pin, from a different checkout,
  # which is the cross-check that neither copied a typo:
  #
  #   tok=$(curl -fsSL 'https://auth.docker.io/token?service=registry.docker.io
  #          &scope=repository:rocm/dev-ubuntu-24.04:pull' | sed -n 's/.*"token":"\([^"]*\)".*/\1/p')
  #   curl -sS -H "Authorization: Bearer $tok" -H 'Accept: application/vnd.docker.
  #          distribution.manifest.v2+json' \
  #     https://registry-1.docker.io/v2/rocm/dev-ubuntu-24.04/manifests/$imageDigest | jq .layers
  layerDigest = "sha256:b30f0301857271613fdf668d9658e5c58cb490972158dd9d23e351c1449fe0b0";
  layerSize = 8050797785;
  # Where the runtime lives inside the layer's rootfs tar.  This layer marks
  # opt/rocm opaque with a `.wh..wh..opq` whiteout and then carries exactly ONE
  # real directory, core-10.0/ — which is why the extraction has to come out of
  # THIS layer rather than by following an /opt/rocm symlink.
  inside = "opt/rocm/core-10.0/lib";

  # ---------------------------------------------------------------------------------
  # Stage 1: the image layer, stored verbatim.
  # ---------------------------------------------------------------------------------
  # An OCI layer blob's sha256 IS the digest the manifest publishes, so the hash
  # below is not a value to be discovered by a failed 8 GB build: it is the one the
  # registry attests to, and
  #   nix hash convert --hash-algo sha256 --to sri <layerDigest>
  # produces it.  Fixed output => this derivation may use the network and nothing
  # downstream of it may.
  rocm10Layer = stdenvNoCC.mkDerivation {
    pname = "rocm10-layer";
    inherit version;

    outputHashMode = "flat";
    outputHashAlgo = "sha256";
    outputHash = "sha256-sw8DAYVycWE/32aNlljlxYy0kJchWN2dI+NRwUSf4LA=";

    # 8 GB with no mirror: never try to substitute it, build it once, locally.
    preferLocalBuild = true;
    allowSubstitutes = false;

    dontUnpack = true;
    dontConfigure = true;
    dontFixup = true;

    nativeBuildInputs = [
      curl
      gnused
    ];

    buildPhase = ''
      runHook preBuild
      export HOME="$TMPDIR"
      export SSL_CERT_FILE="${cacert}/etc/ssl/certs/ca-bundle.crt"

      # Docker Hub's blob endpoint wants an anonymous bearer token scoped to the
      # repository.  It expires after ~5 min, which is fine: the registry checks it
      # once, then hands the blob to its CDN for the rest of the transfer.
      tok=$(${curl}/bin/curl -fsSL --retry 8 --retry-delay 5 --retry-all-errors \
              "https://auth.docker.io/token?service=registry.docker.io&scope=repository:${image}:pull" \
            | ${gnused}/bin/sed -n 's/.*"token":"\([^"]*\)".*/\1/p')
      if [ -z "$tok" ]; then
        echo "rocm10-layer: no registry token from auth.docker.io" >&2
        exit 1
      fi

      # `-C -` because this body is 8 GB and --retry without a resume restarts it
      # from zero: a connection that dies at 7.9 GB must not cost another 8.
      ${curl}/bin/curl -fSL --retry 8 --retry-delay 10 --retry-all-errors -C - \
           --retry-max-time 7200 \
           -H "Authorization: Bearer $tok" \
           "https://registry-1.docker.io/v2/${image}/blobs/${layerDigest}" \
           -o "$out"

      # A truncated transfer fails the output hash anyway; checking against the
      # size the manifest publishes names the failure instead.
      size=$(stat -c%s "$out")
      if [ "$size" != "${toString layerSize}" ]; then
        echo "rocm10-layer: got $size bytes, manifest says ${toString layerSize}" >&2
        exit 1
      fi
      runHook postBuild
    '';

    passthru = {
      inherit
        image
        tag
        imageDigest
        layerDigest
        layerSize
        inside
        ;
    };
  };
in
  # -----------------------------------------------------------------------------------
  # Stage 2: filter `lib/` out of the blob.  An ordinary derivation, so re-filtering
  # costs minutes and re-fetches the 8 GB only if the blob has since been garbage
  # collected (nothing roots it but the expression above).
  # -----------------------------------------------------------------------------------
  runCommand "rocm10-runtime-${version}"
  {
    inherit version;

    # `nix eval .#rocm10-runtime.passthru.layer` -> the 8 GB blob this was
    # extracted from, for re-running the filter without hunting for a digest.
    passthru = {
      layer = rocm10Layer;
      inherit
        image
        tag
        imageDigest
        layerDigest
        inside
        ;
    };

    nativeBuildInputs = [
      gnutar
      gzip
    ];

    # tar restores the archive's own mtimes, so the tree is reproducible; stdenv
    # pins the umask, so the modes are too.
  }
  ''
    mkdir -p "$out"
    # --strip-components=3: opt/rocm/core-10.0/lib/<x> -> lib/<x>.
    # ONE pattern, and it has to end in `/*`: GNU tar matches the bare
    # `opt/rocm/core-10.0/lib` unanchored, so it already swallows everything under
    # lib/, and adding the glob as a second pattern then makes tar exit 2 with
    # `…/lib/*: Not found in archive` (measured on the R9700 port: bare alone =
    # 24 452 members, glob alone = 24 451, both = the error).
    tar -xzf ${rocm10Layer} \
        --no-same-owner --no-same-permissions \
        --wildcards --strip-components=3 \
        -C "$out" \
        '${inside}/*'

    # The SONAMEs the engine resolves at start.  If the layer layout ever moves,
    # fail HERE, not at the first GPU request minutes into a model load.
    for l in \
      libamdhip64.so.7 \
      libhsa-runtime64.so.1 \
      libamd_comgr.so.3 \
      libhipblaslt.so.1 \
      libhipblas.so.3 \
      librocsolver.so.0
    do
      if [ ! -e "$out/lib/$l" ]; then
        echo "rocm10-runtime: missing lib/$l after extraction" >&2
        exit 1
      fi
    done
    [ -d "$out/lib/rocm_sysdeps/lib" ] \
      || { echo "rocm10-runtime: missing rocm_sysdeps" >&2; exit 1; }

    # Which GPU code this runtime carries, written INTO the store path.  aibox's
    # card is gfx1035 presented as gfx1030 (./strata-package.nix) and ROCm 10
    # dropped official RDNA2 support, so "is there any gfx103x code in here" is the
    # question that decides whether `lib_dirs` may point at this directory on this
    # box.  Recording it beats asserting a path this file has not measured: the
    # build does not break over a spelling, the answer is one `cat` away forever,
    # and ./STRATA.md carries the reading taken on the real card.
    {
      echo "runtime: ${version} (${image}@${tag})"
      echo "architectures with kernel code under lib/:"
      find "$out/lib" -xdev \( -iname '*gfx*' -o -iname '*.co' -o -iname '*.dat' \) \
        -printf '%f\n' | sed -n 's/.*\(gfx[0-9]\{4\}\).*/\1/p' | sort -u
      echo "files under lib/: $(find "$out/lib" -type f | wc -l)"
    } >"$out/RUNTIME-CODE.txt"
  ''
