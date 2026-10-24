# SHIM — the gate's config moved to `common/nginx-sso-config.nix` in phase 2 of
# `docs/router-front-door-migration.md`; this file only fixes the listener to what services1
# still runs it on, and it goes away with `systemd.services.nginx-sso` in
# machines/services1/http.nix (phase 3 step 5). Not a PARKED module — this one is
# evaluated and deployed, which is why `grep -rl '^# PARKED' machines/` must not list it.
#
# Why `0.0.0.0` is still passed here rather than being fixed in place: the gate answers the
# nginx on its own box, and until the front door flips that nginx is reached by clients from
# every interface. Loopback would break nothing today either, but the change belongs with the
# move — one box's listener, one change, and the window where two gates exist stays short
# (see the finding about `*:8082` answering a bare curl in the migration doc).
{
  lib,
  sopsSecretText,
}: let
  common = import ../../../common/nginx-sso-config.nix;
in
  common {
    inherit lib sopsSecretText;
    listenAddr = "0.0.0.0";
  }
