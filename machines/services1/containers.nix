{
  config,
  lib,
  pkgs,
  modulesPath,
  ...
}:

let
  CONFIG = import ./config.nix;
in
{
  system.updateContainers = {
    enable = true;
    webhookUrl = lib.strings.trim (builtins.readFile (config.sopsSecretText "slack_url"));
  };

  virtualisation.podman = {
    enable = true;
    autoPrune.enable = true;
    dockerCompat = true;
    dockerSocket.enable = true;
    defaultNetwork.settings.dns_enabled = true;
  };

  virtualisation.oci-containers.backend = "podman";

  # OCI container services must never give up.  The default Restart policy
  # from `oci-containers` is "on-failure", and systemd's start rate-limit
  # (5 starts in 10s) permanently stops a unit after a handful of quick
  # failures — e.g. a container that keeps failing while waiting for the NAS
  # to come back after a power cut.  Force Restart=always and disable the
  # rate limit so containers retry forever.
  systemd.services = lib.mapAttrs' (name: _: lib.nameValuePair "podman-${name}" {
    startLimitIntervalSec = 0;
    serviceConfig = {
      Restart = lib.mkForce "always";
      RestartSec = "5s";
    };
  }) config.virtualisation.oci-containers.containers;
}
