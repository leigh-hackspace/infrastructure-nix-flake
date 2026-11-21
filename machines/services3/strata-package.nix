# Strata (https://github.com/Niko1221/Strata) for services3's GTX 1080 Ti.
#
# Strata is NOT a llama.cpp wrapper: it is its own ggml-based engine for
# Qwen3.8-Flash-Next (a ~58 GB MoE). The hottest experts stay in the GPU's
# VRAM, the rest live in system RAM (or are mapped from the pack), cold ones are
# computed on the CPU pool, the ~27 GiB n-gram (PLE) table is read straight from
# the SSD, and a small MTP draft head guesses tokens the big model verifies in
# one pass. See machines/services2/STRATA.md and docs/services.md.
#
# This is the **sm_61 (Pascal) sibling** of machines/services2/strata-package.nix
# (CUDA 13.0 / sm_86, the RTX 3060) and machines/aibox/strata-package.nix
# (HIP/gfx1030, the Radeon iGPU). Three things are different, and the first is
# the one to read before "fixing" anything here:
#
#   * **CUDA 12.8, not 13.x.** CUDA 13 dropped Pascal and Volta outright: nvcc
#     fails CMake's first compiler test with `compute_70`. GP102 is compute
#     capability 6.1, so this box needs the CUDA 12 branch of the toolkit; it is
#     the same branch upstream ships as its own "CUDA 12 engine" for exactly
#     these cards (`docs/OLDER_GPUS.md`, "NVIDIA: the CUDA 12 engine").
#     nixpkgs has 12_6/12_8/12_9 at this pin; 12_9 is documented to clash with
#     glibc >= 2.43 in the toolkit's math headers (`docs/TROUBLESHOOTING.md`,
#     #601) and this box runs glibc 2.44, so 12_8 is the pick. If the headers
#     ever clash here, 12_6 is the next fallback.
#   * **`STRATA_EXPERIMENTAL_SM60=ON`.** Upstream refuses to build for an
#     architecture below sm_75 unless this is set: it compiles the Pascal/Volta
#     code paths (the pre-sm_75 attention kernel, the Pascal BF16 route, the
#     dp4a replacement for the one intrinsic GP100 lacks) into this build only.
#     The GTX 10 series is on upstream's supported-*experimentally* row of the
#     matrix, "the CUDA 12 engine", which is this build shape.
#   * **One arch: 61.** `sm_61` is GP102/GP104 (GTX 1080 Ti, Tesla P40/P4).
#     `-real` only: there is no second card in this box and no PTX needed for a
#     driver this box owns.
#
# The engine source comes from the flake's `strata` input (upstream `main`),
# shared with services2 and aibox — a second Strata pin is how the two engines
# drifted nine releases apart in October (`docs/pi-agent-models.md` and the
# flake's `strata` input comment), so this file deliberately does not pin its
# own.
#
# Upstream's own recipe for this build (docs/OLDER_GPUS.md, "Building it
# yourself") is the shape below, with the arch narrowed to the one card:
#
#   cmake -S engine -B build-cuda12 -DSTRATA_ENABLE_CUDA=ON -DSTRATA_BUILD_TESTS=OFF \
#         -DSTRATA_EXPERIMENTAL_SM60=ON -DCMAKE_CUDA_ARCHITECTURES="61;70" ...
#
# **Upstream's 0.2.16 layout move (4058aa782, 2026-11-17) is what `cmakeDir` and the
# copy list below exist for**, and it is the same change
# `machines/services2/strata-package.nix` documents at length — read that header once
# and this is that file with a Pascal toolkit. At the locked pin (`3142b66b9`, re-read
# 2026-11-22 against the tree rather than the commit message) the repository root is
# `app/ engine/ terminal/ docs/ tools/`, with **no root `CMakeLists.txt` and no root
# `flake.nix`**, so:
#   * the CMake project is `engine/` — configure there with the cmake hook's `cmakeDir`
#     and not with `sourceRoot` (stdenv chmods `sourceRoot`, and a store subpath cannot
#     be chmodded);
#   * the Python serve layer is `terminal/{serve,strata_control,strata_cli,tools,data}`,
#     and `tools/` is on the *serve* layer's import path too, not only the prep tools';
#   * llama.cpp is **vendored** at `engine/third_party/strata-ggml` — its `UPSTREAM.md`
#     pins the very `3cf03257f219…` this file used to `fetchFromGitHub` — and carries
#     gguf-py as `strata-gguf-py`, so there is no second pin and no `-DSTRATA_GGML_DIR`
#     left here: one source, one pin, like the other two packages.
#
# The Pascal half survived that move, which is what makes this build still correct
# rather than merely inherited: `engine/cmake/backends/cuda.cmake` at this pin still
# FATAL_ERRORs on any arch below 75 unless `STRATA_EXPERIMENTAL_SM60=ON`, still names
# CUDA 13's `nvcc` refusal of `compute_70` as the reason, and `docs/OLDER_GPUS.md`
# still gives the recipe above (`61;70` there is for a mixed Pascal+Volta box; one card
# here). Its `STRATA_DENSE_F32` row still says that route's parity was measured with
# cuBLAS 12.4.5.8 only — which is why `machines/services3/strata.nix` states that
# switch as upstream-measured and unverified on this card.
{
  lib,
  stdenv,
  cmake,
  pkg-config,
  makeWrapper,
  python3,
  cudaPackages,
  # The upstream Strata source tree (the flake's `strata` input; `flake = false` since
  # upstream deleted its root flake.nix, so this is the tree's store path).
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
    version = "unstable-sm61";

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
      # -DCMAKE_CUDA_HOST_COMPILER to cmakeFlags from the splayed packages below.
      cudaPackages.cuda_nvcc
    ];

    buildInputs = [
      cudaPackages.cuda_cudart
      cudaPackages.libcublas
    ];

    cmakeFlags = [
      "-DSTRATA_ENABLE_CUDA=ON"
      "-DSTRATA_ENABLE_HIP=OFF"
      "-DSTRATA_BUILD_TESTS=OFF"
      "-DCMAKE_BUILD_TYPE=Release"
      # GP102 = Pascal = sm_61 (GTX 1080 Ti). One card, one arch.
      "-DCMAKE_CUDA_ARCHITECTURES=61"
      # The Pascal/Volta code paths, and the only thing that lets CMake past its
      # "compute capability 7.5 or newer" refusal.
      "-DSTRATA_EXPERIMENTAL_SM60=ON"
      "-DCMAKE_CUDA_COMPILER=${cudaPackages.cuda_nvcc}/bin/nvcc"
    ];

    # Upstream has no install() rules for `strata`; install the binary, the
    # card-check tool, the Python serve layer and the pack tools by hand, plus
    # the vendored gguf-py — the same list as services2's package, and the import
    # guard below is what keeps it honest.
    installPhase = ''
      runHook preInstall
      mkdir -p $out/bin $out/share/strata
      install -Dm755 strata $out/bin/strata
      # strata-device: the card check (--list-devices / --selftest), the thing to
      # run before trusting this build. It is the only way to prove nvcc really
      # emitted sm_61 code and that the driver will load it.
      install -Dm755 strata-device $out/bin/strata-device
      # The K-quant prompt library is built next to the engine and opened only
      # for K-quant packs; IQ1_M is an i-quant, so it may not exist here.
      if [ -e libstrata_kq.so ]; then install -Dm755 libstrata_kq.so $out/bin/libstrata_kq.so; fi

      # The runtime Python layer — under `terminal/` since the 0.2.16 layout move
      # (see the header).  `strata_control` and `strata_cli` are not
      # optional: `serve/lifeline.py` imports `strata_control` at module scope,
      # so a tree without them starts and dies in a crash-loop (2026-10-20, the
      # first boot on 0.1.46.1).  And `tools/` is on that layer's import path as
      # well, not just the prep tools' directory: `strata_control/gguf_preflight.py:15`
      # imports `tools.gguf_reader`.  The guard below is what keeps this list honest.
      cp -r ${finalAttrs.src}/terminal/serve ${finalAttrs.src}/terminal/strata_control \
        ${finalAttrs.src}/terminal/strata_cli ${finalAttrs.src}/terminal/tools \
        ${finalAttrs.src}/terminal/data $out/share/strata/
      # gguf-py for the pack tools, from the vendored copy (same llama.cpp pin as the
      # vendored ggml). `terminal/tools/_paths.py` looks for exactly this shape —
      # `third_party/strata-gguf-py` beside the `tools/` folder — so the tools find it
      # with no environment set; the wrapper below names STRATA_GGUF_PY anyway, since
      # that is the documented override and naming it is free.
      mkdir -p $out/share/strata/third_party
      cp -r ${finalAttrs.src}/engine/third_party/strata-gguf-py $out/share/strata/third_party/

      # Contract guard, not a smoke test: import the server with only the
      # installed tree on the path, so a copy list that lags upstream's imports
      # fails the *build* in two seconds instead of crash-looping the service on
      # a box whose model load takes minutes.  Bytecode off so the check writes
      # nothing into $out.
      PYTHONDONTWRITEBYTECODE=1 PYTHONPATH=$out/share/strata ${serverPython}/bin/python3 -c '
        import serve.server, strata_control.procs, strata_cli.client
      '

      # strata-server: what systemd runs (python -m serve.server).
      makeWrapper ${serverPython}/bin/python3 $out/bin/strata-server \
        --add-flags "-m serve.server" \
        --set PYTHONPATH "$out/share/strata"

      # strata-prep: the one-time model-prep tools (iq_pack.py, mtp_fetch.py,
      # mtp_pack.py, mtp_rt.py), run from their own directory with numpy/pyyaml/
      # tqdm and STRATA_GGUF_PY already pointing at the vendored gguf-py.
      makeWrapper ${prepPython}/bin/python3 $out/bin/strata-prep \
        --chdir "$out/share/strata/tools" \
        --set PYTHONPATH "$out/share/strata/tools" \
        --set STRATA_GGUF_PY "$out/share/strata/third_party/strata-gguf-py"

      runHook postInstall
    '';

    meta = {
      description = "Strata: run Qwen3.8-Flash-Next across GPU VRAM, RAM and CPU (CUDA 12.8 / sm_61, Pascal)";
      homepage = "https://github.com/Niko1221/Strata";
      license = lib.licenses.mit;
      platforms = ["x86_64-linux"];
      mainProgram = "strata-server";
    };
  })
