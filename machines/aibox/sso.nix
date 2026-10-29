# MODULE — aibox's POSIX identity: authentik users and groups as Unix accounts.
#
# The whole mechanism lives in `common/auth-posix.nix` — the sssd domain, the CA it trusts,
# the sudo rule, the ordering — because the Pi Room netboot image needs the identical thing
# and the two hand-written copies had already drifted.  **Read that module's header before
# changing anything here**: it carries what the uid/gid numbers actually are (they are
# computed from database primary keys and stored nowhere), what `id`/`getent` cannot tell
# you on this box (nscd and sssd both cache in front of the directory), and why the bind
# secret below is public by construction.
#
# `sso.nix` is still the file named in `./default.nix`; what used to be in it — a second
# copy of the root CA PEM, a second sssd domain (with `cache_credentials = true` where this
# box had `false`, and `debug_level = 8` where the image had 6), and a second copy of the
# sudo rule — is now one module.
{
  imports = [../../common/auth-posix.nix];

  infra.authPosix = {
    enable = true;

    # Public to anyone who can read this store path, and to anyone who can PXE-boot a Pi
    # Room client.  Rotating it is a git commit today; the plan to make it unnecessary to
    # ship at all is `docs/auth-posix-logins.md`.  It is also a *real authentik account*
    # (pk 70, uid 2070), so every LDAP client in the fleet binds as one member of staff —
    # which is why nothing here may experiment with deliberately-wrong binds.
    bindToken = "pgina";
  };

  # Keep local users working even when the directory is not reachable.  This is the reason
  # `access_provider` is still `permit` rather than a group gate: `leigh-admin`
  # (common/users.nix) is local and key-injected, so administrative access never depends on
  # authentik — but an interactive sssd login would, and that has no test harness yet.
  users.mutableUsers = true;
}
