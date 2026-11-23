# MODULE — NixOS module; in a machine's module list or a directory's imports, never read as data.
# The two halves of the LAN ssh hop, pinned in the store: what an ssh **checks** and what
# an ssh **offers** (AGENTS.md rules 25 and 32).
#
# **Why these two lines live together and not next to ./users.nix.** They are the system
# client config, `/etc/ssh/ssh_config`, which is the only file every ssh on the box reads —
# root's included, out of `HOME=/root`. This pinned nixpkgs writes **no home files**
# (`./users.nix` has the refusal: `users.users.<name>` has no file-writing option at this
# pin), and rules 25 and 32 are both about ssh that is not the operator's: a job, a timer,
# a `git` invoked by a deploy. The same reason holds now that the nix hop is gone — the
# verb that most needs them is `git pull`, typed as `leigh-admin`, on a box whose key is
# `~/.ssh/agent-hop-key` and no other name (AGENTS.md rule 32's receipt is services3
# printing `(leigh-admin@10.3.1.20) Password:` and hanging, on a `prohibit-password` box).
#
# Until 2026-11-23 these blocks were the ssh half of `common/build-farm.nix`, because the
# remote builder's hop needed both: the daemon's ssh as root to reach aibox, and the
# identity to offer once there. The builder is withdrawn (see ./nix-gc.nix's header), and
# the pins stay, because every other LAN hop needs them just the same — `just deploy`,
# `just known-hosts`, the NAS, the UPS exporter's ssh from `gw`, and any `git pull` on any
# box.
#
# ---
#
# ## If delegation is ever wanted back, this is the whole list
#
# Not kept as live config on purpose: dead config is deleted, not parked (AGENTS.md
# rule 23), and the first three items were each learned the expensive way.
#
# 1. `nix.buildMachines` **alone is not enough**: `nix-remote-build.nix` sets
#    `nix.settings.builders = null` unless `nix.distributedBuilds` is true, so the
#    `nix.conf` line the daemon reads stays empty and `--max-jobs 0` fails with "local
#    builds are disabled" even though `/etc/nix/machines` is correct. Both are needed.
# 2. **The builder must *trust* the client's ssh user, not merely accept it.** With
#    `require-sigs = true` (the default) the builder's daemon refuses to build
#    input-addressed derivations for a client it does not trust, so every delegated build
#    died with `you are not privileged to build input-addressed derivations` — hit
#    2026-10-11 on services1's `nixos-rebuild`, because the sops-secret derivations are
#    built *during evaluation* and therefore go to the builder. The grant is
#    `nix.settings.trusted-users = [ "leigh-admin" ]`, on **both** ends, and it is a
#    different thing from `allowed-users` (which is `*` here and is what admits a client)
#    and a very different thing from `nixbld` membership (`common/nixbld.nix` refuses
#    that, and used to SIGKILL real accounts).
# 3. **`nix.settings.builders-use-substitutes = true` is per-*client* and is not the
#    default.** Off, every input of every delegated build is substituted by the
#    delegating box and then uploaded over the hop, with the builder's job slots idle —
#    which is exactly what "the build farm is slow" measured as on 2026-10-25. On, the
#    builder fetches what it can itself and asks for the rest.
# 4. A box that delegates and runs `nix.settings.max-jobs = 0` has **no local fallback**,
#    which is a guarantee and not a preference: the build fails loudly instead of quietly
#    compiling on the box you are standing on. `gw` is the only box that ever ran that way
#    and it no longer does (machines/gw/default.nix).
# 5. The pinned host keys below and the identity block below are what make the hop work
#    at all from a daemon that has no terminal and no operator `known_hosts`.
#
# ---
{
  config,
  lib,
  ...
}: let
  HOSTS = import ./hosts.nix;
in {
  # Every box that takes part in a LAN hop pins the network's own host keys, in the
  # store, rather than trusting whichever root `known_hosts` happens to exist: when a
  # job's ssh fails with `Host key verification failed` there is nothing else in the log,
  # and the box that *works* is the one someone ssh'ed from by hand at some point. See
  # ./ssh-known-hosts.nix for why pinning (rather than accept-new) is the point, and
  # `just known-hosts` for regenerating it after a box regenerates its keys.
  programs.ssh.knownHosts = import ./ssh-known-hosts.nix;

  # The other half, and the half that was missing on every box until 2026-11-22.
  #
  # `knownHosts` says what an ssh *checks*; nothing said what an ssh *offers*.
  # The fleet's key is at `~/.ssh/agent-hop-key`, which is none of the file names ssh
  # tries on its own (`id_rsa`, `id_ed25519`, …), and **no ssh-agent runs on these
  # boxes** — so an ssh that does not carry `-i ~/.ssh/agent-hop-key` offers *nothing*,
  # and sshd's last offer is a **prompt**.  That is exactly what a plain `git pull`
  # printed on services3 on 2026-11-15:
  #
  # (leigh-admin@10.3.1.20) Password:
  #
  # on a box whose sshd is `prohibit-password`, inside a job that had no terminal to
  # answer it.  The job wedges, and the wedge reads as "the network" rather than as
  # "no identity was offered" — the symptom appears far from the cause, the same shape
  # as the `Host key verification failed` failure above, one file over.
  #
  # Why the *system* client config rather than a per-user `~/.ssh/config`: this pinned
  # nixpkgs' `users.users.<name>` submodule has **no file-writing option at all** —
  # `just check` refuses `users.users.leigh-admin.extraFiles`, and this release moved
  # authorized keys to `AuthorizedKeysFile`/`/etc/ssh/authorized_keys.d`
  # (`nixos/modules/services/networking/ssh/sshd.nix`), leaving home files to Home
  # Manager, which this flake does not use.  `programs.ssh.hosts` is the
  # attribute-name-as-`Host`-pattern mechanism the module already has, and its own
  # documented example is this stanza (`IdentityFile` + `IdentitiesOnly`).  `~` in
  # `IdentityFile` is expanded by ssh to the *calling* user's home, so it names the
  # right file on every box without naming a box — and being the system file, it also
  # reaches root's ssh (`HOME=/root`), which the hand-written stanzas used to be the
  # only thing standing in for.
  #
  # What each line is for:
  #
  # * `IdentitiesOnly` is the documented fix for `Too many authentication failures`
  #   (`just/deploy.just`'s `deploy` recipe): a box that offers every identity it has is
  #   refused *before* the right one is tried — services1's `~/.ssh` also holds
  #   `id_ed25519`, which no LAN machine accepts, and measured from services3 a bare
  #   `git ls-remote` against the peer reached exactly that line.  Scoped to these hosts
  #   it is why the stanza is safe: GitHub is not one of them, and the documented leak
  #   trap (AGENTS.md rule 33 — an **exported** `GIT_SSH_COMMAND` putting the hop key on
  #   the GitHub remote, `Permission denied (publickey)`) cannot happen from a generated
  #   file, because the host is what the block matches on.
  # * `ConnectTimeout` so a hop to a box that is not answering fails *inside* a tool
  #   call rather than hanging it (rule 11: bound every wait).
  # * deliberately **no** `PasswordAuthentication no`, and no match on a *user*:
  #   services3's owner account keeps a password login as the *spare path* onto a box
  #   whose only uplink is a Wi-Fi dongle (`machines/services3/users.nix`, which warns to
  #   check `sudo sshd -T | grep -i passwordauthentication` after any ssh change), and a
  #   host-keyed `no` would retire that fallback for whoever types it.  Pubkey is tried
  #   first, so this stanza answers a LAN hop while the password path stays available
  #   underneath it.
  # * the core switches are **not** in the list (Telnet only,
  #   `docs/procurve-switches.md`); the NAS **is** (`docs/nas-and-borg.md`:
  #   `leigh-admin@10.3.1.6` is keyed with the machine-hop key), and it is one of
  #   `./ssh-known-hosts.nix`'s pins, so the two halves of the hop stay keyed to one
  #   address list — this one, generated from `./hosts.nix`.
  #
  # `order = 100` is safe because nothing else in the pinned nixpkgs declares
  # `programs.ssh.hosts` (verified: no other module names the option) and the module
  # throws on two blocks sharing an order; blocks are written in ascending order, so
  # keep this low for when a catch-all `"*"` block is ever added.  Read the rendered
  # file after any switch: `/etc/ssh/ssh_config` must carry the `Host` line and the
  # stanza below it (rule 21 — read the rendered file, never evaluate).
  programs.ssh.hosts = {
    "${HOSTS.services1} ${HOSTS.services2} ${HOSTS.services3} ${HOSTS.aibox} ${HOSTS.router} ${HOSTS.nas}" = {
      order = 100;
      IdentityFile = "~/.ssh/agent-hop-key";
      IdentitiesOnly = true;
      ConnectTimeout = 5;
    };
  };
}
