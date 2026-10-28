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
  # The upstream Strata source tree (the flake's `strata` input outPath), passed
  # in by flake.nix and by ./strata.nix.  Since 2026-10-28 this is the SAME source
  # services2's CUDA engine builds from, so `nix flake update strata` moves both
  # engines; before that aibox pinned its own fetchFromGitHub and the two drifted
  # nine releases apart.  Plain source, so `finalAttrs.src` is usable in the
  # install phase.
  src,
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
  # (WebP image parts) and psutil (Monitor) are optional.  No new REQUIRED import
  # at 0.1.52: jsonschema/referencing stay a lazy optional import in
  # serve/structured.py (absent -> "one JSON object" checking only), and
  # textual/requests/tqdm in requirements.txt are the app/Terminal, not the serve
  # layer - verified by the import guard in the install phase rather than by trust.
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
    # The input floats on upstream `main`; the pinned llama.cpp above is the
    # reproducible part of the build.  (0.1.52 was `CMakeLists.txt`'s project
    # version at the rev this was written against - the number lives in the tree
    # now, not here, which is the point of taking the input.)
    version = "unstable";

    inherit src;

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
      # strata-alias-check: upstream's tests/hip/mapped_alias.cpp, the check that a
      # pinned host allocation's DEVICE ALIAS works on this stack. That is the gate
      # for `--pcie-mode direct` (see ../strata-uma-2026-10-08.md): the PCIe share is
      # delivered as device_alias(layer, expert), and if the alias is refused
      # pcie_layer() is false and --pcie-frac is inert. Upstream builds the HIP test
      # targets whenever HIP is on (they are not behind STRATA_BUILD_TESTS), so this
      # is one install line - run it with ./strata-tune/alias-check.sh.
      install -Dm755 hip_mapped_alias $out/bin/strata-alias-check
      # The K-quant prompt library is built next to the engine and opened only for
      # K-quant packs (moe_mmq.cu); IQ2_XS is an i-quant, so it may not exist here.
      if [ -e libstrata_kq.so ]; then install -Dm755 libstrata_kq.so $out/bin/libstrata_kq.so; fi

      # strata_control/ and strata_cli/ are NOT optional extras from 0.1.46 on:
      # serve/lifeline.py imports `strata_control.procs` at module scope and
      # serve/server.py reaches engine_component/tls/hipblaslt_tuning/vision_reader
      # /draft_layer/drafting/user_args through it, and strata_control reaches
      # strata_cli.  A tree that ships only {serve,tools,data} starts and dies in a
      # ModuleNotFoundError crash-loop - which is how the same bump first failed on
      # services2 (2026-10-20).  Both are stdlib-only on that path.
      cp -r ${finalAttrs.src}/serve ${finalAttrs.src}/strata_control ${finalAttrs.src}/strata_cli \
        ${finalAttrs.src}/tools ${finalAttrs.src}/data $out/share/strata/
      cp -r ${llama}/gguf-py $out/share/strata/gguf-py

      # Contract guard, not a smoke test: import the server with only the installed
      # tree on the path, so a copy list that lags upstream's imports fails the
      # *build* in two seconds instead of crash-looping a service whose model load
      # takes minutes.  Bytecode off so the check writes nothing into $out.
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

    # The HIP device check (src/core/device.cu) compares the card's gcnArchName
    # with this list, so the runtime must report one of them.
    passthru = {
      inherit hipArch llama;
    };

    meta = {
      description = "Strata: run Qwen3.8-Flash-Next across GPU, RAM and CPU (HIP/${hipArch})";
      homepage = "https://github.com/Niko1221/Strata";
      license = lib.licenses.mit;
      platforms = ["x86_64-linux"];
      mainProgram = "strata-server";
    };
  })
