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
#   cmake -S . -B build-cuda12 -DSTRATA_ENABLE_CUDA=ON -DSTRATA_BUILD_TESTS=OFF \
#         -DSTRATA_EXPERIMENTAL_SM60=ON -DCMAKE_CUDA_ARCHITECTURES="61;70" ...
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
  #
  # Re-read at this bump: `grep -n 'LLAMA_CPP_COMMIT = ' $src/setup.py` — the
  # same revision services2's package names, because both build from one input.
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
    version = "unstable-sm61";

    inherit src;

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
      "-DSTRATA_GGML_DIR=${llama}"
    ];

    # Upstream has no install() rules for `strata`; install the binary, the
    # card-check tool, the Python serve layer and the pack tools by hand, plus
    # the pinned llama.cpp's gguf-py — the same list as services2's package, and
    # the import guard below is what keeps it honest.
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

      # The runtime Python layer.  `strata_control` and `strata_cli` are not
      # optional: `serve/lifeline.py` imports `strata_control` at module scope,
      # so a tree without them starts and dies in a crash-loop.
      cp -r ${finalAttrs.src}/serve ${finalAttrs.src}/strata_control ${finalAttrs.src}/strata_cli \
        ${finalAttrs.src}/tools ${finalAttrs.src}/data $out/share/strata/
      cp -r ${llama}/gguf-py $out/share/strata/gguf-py

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
      # tqdm and STRATA_GGUF_PY already pointing at the pinned gguf-py.
      makeWrapper ${prepPython}/bin/python3 $out/bin/strata-prep \
        --chdir "$out/share/strata/tools" \
        --set PYTHONPATH "$out/share/strata/tools" \
        --set STRATA_GGUF_PY "$out/share/strata/gguf-py"

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
