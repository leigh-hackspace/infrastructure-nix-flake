flakeInputs: {
  config,
  pkgs,
  ...
}: {
  networking.hostName = "aibox"; # Define your hostname.

  imports = [
    ./ai.nix
    ./alexandria.nix
    ./configuration.nix
    ./frigate-monitor.nix
    ./hardware-configuration.nix
    ./immich.nix
    ../../common/exporters.nix
    ./netboot.nix
    ./networking.nix
    # PARKED (see docs/repo-audit-2026-10-07.md §1.4): the GTX 1060 was removed
    # from this box on 2026-09-02, so ./nvidia.nix is kept for reference only.
    # ./nvidia.nix
    ./sso.nix
    ./status-dashboard.nix
    # PARKED (2026-10-17): strata was disabled to make this box the network's
    # builder (common/build-farm.nix) — its ~33 GiB expert arena is memory a build
    # cannot have.  The module, ./STRATA.md and the `.#strata` package stay so the
    # engine can be rebuilt and its measurements reproduced; to bring it back,
    # uncomment this import and add `services.strata.enable = true;` below.  The
    # model data under ~/Models and ~/Strata is not removed by that edit (see
    # docs/follow-ups.md).
    # ./strata.nix
    ./whisper.nix

    flakeInputs.sops-nix.nixosModules.sops
  ];

  # aibox is the network's build machine (common/build-farm.nix): 6c/12t, 59 GiB
  # of RAM and no engine holding it since strata was parked.  Its apps are leaf
  # services that services1 proxies, so a build storm here costs frigate/immich/
  # whisper rather than the box that runs the rest of the network.
  infra.buildFarm.serveBuilds = true;

  # With strata gone, llama-server (./ai.nix) gets its default wantedBy back and
  # starts at boot again.  It holds its model in GTT once a model is loaded, so
  # stop it before a long build if memory is tight — it is not displaced by the
  # flake any more.
}
