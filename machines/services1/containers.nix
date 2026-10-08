# Nightly container-image update (nixos-utils' containers module, imported for
# both machines in flake.nix; the timer and the update script come from there).
#
# Podman itself, the oci-containers backend and the "never give up" restart
# policy for container units live in common/containers.nix.
{ config, lib, ... }:
{
  system.updateContainers = {
    enable = true;
    webhookUrl = lib.strings.trim (builtins.readFile (config.sopsSecretText "slack_url"));
  };
}
