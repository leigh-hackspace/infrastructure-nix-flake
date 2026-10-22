{
  config,
  pkgs,
  # `flakeInputs` arrives through specialArgs — flake.nix's `extraArgs` — for the one
  # line that needs it (`flakeInputs.sops-nix.nixosModules.sops` below).  This file was
  # also the inconsistent signature of the four: `flakeInputs: {config, pkgs, ...}: {…}`,
  # a module that was a function returning a module, so anyone copying it got the
  # currying too.
  flakeInputs,
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
    # PARKED modules say so on their own first line (AGENTS.md rule 23), so this list
    # only names them: neither is evaluated, and `nix flake check` cannot see either.
    # ./nvidia.nix
    ./sso.nix
    ./status-dashboard.nix
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
