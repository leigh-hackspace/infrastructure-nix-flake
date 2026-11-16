{
  # `flakeInputs` arrives through specialArgs — flake.nix's `extraArgs` — for the one
  # line that needs it (`flakeInputs.sops-nix.nixosModules.sops` below: a dashed
  # attribute name cannot be a function-argument pattern, which is why services1's and
  # services2's default.nix carry the same comment).
  flakeInputs,
  ...
}: {
  networking.hostName = "services3";

  # The engine: llama.cpp (Vulkan) on the GTX 1080 Ti, serving **Ternary-Bonsai-2-9B-Q2_0
  # and its vision projector** since 2026-11-12 (LFM3-1.7B before that), reached from
  # the front door as `llama.int.leighhack.org`.  ./nvidia.nix is what makes the card a
  # Vulkan device at all, and ./llama.nix carries the sizing, the model path and the
  # reachability argument.
  #
  # This box delegates its builds to aibox like every box that is not the builder
  # (common/build-farm.nix).  Here it matters more than on the others for two reasons:
  # the box is 8 Skylake cores against aibox's 6c/12t Zen 3+, and its uplink is a
  # Wi-Fi dongle, so a local compile is work done by the machine that is also carrying
  # its own ssh — and llama.cpp's Vulkan backend is exactly the kind of compile (SPIR-V
  # shaders) that would take the afternoon.  Nix prefers the external builder and falls
  # back to local when aibox is unreachable, which is what keeps a deploy of this box
  # possible while the dongle is the only path to it.
  infra.buildFarm.builderHost = (import ../../common/hosts.nix).aibox;
  infra.buildFarm.maxJobs = 6;

  imports = [
    ./configuration.nix
    ./hardware-configuration.nix
    ./networking.nix
    ./nvidia.nix
    ./llama.nix
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
}
