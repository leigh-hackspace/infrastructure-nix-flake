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
    # pi's endpoint list (docs/pi-agent-models.md). This box is `strata` in
    # common/pi-endpoints.nix: the engine it runs is dialled through the front door
    # like everywhere else, so aibox asks gw a question rather than reading its own
    # key out of its own config.
    ../../common/pi-agent.nix
    ./netboot.nix
    ./networking.nix
    # aibox is the far end of the router's declared relay path (./wan-relay.nix):
    # the phone that cannot enumerate on gw's EHCI does on this box's xHCI, and
    # this is the half that forwards the router's packets out its USB tether.
    ./wan-relay.nix
    # PARKED modules say so on their own first line (AGENTS.md rule 23), so this list
    # only names them: none is evaluated, and `nix flake check` cannot see one. The
    # nvidia module for the removed GTX 1060 was deleted here on 2026-11-18 rather
    # than left parked (`git log --diff-filter=D -- machines/aibox/nvidia.nix`).
    ./sso.nix
    ./status-dashboard.nix
    ./strata.nix
    ./whisper.nix

    flakeInputs.sops-nix.nixosModules.sops
  ];

  # aibox is **not** the network's build machine any more — it was, from 2026-10-17 to
  # 2026-11-23, and the whole reason it was that box (59 GiB of RAM, 6c/12t) is the reason
  # it stopped: those 59 GiB are the Strata engine's, whose expert arena alone is 33-39
  # GiB.  With the arena loaded, delegating a build here meant the engine either paid with
  # latency (the driver pauses all GPU work while Linux moves memory the GPU uses: 354 s of
  # it in one engine run) or died (the kernel OOM-killed it twice on 2026-11-21).  Both
  # halves of that are now settled: nothing delegates here (common/nix-gc.nix's header for
  # the withdrawal and common/ssh-hop.nix's footer for what re-enabling it would need), and
  # what this box compiles for itself is bounded in ./configuration.nix and protected in
  # ./strata.nix.  ./STRATA.md has the numbers, the two death receipts and the dials.
  # The apps below (frigate-monitor, immich, whisper, alexandria) are leaf services that
  # the front door proxies directly.

  services.strata.enable = true;

  programs.pi-agent.enable = true;

  # llama-server (./ai.nix) is displaced by strata at boot - the two cannot be
  # resident at once (llama-server holds ~33 GB of GTT for its model, the IQ2_XS arena
  # is 35.5 GiB).  `systemctl stop strata && systemctl start llama-server` still works
  # for the small models and for the `ai.*` vhosts, which point at its :8081; see
  # ./strata.nix and ./STRATA.md "Operational notes".
}
