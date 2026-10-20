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
#     its `LLAMA_CPP_COMMIT` (pinned once below) is unchanged across 0.1.41-0.1.44.1.
#   * `STRATA_PREFILL_MMQ=ON` is HIP-only (CMake FATAL_ERRORs if CUDA is on with
#     it), so CUDA takes the default MMQ path; do not copy that flag from aibox.
#
# The engine source comes from the flake's `strata` input (upstream `main`),
# not a fetchFromGitHub pin: "bleeding edge" is the point.  It is a plain source
# tree, so `finalAttrs.src` is usable directly in the install phase.  (aibox's
# HIP package still pins its own source; the two are deliberately independent.)
{
  lib,
  stdenv,
  fetchFromGitHub,
  cmake,
  pkg-config,
  makeWrapper,
  python3,
  cudaPackages,
  # The upstream Strata source tree (the flake's `strata` input outPath).
  src,
}: let
  # Upstream pins llama.cpp (for ggml + its i-quant block layouts) at this exact
  # revision (setup.py `LLAMA_CPP_COMMIT`); passing it as STRATA_GGML_DIR keeps
  # the build offline and lets Nix pin it. The same source's `gguf-py` is what
  # the one-time pack tools need via STRATA_GGUF_PY (it knows Q2_0 = type 42).
  llama = fetchFromGitHub {
    owner = "ggml-org";
    repo = "llama.cpp";
    rev = "3cf03257f219afbe7334045ff7c6a06ac68c627d";
    hash = "sha256-SRGoXa+4ACBCB3eaG9XFYhMN1i0FyPEy9Rrer+dFGYI=";
  };

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
    # The flake input tracks upstream main; the pinned llama.cpp above is the
    # reproducible part of the build.
    version = "unstable";

    inherit src;

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
      "-DSTRATA_GGML_DIR=${llama}"
    ];

    # Upstream has no install() rules for `strata`; install the binary, the
    # card-check tool, the Python serve layer and the pack tools by hand, plus
    # the pinned llama.cpp's gguf-py so the tools can run against it without
    # hunting the store for the source.
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

      # The runtime Python layer.  `strata_control` and `strata_cli` are not
      # optional extras: 0.1.46 split `apps/cli` into `strata_cli` and moved the
      # engine/config helpers into `strata_control`, which `serve/lifeline.py`
      # imports at module scope — a tree without them starts and dies in a
      # crash-loop (2026-10-20, the first boot on 0.1.46.1).  The guard below is
      # what keeps this list honest from now on.
      cp -r ${finalAttrs.src}/serve ${finalAttrs.src}/strata_control ${finalAttrs.src}/strata_cli \
        ${finalAttrs.src}/tools ${finalAttrs.src}/data $out/share/strata/
      cp -r ${llama}/gguf-py $out/share/strata/gguf-py

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
        --set STRATA_GGUF_PY "$out/share/strata/gguf-py"

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
