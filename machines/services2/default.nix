{
  # `flakeInputs` arrives through specialArgs — flake.nix's `extraArgs` — for the one
  # line that needs it (`flakeInputs.sops-nix.nixosModules.sops` below: a dashed
  # attribute name cannot be a function-argument pattern, which is why this file used
  # to be `flakeInputs: {…}` applied at flake.nix's call site).
  flakeInputs,
  ...
}: {
  networking.hostName = "services2"; # Define your hostname.

  # services2 has built only for itself since 2026-10-17, when aibox took the network's
  # builds; aibox gave that role up on 2026-11-23 (common/nix-gc.nix's header is the why),
  # so this box is back to being a plain client of nothing.  The reason it stopped is in
  # this box's STRATA.md and it still holds — its 30 GiB of RAM is the Coder engine's, and
  # a build competing with the engine was the box's worst failure mode.  That is also why
  # *this* box, despite having the most cores on the network (16), is not where
  # `just check-build` belongs: the gate is memory-bound, and the biggest memory on the
  # network is aibox's.  It keeps its 211 GB of free store.

  imports = [
    ./sops.nix

    # What this box runs: its own status dashboard and (since the RTX 3060 arrived)
    # the network's only live Strata engine.  The rest of what it used to run —
    # gatus, Prometheus and Grafana — moved to services1 on 2026-10-17, because
    # this box's 30 GB of RAM is strata's: the engine sits at ~28 GiB RSS against
    # those services and the kernel reaps process after process under that
    # pressure.  See ./README.md for what still runs here and what is left to
    # measure this box, and ./STRATA.md for the engine.
    ./status.nix
    ./strata.nix

    ./configuration.nix
    ./hardware-configuration.nix
    # NVIDIA driver for the RTX 3060 that runs Strata's CUDA engine.
    ./nvidia.nix
    ./networking.nix

    # Refuse to start strata while the loaded NVIDIA kernel module and the driver this
    # system was built for disagree — which is exactly the state a `nixpkgs` bump leaves
    # the box in between `switch` and the reboot. ./STRATA.md's receipts and
    # docs/work-log-2026-11.md have the measurement; ./strata.nix turns it on for the unit.
    ../../common/gpu-driver-match.nix

    # Metrics exporters — the per-box half of monitoring.  This box is still
    # measured, now by services1's Prometheus (machines/services1/services/
    # observability.nix), which scrapes 10.3.1.21 over the LAN.
    ../../common/exporters.nix

    # `programs.pi-agent` (docs/pi-agent-models.md): this box is both a client of the
    # fleet's endpoints and one of them (`strata-services2` in
    # common/pi-endpoints.nix), so its own engine being stopped is the ordinary case —
    # the timer keeps the model list it last saw and writes nothing else.
    ../../common/pi-agent.nix

    flakeInputs.sops-nix.nixosModules.sops
  ];

  # The Qwen3.8-Flash-Next Coder engine on the RTX 3060; see ./strata.nix for
  # the one-time model prep and the memory sizing.
  services.strata.enable = true;

  programs.pi-agent.enable = true;
}
