# Strata (https://github.com/Niko1221/Strata) for services2's RTX 3060.
#
# Strata is NOT a llama.cpp wrapper: it is its own ggml-based engine for
# Qwen3.8-Flash-Next (a ~58 GB MoE). The hottest experts stay in the GPU's
# VRAM, the rest live in system RAM (or are mapped from the pack), cold ones are
# computed on the CPU pool, the ~27 GiB n-gram (PLE) table is read straight from
# the SSD, and a small MTP draft head guesses tokens the big model verifies in
# one pass. See machines/services2/STRATA.md and docs/services.md.
#
# This is the CUDA sibling of machines/aibox/strata-package.nix (which builds
# the HIP/gfx1030 engine). The differences that matter:
#   * backend CUDA, one arch: GA106 is Ampere, compute capability **sm_86**.
#   * CUDA 13.0 (nixpkgs `cudaPackages_13_0`, via the `cudaSupport` on services2's
#     nixpkgs import). Upstream setup.py's ready-made engine is CUDA 13.0 and
#     its `LLAMA_CPP_COMMIT` (pinned once below) is unchanged across 0.1.41-0.1.52
#     (re-read at the 2026-10-23 and the 2026-10-27 bumps: `setup.py` still names the same
#     revision, `3cf03257f219…`, and so does `third_party/ggml/VERSION.txt` in the new tree,
#     so the ggml / i-quant side of this build is byte-for-byte what 0.1.46 built).
#   * `STRATA_PREFILL_MMQ=ON` was HIP-only (CMake FATAL_ERROR'd if CUDA was on
#     with it). Since 0.2.16 the option is **gone from the tree entirely** — llama.cpp's
#     MMQ prompt kernels left with the MMQ build path (docs/AMD_HIP_PERFORMANCE.md:
#     "Strata 0.2.15 and earlier; that build option was removed in 0.2.16") — so
#     neither package passes it, and Strata's own prompt kernels (`strata_pf_own`,
#     with the int8 prompt-expert kernels default on for the AMD cards) are the path.
#
# The engine source comes from the flake's `strata` input (upstream `main`),
# not a fetchFromGitHub pin: "bleeding edge" is the point.  It is a plain source
# tree, so `finalAttrs.src` is usable directly in the install phase, and since
# 2026-10-28 **both** packages take it (aibox's used to pin its own
# fetchFromGitHub, which is how the two drifted nine releases apart).
#
# **Upstream's 0.2.16 layout move (4058aa782, 2026-11-17) is what `cmakeDir` and
# the copy list below exist for.**  The top level became `app/` (the TS host and
# desktop front ends), `engine/` (the CMake project), `terminal/` (the Python
# layer: `serve/`, `strata_control/`, `strata_cli/`, `tools/`, `data/`), `docs/`
# and `tools/` (CI and release scripts).  Two consequences, read off the tree
# rather than off the commit message:
#   * the CMake project is `engine/`, and it still reaches outside itself —
#     `engine/cmake/strata_gpu_engine.cmake:495` compiles
#     `../terminal/tools/hip/tune_rocblas.cpp` — so the whole input stays the
#     source root and only the *configure* moves into `engine/`.  That is done
#     with the cmake setup hook's `cmakeDir` (its default is `..`, taken from
#     `build/`) and not with `sourceRoot`, because `sourceRoot` is chmodded by
#     stdenv after unpack and a subpath of the read-only store cannot be.
#   * llama.cpp is now **vendored** at the very revision this package used to
#     fetch: `engine/third_party/strata-ggml/UPSTREAM.md` pins `3cf03257f219…`, the
#     same commit `third_party/ggml/VERSION.txt` names, byte-identical to it apart
#     from CMake trims, and it carries gguf-py as `strata-gguf-py`.  So the
#     `fetchFromGitHub` this file carried is gone and `-DSTRATA_GGML_DIR` is not
#     passed: upstream calls that override "kept for two releases, then removed",
#     and taking the vendored tree is one source and one pin instead of two.
#
{
  lib,
  stdenv,
  cmake,
  pkg-config,
  makeWrapper,
  python3,
  cudaPackages,
  # The upstream Strata source tree (the flake's `strata` input; `flake = false`
  # since upstream deleted its root flake.nix, so this is the tree's store path).
  src,
}: let
  # serve/server.py needs jinja2 (chat template) and regex (tokenizer); pillow
  # (WebP image parts) and psutil (Monitor) are optional.
  serverPython = python3.withPackages (
    ps:
      with ps; [
        jinja2
        regex
        pillow
        psutil
      ]
  );

  # The pack/MTP tools additionally need numpy + pyyaml + tqdm (they die with
  # ModuleNotFoundError: No module named 'yaml' otherwise).
  prepPython = python3.withPackages (
    ps:
      with ps; [
        numpy
        pyyaml
        tqdm
        regex
      ]
  );
in
  stdenv.mkDerivation (finalAttrs: {
    pname = "strata";
    # The flake input tracks upstream main, and since 0.2.16 it also carries the
    # ggml this builds against (vendored, pinned in-tree) — so there is no second
    # pin left here to be the reproducible part of the build.
    version = "unstable";

    inherit src;

    # 0.2.16 layout: configure in `engine/` (see the header). The cmake setup hook
    # runs `mkdir -p build; cd build; cmake $cmakeDir`, so this names the source
    # directory as seen from `build/`; the phases' cwd is still the tree root, which
    # is why `install -Dm755 strata` below still finds the binary in `build/`.
    cmakeDir = "../engine";

    nativeBuildInputs = [
      cmake
      pkg-config
      makeWrapper
      # nvcc and the CUDA setup hook, which appends -DCUDAToolkit_ROOT and
      # -DCMAKE_CUDA_HOST_COMPILER to cmakeFlags from the splayed packages below
      # (that is the supported way to point CMake at CUDA without the deprecated
      # `cudatoolkit` merged root).
      cudaPackages.cuda_nvcc
    ];

    # The splayed CUDA 13.0 packages (the marker files they carry are what the
    # cuda setup hook collects into CUDAToolkit_ROOT). cudatoolkit is deprecated
    # in current nixpkgs and warns on evaluation.
    buildInputs = [
      cudaPackages.cuda_cudart
      cudaPackages.libcublas
    ];

    cmakeFlags = [
      "-DSTRATA_ENABLE_CUDA=ON"
      "-DSTRATA_ENABLE_HIP=OFF"
      "-DSTRATA_BUILD_TESTS=OFF"
      "-DCMAKE_BUILD_TYPE=Release"
      # GA106 = Ampere = sm_86 (RTX 3060).
      "-DCMAKE_CUDA_ARCHITECTURES=86"
      "-DCMAKE_CUDA_COMPILER=${cudaPackages.cuda_nvcc}/bin/nvcc"
    ];

    # Upstream has no install() rules for `strata`; install the binary, the
    # card-check tool, the Python serve layer and the pack tools by hand, plus the
    # vendored gguf-py so the tools can run against it without hunting the store.
    installPhase = ''
      runHook preInstall
      mkdir -p $out/bin $out/share/strata
      install -Dm755 strata $out/bin/strata
      # strata-device: the card check (--list-devices / --selftest), the thing to
      # run before trusting the CUDA build. Built whenever CUDA or HIP is on.
      install -Dm755 strata-device $out/bin/strata-device
      # The K-quant prompt library is built next to the engine and opened only
      # for K-quant packs; IQ1_M is an i-quant, so it may not exist here.
      if [ -e libstrata_kq.so ]; then install -Dm755 libstrata_kq.so $out/bin/libstrata_kq.so; fi

      # The runtime Python layer — under `terminal/` since the 0.2.16 layout move
      # (see the header).  `strata_control` and `strata_cli` are not
      # optional extras: 0.1.46 split `apps/cli` into `strata_cli` and moved the
      # engine/config helpers into `strata_control`, which `serve/lifeline.py`
      # imports at module scope — a tree without them starts and dies in a
      # crash-loop (2026-10-20, the first boot on 0.1.46.1).  And `tools/` is now on
      # that layer's path as well, not just the prep tools' directory:
      # `strata_control/gguf_preflight.py:15` imports `tools.gguf_reader`.
      # The guard below is what keeps this list honest from now on.
      cp -r ${finalAttrs.src}/terminal/serve ${finalAttrs.src}/terminal/strata_control \
        ${finalAttrs.src}/terminal/strata_cli ${finalAttrs.src}/terminal/tools \
        ${finalAttrs.src}/terminal/data $out/share/strata/
      # gguf-py for the pack tools, from the vendored copy (same llama.cpp pin as
      # the vendored ggml). `terminal/tools/_paths.py` looks for exactly this shape
      # — `third_party/strata-gguf-py` beside the `tools/` folder, i.e. ENGINE =
      # `$out/share/strata` — so the tools find it with no environment set; the
      # wrapper below names STRATA_GGUF_PY anyway, since that is the documented
      # override and naming it is free.
      mkdir -p $out/share/strata/third_party
      cp -r ${finalAttrs.src}/engine/third_party/strata-gguf-py $out/share/strata/third_party/

      # Contract guard, not a smoke test: import the server with only the
      # installed tree on the path, so a copy list that lags upstream's imports
      # fails the *build* in two seconds instead of crash-looping the service on
      # a box whose model load takes three minutes.  Bytecode off so the check
      # writes nothing into $out.
      PYTHONDONTWRITEBYTECODE=1 PYTHONPATH=$out/share/strata ${serverPython}/bin/python3 -c '
        import serve.server, strata_control.procs, strata_cli.client
      '

      # strata-server: what systemd runs (python -m serve.server).
      makeWrapper ${serverPython}/bin/python3 $out/bin/strata-server \
        --add-flags "-m serve.server" \
        --set PYTHONPATH "$out/share/strata"

      # strata-prep: the one-time model-prep tools (iq_pack.py, mtp_fetch.py,
      # mtp_pack.py, mtp_rt.py), run from their own directory with numpy/pyyaml/
      # tqdm and STRATA_GGUF_PY already pointing at the pinned gguf-py.
      makeWrapper ${prepPython}/bin/python3 $out/bin/strata-prep \
        --chdir "$out/share/strata/tools" \
        --set PYTHONPATH "$out/share/strata/tools" \
        --set STRATA_GGUF_PY "$out/share/strata/third_party/strata-gguf-py"

      runHook postInstall
    '';

    meta = {
      description = "Strata: run Qwen3.8-Flash-Next across GPU VRAM, RAM and CPU (CUDA/sm_86)";
      homepage = "https://github.com/Niko1221/Strata";
      license = lib.licenses.mit;
      platforms = ["x86_64-linux"];
      mainProgram = "strata-server";
    };
  })
