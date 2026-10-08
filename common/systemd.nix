# Shared systemd unit helpers.
#
# Handed to every module through the flake's `specialArgs` as `INFRA` (see
# flake.nix), so the house "never give up" restart policy has one definition
# instead of a hand-copied pair of lines — and a comment explaining why — in
# every unit file.
{lib}: {
  # Wrap a systemd.services.<name> definition written in this repo with the
  # "never give up" policy: Restart=always plus startLimitIntervalSec=0.
  #
  # The two halves live in different places and that is the whole trap:
  # Restart/RestartSec belong in `serviceConfig`, the rate-limit knobs on the
  # unit itself (`systemd.services.<name>.startLimitIntervalSec`).  Writing only
  # the first half — which is what six units here did — leaves systemd's default
  # start rate limit (5 starts in 10 s) in force, so the unit is permanently
  # stopped after a handful of quick failures: exactly what happens to anything
  # still waiting for the NAS to come back after a power cut.
  #
  #   systemd.services.foo = INFRA.mkNeverGiveUp {
  #     description = "...";
  #     serviceConfig.ExecStart = "...";
  #   };
  #
  # Any Restart/RestartSec in the definition is replaced, so call sites must not
  # set them.
  mkNeverGiveUp = def:
    def
    // {
      startLimitIntervalSec = 0;
      serviceConfig =
        (def.serviceConfig or {})
        // {
          Restart = "always";
          RestartSec = def.serviceConfig.RestartSec or "5s";
        };
    };

  # The same policy applied to a unit defined by a nixpkgs module
  # (oci-containers' podman-* units, nginx, ...), where the module already chose
  # a Restart policy — and sometimes a rate limit — that has to be beaten,
  # hence mkForce on all three (nginx's own module sets startLimitIntervalSec=60,
  # which conflicts at plain priority).
  #
  #   systemd.services = lib.mapAttrs' ... (name: _:
  #     lib.nameValuePair "podman-${name}" INFRA.mkNeverGiveUpOverride);
  mkNeverGiveUpOverride = {
    startLimitIntervalSec = lib.mkForce 0;
    serviceConfig = {
      Restart = lib.mkForce "always";
      RestartSec = lib.mkForce "5s";
    };
  };
}
