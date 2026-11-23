{
  # `flakeInputs` arrives through specialArgs — flake.nix's `extraArgs` — for the one
  # line that needs it (`flakeInputs.sops-nix.nixosModules.sops` below: a dashed
  # attribute name cannot be a function-argument pattern, which is why services1's and
  # services2's default.nix carry the same comment).
  flakeInputs,
  ...
}: {
  networking.hostName = "services3";

  # The engine: **Strata** (CUDA 12.8 / sm_61) serving **Qwen3.8-Flash-Next Coder
  # IQ1_M** on the GTX 1080 Ti, reached from the front door as
  # `strata3.int.leighhack.org` since 2026-11-21. ./strata.nix carries the sizing, the
  # pack paths and the VRAM budget; ./strata-package.nix carries the CUDA-12 build and
  # why this card cannot use the same toolkit as services2.
  #
  # **./llama.nix is PARKED** (its own first line is the notice): Strata replaced it on
  # 2026-11-21, because one GPU cannot serve two engines and this box's job is now the
  # 58 GB MoE rather than a 9B dense model. Its measurements are the record of what this
  # card did before, and un-parking it means un-parking it *instead of* ./strata.nix.
  #
  # This box builds for itself as of 2026-11-23; before that it delegated to aibox, and
  # the two facts that made delegation attractive here are still true: the box is 8
  # Skylake cores, and its uplink is a Wi-Fi dongle, so a local compile is work done by
  # the machine that is also carrying its own ssh — and a CUDA engine bump (./strata.nix,
  # `machines/services3/strata-package.nix`) is exactly the kind of compile that would
  # take the afternoon.  So the honest rule for this box: a bump of the `strata` input
  # lands here last, in a window, and `nix flake update strata` is a network-wide act
  # whose cost is now paid per box rather than once on a builder (AGENTS.md, the Strata
  # row).  What delegation was hiding is that `just switch` here always built the things
  # that changed; it just did it over the hop.

  imports = [
    ./configuration.nix
    ./hardware-configuration.nix
    ./networking.nix
    # ./nvidia.nix is still what makes the card usable at all — for Strata it is the
    # CUDA driver + libcuda, not a Vulkan ICD, but it is the same `legacy_580` branch
    # (580 is the last NVIDIA branch that supports Pascal) and the same
    # `hardware.graphics.enable` that puts /run/opengl-driver/lib on the engine's
    # library path.
    ./nvidia.nix
    # ./llama.nix is PARKED — see the comment above and the file's first line.
    # ./llama.nix
    ./strata.nix
    ./users.nix

    # Refuse to start llama-server while the loaded NVIDIA kernel module and the driver
    # this system was built for disagree — the state a `nixpkgs` bump leaves between
    # `switch` and reboot, which is what took `llama.int` down on 2026-11-15 while every
    # unit looked healthy. ./llama.nix turns it on for the unit.
    ../../common/gpu-driver-match.nix

    # Measured like every other box, by services1's Prometheus:
    # machines/services1/services/observability.nix has the three scrape rows for this
    # box. They are not *scraping* yet — they land when services1 next switches, which is
    # a deploy decision about that box and its six app units, not a fee for joining; see
    # ./README.md "Open items" and docs/follow-ups.md.
    ../../common/exporters.nix

    # pi's endpoint list (docs/pi-agent-models.md) and, through it, web search
    # (docs/pi-web-search.md). This file used to predict that the sync would print
    # `SKIP: /home/leigh-admin/.pi/agent does not exist` because pi was not installed
    # here; measured 2026-11-16 it is installed, the sync writes all three files, and
    # Brave answers 200 from this box's USB-dongle uplink. The SKIP path is still the
    # right design for a box where pi is genuinely absent — it is a message rather than a
    # silence, so the next person to install pi gets the config without setting anything
    # up.
    ../../common/pi-agent.nix

    flakeInputs.sops-nix.nixosModules.sops
  ];

  # `./vlan225.nix` is **not** in this list and must not be added on the bench — it is
  # the wired VLAN 225 wiring this box wants once it is racked, and enabling it here
  # would leave the box with no address at all, because the network's Wi-Fi hands out
  # VLAN 226.  Its own header is the parking notice (AGENTS.md rule 23);
  # ./README.md owns the move.

  programs.pi-agent.enable = true;

  # The one service this box runs. Nothing else competes for the card or the RAM:
  # the three Prometheus exporters are text-collectors on :9100/:9558/:9633, and
  # `programs.pi-agent` is config plus a timer.
  services.strata3.enable = true;
}
