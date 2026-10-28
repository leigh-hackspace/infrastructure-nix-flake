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
    ./strata.nix
    ./whisper.nix

    flakeInputs.sops-nix.nixosModules.sops
  ];

  # aibox is the network's build machine (common/build-farm.nix): 6c/12t and 59 GiB
  # of RAM.  It is also, since 2026-10-28, the Strata engine again (./strata.nix),
  # whose expert arena is ~46 GiB of that - which is the trade that made it the
  # builder in the first place, taken the other way on purpose.  What it means in
  # practice: with the engine loaded a build runs in the ~10 GiB left over, so a
  # big-compile pass (a nixpkgs bump, a rust release build) wants `systemctl stop
  # strata` first, and `just check-build` here is a build against a loaded arena.
  # ./STRATA.md has the numbers and `services.strata.expertCache`-sized dials; the
  # apps below (frigate-monitor, immich, whisper, alexandria) are leaf services that
  # the front door proxies directly, so a build storm here costs those rather than the
  # box that runs the rest of the network.
  infra.buildFarm.serveBuilds = true;

  services.strata.enable = true;

  # llama-server (./ai.nix) is displaced by strata at boot - the two cannot be
  # resident at once (llama-server holds ~33 GB of GTT for its model, the IQ2_XS arena
  # is 35.5 GiB).  `systemctl stop strata && systemctl start llama-server` still works
  # for the small models and for the `ai.*` vhosts, which point at its :8081; see
  # ./strata.nix and ./STRATA.md "Operational notes".
}
