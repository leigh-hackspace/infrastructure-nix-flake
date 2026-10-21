# nixbld.nix
#
# Keep the `nixbld` group a pool of throwaway build UIDs and nothing else.
#
# `nixbld` is not a permissions group. In nix 2.34 (src/libstore/unix/user-lock.cc)
# `useBuildUsers()` is `(build-users-group != "") && isRootUser()`, and
# `SimpleUserLock::acquire()` then treats **every member of that group** as a
# candidate UID to run the build under, taking the first one whose lockfile under
# /nix/var/nix/userpool is free — in /etc/group order. Before the build starts and
# again when it ends, `DerivationBuilderImpl::killSandbox()` calls
# `killUser(buildUser->getUID())`, which is `setuid(uid); kill(-1, SIGKILL)`
# (src/libutil/unix/processes.cc): every process owned by that UID is destroyed.
# Not "when a build fails" — `prepareUser()` runs it at the start of every build.
#
# So membership is a target, not a grant. On 2026-10-11 `common/build-farm.nix`
# put `leigh-admin` in `nixbld`, `leigh-admin` sorts before `nixbld1` in
# /etc/group, and every build on services1 and services2 therefore ran as uid 1234
# and SIGKILLed the admin's whole process set: tmux, the Zed remote server,
# `user@1234.service`, `network-status.service` (then running as `User=leigh-admin`
# on services1; it is a DynamicUser on gw now),
# and the ssh session of whoever delegated the build. That is the whole of
# docs/nix-build-sigkill-2026-10-11.md — including why aibox, where nothing added
# itself to the group, was never affected (not, as the post-mortem guessed, because
# its `sandbox` is `true` rather than `relaxed`).
#
# Nothing needs the membership, whatever the comment in build-farm.nix used to say:
#
#   * `allowed-users = *` already admits every local user to the daemon, and that
#     is what accepts the ssh hop on a builder;
#   * `/nix/var/nix` is root-only, so a non-root nix always reaches the store
#     through the daemon socket rather than directly (store-registration.cc picks
#     `LocalStore` only when `access(stateDir, R_OK|W_OK)` succeeds) — group
#     membership buys a client no store access at all;
#   * what a client actually needs in order to ask for input-addressed builds is
#     `nix.settings.trusted-users`, which build-farm.nix still sets.
#
# Two guards, because the mistake produces no error when it is made — only a dead
# session minutes later — and is easy to make again:
#
#   1. an assertion, so the declarative version fails `nix flake check` / `just check`;
#   2. a timer, so the imperative version (`usermod -aG nixbld`, a hand-edited
#      /etc/group) is undone within minutes and shouted about in the journal.
{
  config,
  lib,
  pkgs,
  ...
}: let
  # nixpkgs creates the pool itself: nixbld1 … nix.nrBuildUsers, each with
  # `groups = [ "nixbld" ]`. Those are the only legitimate members.
  isPoolAccount = name: builtins.match "nixbld[0-9]+" name != null;

  # `group` is the primary group (nix does not read primary membership, but an
  # account whose primary group were nixbld is the same mistake).
  claimsNixbld = u:
    builtins.elem "nixbld" ([u.group] ++ u.groups or [] ++ u.extraGroups or []);

  offenders = builtins.attrNames (
    lib.attrsets.filterAttrs (n: u: !isPoolAccount n && claimsNixbld u) config.users.users
  );
in {
  assertions = [
    {
      assertion = offenders == [];
      message = ''
        common/nixbld.nix: ${lib.concatStringsSep ", " offenders} would be a member of the
        `nixbld` group. That group is the pool of UIDs nix runs builds as: nix
        assigns one member per build and SIGKILLs that UID before and after it,
        so a real account in `nixbld` loses every process it owns to every build
        on this machine (docs/nix-build-sigkill-2026-10-11.md).

        A client that has to be allowed to ask the daemon to build things needs
        `nix.settings.trusted-users` (and `nix.settings.allowed-users`), not membership.
      '';
    }
  ];

  # Self-healing half: /etc/group is what nix's getgrnam() reads (nsswitch has
  # `group: files …`), so pruning it here is enough — no restart of nix-daemon is
  # needed, the member list is re-read for every build.
  systemd.services.nixbld-guard = {
    description = "Keep non-pool accounts out of the nix build-user group";
    documentation = ["https://nixos.org/manual/nix/stable/package-management/conf-file"];
    path = [
      pkgs.coreutils
      pkgs.gnugrep
      pkgs.shadow
    ];
    serviceConfig = {
      Type = "oneshot";
      ExecStart = pkgs.writeShellScript "nixbld-guard" ''
        set -u

        bad=$(
          grep '^nixbld:' /etc/group \
          | cut -d: -f4 \
          | tr ',' '\n' \
          | grep -Ev '^$|^nixbld[0-9]+$' \
          || true
        )
        [ -z "$bad" ] && exit 0

        rc=0
        for u in $bad; do
          echo "removing '$u' from the nixbld group: nix runs builds as a member of that group and SIGKILLs the UID it picked, before and after every build, so a real account in nixbld has its whole process set destroyed by every build (docs/nix-build-sigkill-2026-10-11.md)" >&2
          gpasswd -d "$u" nixbld || { echo "nixbld-guard: FAILED to remove '$u' from nixbld" >&2; rc=1; }
        done
        exit $rc
      '';
    };
  };

  systemd.timers.nixbld-guard = {
    description = "Re-check the nix build-user group membership";
    wantedBy = ["timers.target"];
    timerConfig = {
      # Soon enough to catch a hand-made change before the next build does, and
      # cheap enough to run forever.
      OnBootSec = "2min";
      OnUnitActiveSec = "15min";
    };
  };
}
