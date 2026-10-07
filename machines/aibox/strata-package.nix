{
  lib,
  stdenv,
  fetchFromGitHub,
  cmake,
  pkg-config,
  makeWrapper,
  python3,
  rocmPackages,
  # aibox's GPU is the Radeon 660M iGPU: rocminfo reports gfx1035, which is NOT
  # one of the architectures Strata's cmake accepts (validated gfx1100/gfx1201,
  # community gfx1101/gfx1200, unvalidated gfx1012/gfx1102/gfx1030/gfx1031).
  # gfx1035 is RDNA2 with the same wave32 + 64 KiB LDS + v_dot4_i32_i8 ISA as
  # gfx1030, so the engine is built for gfx1030 and the runtime is told (via
  # HSA_OVERRIDE_GFX_VERSION=10.3.0 in ./strata.nix) to report gfx1030 - the
  # same trick machines/aibox/alexandria-style ROCm setups use for this box.
  hipArch ? "gfx1030",
}:

# Strata (https://github.com/Niko1221/Strata) for aibox's integrated Radeon.
#
# Strata is NOT a llama.cpp wrapper: it is its own ggml-based engine for the
# Qwen3.8-Flash-Next MoE. The hottest experts stay on the GPU, all of them live
# in system RAM, cold ones are computed on the CPU pool, the 28.8 GB n-gram
# (PLE) table is read straight from the SSD, and a small MTP draft head guesses
# tokens the big model verifies in one pass.
#
# On aibox there is no dedicated VRAM: every "VRAM" allocation is GTT, i.e. the
# same system RAM as the expert arena. That is why ./strata.nix sets an explicit
# --expert-cache instead of `auto` (auto sizes itself from hipMemGetInfo, which
# on an iGPU reports the whole free GTT and would fill the machine).
#
# This builds the engine + bundles the Python `serve` layer and the one-time
# pack tools. Model data (GGUF, pack, MTP runtime) is NOT in the store: it is
# prepared once by hand (see ./strata.nix) and named by the JSON run config.
let
  # Upstream pins llama.cpp (for ggml + its i-quant block layouts) at this exact
  # revision; setup.py downloads the same zip. Passing it as STRATA_GGML_DIR
  # keeps the build offline and lets Nix pin it. The same source's `gguf-py` is
  # what the pack tools need via STRATA_GGUF_PY (it knows Q2_0 = type 42).
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
    with ps;
    [
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
    with ps;
    [
      numpy
      pyyaml
      tqdm
      regex
    ]
  );
in
stdenv.mkDerivation (finalAttrs: {
  pname = "strata";
  version = "0.1.40.2";

  src = fetchFromGitHub {
    owner = "Niko1221";
    repo = "Strata";
    rev = "e8ca9afd03d839d4f8dbbe82dffce7f8a3bafd7a";
    hash = "sha256-NCOHJF8L32g67h8S4XY9uOABAKEoqapGqYLUEoiVHME=";
  };

  nativeBuildInputs = [
    cmake
    pkg-config
    makeWrapper
  ];

  # The default rocmPackages scope (all gfx targets, incl. gfx1030 in ROCm
  # 7.2.3) so those derivations hit the binary cache; our own kernels are
  # compiled for hipArch alone below. hipBLASLt ships no gfx1030 kernels, so the
  # dense projections take the plain hipBLAS path (slower prompts, same answers).
  buildInputs = [
    rocmPackages.clr
    rocmPackages.hipblas
    rocmPackages.rocblas
    rocmPackages.hipblaslt
  ];

  cmakeFlags = [
    "-DSTRATA_ENABLE_CUDA=OFF"
    "-DSTRATA_ENABLE_HIP=ON"
    "-DSTRATA_BUILD_TESTS=OFF"
    # ggml's MMQ prompt kernels; setup.py builds every AMD engine with this on.
    "-DSTRATA_PREFILL_MMQ=ON"
    "-DSTRATA_GGML_DIR=${llama}"
    "-DCMAKE_HIP_COMPILER=${rocmPackages.clr.hipClangPath}/clang++"
    "-DCMAKE_HIP_ARCHITECTURES=${hipArch}"
    "-DCMAKE_PREFIX_PATH=${
      lib.makeSearchPath "lib/cmake" [
        rocmPackages.clr
        rocmPackages.hipblas
        rocmPackages.rocblas
        rocmPackages.hipblaslt
      ]
    }"
  ];

  # Upstream has no install() rules for `strata`; install the binary, the Python
  # serve layer and the pack tools by hand, plus the pinned llama.cpp's gguf-py
  # so the tools can run against it without hunting the store for the source.
  installPhase = ''
    runHook preInstall
    mkdir -p $out/bin $out/share/strata
    install -Dm755 strata $out/bin/strata
    # strata-device: the card check (--list-devices / --selftest), the thing to
    # run before downloading 68 GB of model.
    install -Dm755 strata-device $out/bin/strata-device
    cp -r ${finalAttrs.src}/serve ${finalAttrs.src}/tools ${finalAttrs.src}/data $out/share/strata/
    cp -r ${llama}/gguf-py $out/share/strata/gguf-py

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

  # The HIP device check (src/core/device.cu) compares the card's gcnArchName
  # with this list, so the runtime must report one of them.
  passthru = {
    inherit hipArch llama;
  };

  meta = {
    description = "Strata: run Qwen3.8-Flash-Next across GPU, RAM and CPU (HIP/${hipArch})";
    homepage = "https://github.com/Niko1221/Strata";
    license = lib.licenses.mit;
    platforms = [ "x86_64-linux" ];
    mainProgram = "strata-server";
  };
})
