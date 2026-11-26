# strata-vision: the picture reader (mmproj) of Strata's multimodal path, built for
# services2 and deliberately **without CUDA**.
#
# Why this is its own derivation and not a second `cmake` inside
# `./strata-package.nix`: the engine's build is a CUDA/sm_86 compile of the whole
# ggml + Strata kernels (tens of minutes, and it would be dirtied by every change
# here), while the reader is a separate, small C++17 program.  Since upstream's
# own-stack step 6 (0.2.16) `strata-vision` is Strata's own Qwen vision tower
# (`terminal/tools/vision/own`) — it links **no llama.cpp, no mtmd and no ggml** —
# so a CPU build needs nothing but a compiler, and its `--gpu` half (cuBLAS) is
# exactly what this box cannot afford.  Keeping the two derivations apart is what
# makes `nix build .#strata-vision` an honest, quick test of the reader alone.
#
# The CPU reader is upstream's own default for a card under 24 GB of VRAM
# (`terminal/strata_control/vision_reader.py`, GPU_MIN_VRAM_GB = 23.5): the GPU
# reader keeps 1-1.4 GB of VRAM that the experts would use (#2406), and on a
# 12 GB RTX 3060 running a ~22 GiB resident arena that trade is not available.
# Owner's documented cost (docs/DETAILS.md): "About 3 s for a 300-token picture
# on 8 cores; more tokens take longer in proportion."  Nothing here is fast — the
# point is that it runs, on demand, without touching the card.
#
# The file it needs at run time is the model's projector (mmproj), which is
# **model data and not in the store** — see `./strata.nix`'s header for the
# download and its pinned size/sha256.  This package is only the program.
{
  lib,
  stdenv,
  cmake,
  # The flake's `strata` input (the source tree, not a flake); the same path
  # ./strata-package.nix builds the engine from, so the two cannot drift.
  src,
}:
stdenv.mkDerivation (finalAttrs: {
  pname = "strata-vision";
  version = "unstable";

  inherit src;

  # 0.2.16 layout: the reader is `terminal/tools/vision` (the engine's CMake
  # project is `engine/`, and the top-level `tools/` is CI/release scripts).  The
  # cmake setup hook creates `build/` at the source root and configures from
  # there, so this is the source directory as seen from `build/`.
  cmakeDir = "../terminal/tools/vision";

  nativeBuildInputs = [cmake];

  cmakeFlags = [
    "-DCMAKE_BUILD_TYPE=Release"
    # Upstream's default: AVX2 baseline with the kernel tier chosen at run time
    # (`own/cpu_caps.h`), so the flag only decides the generic code's floor.
    # `STRATA_PORTABLE=OFF` is what setup.py passes when it builds for the PC it
    # runs on (`-march=native`), and a store path built that way is a trap here:
    # it substitutes to another box and dies on an illegal instruction (#411/#412).
    "-DSTRATA_PORTABLE=ON"
    # The card's reader is a different program (`strata-vision-hip`/`-sycl`) or
    # the same one with `--gpu`; this box wants neither, and the CMake says CUDA
    # and HIP are mutually exclusive anyway.
    "-DSTRATA_VISION_CUDA=OFF"
    "-DSTRATA_VISION_HIP=OFF"
    "-DSTRATA_VISION_SYCL=OFF"
    # Test oracles pull llama.cpp's mtmd back in; no package ships them.
    "-DSTRATA_VISION_ORACLES=OFF"
  ];

  # Upstream has no install() rule for the reader (nor for the engine): the
  # target writes `bin/strata-vision` under the build directory.
  installPhase = ''
    runHook preInstall
    install -Dm755 bin/strata-vision $out/bin/strata-vision
    runHook postInstall
  '';

  meta = {
    description = "Strata's picture reader (Qwen vision tower + mmproj), CPU build";
    homepage = "https://github.com/Niko1221/Strata";
    license = lib.licenses.mit;
    platforms = ["x86_64-linux"];
    mainProgram = "strata-vision";
  };
})
