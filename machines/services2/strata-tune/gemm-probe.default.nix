# NOT part of the flake, on purpose.  How to build the probe beside this file:
#
#   cp gemm-probe.cu gemm-probe.default.nix /tmp/probe/   # default.nix reads ./probe.cu
#   NIXPKGS_ALLOW_UNFREE=1 nix build --impure -I nixpkgs=flake:nixpkgs -f /tmp/probe/default.nix -o /tmp/probe-link
#   LD_LIBRARY_PATH=/run/opengl-driver/lib /tmp/probe-link/bin/probe 4096 2 15
#
# It is not in `flake.nix` because callCuda comes from an unfree-marked package set and
# nothing should have to build a CUDA binary to evaluate this repo (AGENTS.md,
# "Evaluation must not build anything").  What it measures and what it said on 2026-11-14
# (26 TFLOP/s of bf16 at 95 W on the RTX 3060) is in ../STRATA-TUNE-2026-11-14.md.
# Its one loose end is a print bug: `cublas …:84 code 0` once per GEMM, where 0 is
# success - see the doc's last paragraph for why that does not touch the rate.
# Throwaway: the cuBLAS GEMM probe whose number docs/follow-ups.md says nobody had run.
# Not part of the flake - it lives in /tmp, built against the same CUDA set
# machines/services2/strata-package.nix uses (backendStdenv + the splayed packages).
#
#   nix build --impure -I nixpkgs=flake:nixpkgs -f /tmp/probe/default.nix -o /probe
let
  pkgs = import <nixpkgs> {
    config = {
      allowUnfree = true;
      cudaSupport = true;
    };
  };
  cudaPackages = pkgs.cudaPackages_13_0;
in
  cudaPackages.backendStdenv.mkDerivation {
    name = "gemm-probe";
    src = ./.;
    nativeBuildInputs = [cudaPackages.cuda_nvcc];
    buildInputs = [cudaPackages.cuda_cudart cudaPackages.libcublas];
    buildPhase = ''
      runHook preBuild
      nvcc -O3 -arch=sm_86 -o probe $src/probe.cu -lcublas
      runHook postBuild
    '';
    installPhase = ''
      runHook preInstall
      mkdir -p $out/bin
      install -m755 probe $out/bin/probe
      runHook postInstall
    '';
    dontStrip = true;
  }
