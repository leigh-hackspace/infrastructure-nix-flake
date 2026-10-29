# MODULE — NixOS module; in a machine's module list, never read as data.
#
# The fleet's POSIX identity layer: authentik users and groups, resolved as Unix
# accounts, on the two boxes that need it — `aibox` (`machines/aibox/sso.nix`) and the
# Pi Room netboot image (`machines/pi-room-sys`).  Both used to carry their own hand-written
# copy of the same sssd domain and their own copy of the root CA; the two had already
# drifted (`cache_credentials` false/true, `debug_level` 8/6 — rule 2, third time).
#
# ─── What this layer actually is, measured 2026-10-29 ───────────────────────────────────
#
# * **Every uid is `2000 + authentik_user_pk`, and nothing stores it.**  Checked against all
#   105 users LDAP serves: `uidNumber == 2000 + id` for 105/105, and
#   `authentik_core_user` has no uid column and no `uidNumber` attribute (0/106).  The
#   number is recomputed per query from a Postgres sequence value.  It is load-bearing:
#   `/home/cjdell` on the NAS is `uid=2013 gid=2013` mode `0700`, and cjdell's pk is 13.
#   So home-directory *confidentiality* rests on a primary key surviving every restore.
#   See `docs/auth-posix-logins.md` before changing anything that could renumber a user.
# * **Group gids are computed too, and stored nowhere** — no column, no attribute, and
#   `dbsize` is 0 in redis.  They are not `gid_start_number + n` (4000 declared, 15295…76456
#   served) and they are not any simple hash of the group name or `group_uuid` (searched
#   crc32/adler32/md5/sha1/sha256/blake2b over seven spellings of each × 17 moduli, no
#   match).  So a gid is a function of fields we do not control, which is the important
#   part: renaming, deleting-and-recreating, or *upgrading the outpost* can re-key every
#   group-owned file at once, and nothing in any database would notice.
# * **Nested groups are unobtainable, in both directions.**  The outpost's `memberOf` is
#   not transitive and sssd's nested walk fails; `Members ⊂ Public` therefore delivers
#   `Public` to nobody who is not also a direct member.  Verified: `id bobm6tma` →
#   `(Members)` alone, before and after any sssd knob, while a direct member gets
#   `(Public)`.  `ldap_group_nesting_level = 0` does **not** silence the resulting
#   `Nested group processing failed` lines (measured: 9 cold-cache lookups → 18 with the
#   option set), because the failure is in direct-membership processing, not in the depth
#   limit.  Do not "fix" the noise that way.
# * **Every user's primary gid names no group at all** (105/105).  That is what sssd logs
#   the failure *about*, and it also means each user's processes have a primary group with
#   no name and no members — so group-based sharing is impossible here today.
# * **The bind credential is public, by construction.**  `ldap_default_authtok` must be in
#   sssd's config file, and on the netboot image that file is a store path inside a
#   squashfs that anyone who can PXE mounts and reads.  With it, the whole member list is
#   readable: `cn`, `displayName`, `mail`, `memberOf` for all 105 (measured — the outpost
#   does *not* serve `attributes`, so door-card ids are not exposed).  The outpost itself
#   behaves correctly: empty password → `Invalid credentials (49)`, wrong → `(49)`,
#   anonymous → `Insufficient access (50)`.
# * **Probe it with `ldapsearch`, never `ldapwhoami`** — the outpost does not implement the
#   Who-Am-I extended operation, so a *successful* bind answers
#   `Protocol error (2)`, which reads exactly like a failure.
# * **`id`/`getent` cannot tell a live directory read from a cache hit on these boxes.**
#   nscd is active in front of sssd *and* sssd keeps its own sysdb; on a netboot client both
#   are tmpfs but both survive a config change.  Measured the hard way: a client whose
#   `ldap_uri` was pointed at `no-such-host.invalid` went on answering `getent passwd` for a
#   user never looked up that boot.  Any experiment that changes sssd's config here must
#   stop `nscd`, clear `/var/lib/sss/db` and `/var/lib/sss/mc` (not `/var/lib/sssd`, which
#   is not the cache), and confirm against the outpost's own log on services1.
#
# ─── What this module changes relative to the two copies it replaces ────────────────────
#
# 1. `cache_credentials` converges on `false` (the netboot image said `true`; its cache is
#    tmpfs, so it cached password hashes for the length of one power cycle).
# 2. `ldap_id_use_start_tls` is gone.  It was `true` on a connection that is `ldaps://`
#    already — StartTLS and LDAPS are the two mutually exclusive ways to get TLS.
# 3. `debug_level` is an option, default `6` (aibox ran `8`, which logs every attribute of
#    every lookup into a journal other people read).
# 4. sssd is ordered after `network-online.target` rather than racing it.  Its restart policy is
#    nixpkgs' (50 s, deliberate) — see the comment on the unit for what happens if you "fix" it.
#
# Deliberately **not** changed here, each with its reason:
#
# * `access_provider = permit`.  Real authorisation is right, but with nesting unobtainable
#   a group gate is a gate on direct membership only, and I have no working instrument for
#   "would this deny a login" (see the cache note above).  It is an option below; the
#   migration and the measurement recipe are in `docs/auth-posix-logins.md`.
# * Moving `bindToken` out of the store.  sssd's `option# = /file` convention is the obvious
#   answer and I could not verify it — every attempt was defeated by the caching above.
#   Shipping an unverified secret mechanism into the auth path of two machines is worse than
#   the known problem.  Read the file's contents into the config *only* after proving it.
{
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.infra.authPosix;

  rootCa = ./pki/leighhack-root-ca.pem;

  sssdDomain =
    {
      id_provider = "ldap";
      auth_provider = "ldap";
      access_provider =
        if cfg.accessGroup == null
        then "permit"
        else "access";

      # Connection.  ldaps://, verified against this CA with `demand` from the netboot VLAN.
      ldap_uri = "ldaps://authentik.int.leighhack.org";
      ldap_search_base = "dc=ldap,dc=goauthentik,dc=io";
      ldap_tls_reqcert = "demand";
      ldap_tls_cacert = "/etc/static/pki/tls/certs/ca-bundle.crt";
      ldap_network_timeout = "5";
      ldap_opt_timeout = "5";

      # Bind credentials — see the header: this value is readable by anyone who can read the
      # rendered config, which on a netboot client is anyone who can plug in an Ethernet cable.
      ldap_default_bind_dn = "cn=pgina,ou=users,dc=ldap,dc=goauthentik,dc=io";
      ldap_default_authtok = cfg.bindToken;

      # Search bases
      ldap_user_search_base = "ou=users,dc=ldap,dc=goauthentik,dc=io";
      ldap_group_search_base = "ou=groups,dc=ldap,dc=goauthentik,dc=io";

      # User attributes, matched to authentik's LDAP schema
      ldap_user_object_class = "user";
      ldap_user_name = "cn";
      ldap_user_uid_number = "uidNumber";
      ldap_user_gid_number = "gidNumber";
      ldap_user_home_directory = "homeDirectory";
      ldap_user_shell = "loginShell";
      ldap_user_gecos = "displayName";

      # authentik never sets loginShell, so a user would land with no shell at all.
      default_shell = "/run/current-system/sw/bin/bash";
      fallback_homedir = "/home/%u";

      # Group attributes
      ldap_group_object_class = "group";
      ldap_group_name = "cn";
      ldap_group_gid_number = "gidNumber";
      ldap_group_member = "member";
      ldap_schema = "rfc2307bis";
      ldap_id_mapping = "false";

      # The outpost answers rootDSE only anonymously, and anonymous is refused.
      ldap_disable_rootdse = "true";

      # Caching.  Off, and see the header for why turning it on is not the offline-login win
      # it looks like on these two machines.
      cache_credentials = "false";
      enumerate = "false";
    }
    // lib.optionalAttrs (cfg.accessGroup != null) {
      # Direct membership only — `Members ⊂ Public` will not count here (header).
      access_provider = "access";
      require_in_group = "true";
      access_group = cfg.accessGroup;
    };
in {
  options.infra.authPosix = {
    enable =
      lib.mkEnableOption ''
        authentik as this machine's POSIX identity source (sssd over LDAPS)'';

    bindToken = lib.mkOption {
      type = lib.types.str;
      description = ''
        The shared LDAP bind secret.  It lands in this machine's store-hosted
        /etc/sssd/sssd.conf, so treat it as public to whoever can read the config —
        which for the netboot image is the whole Pi Room VLAN.  See the header for
        why it is not simply read from a file instead, and
        docs/auth-posix-logins.md for the plan to make it unnecessary.'';
    };

    accessGroup = lib.mkOption {
      type = with lib.types; nullOr (listOf str);
      default = null;
      description = ''
        authentik groups allowed to hold a session, e.g. ["Infra" "Members"].
        null keeps today's `access_provider = permit`: anyone with an authentik
        password may log in.  Read the header before setting it.'';
    };

    debugLevel = lib.mkOption {
      type = lib.types.ints.between 0 9;
      default = 6;
      description = ''
        sssd debug level.  8+ logs every attribute of every lookup into the
        journal; 6 is enough to see failures.'';
    };
  };

  config = lib.mkIf cfg.enable {
    # One copy of the CA that signs the LDAPS leaf, for the box that has to trust it.
    security.pki.certificates = [(builtins.readFile rootCa)];

    services.sssd = {
      enable = true;
      config = lib.generators.toINI {} {
        sssd = {
          config_file_version = "2";
          services = "nss, pam";
          domains = "authentik";
          debug_level = toString cfg.debugLevel;
        };
        nss = {
          filter_users = "root,nixbld";
          filter_groups = "root,wheel,nixbld";
          debug_level = toString cfg.debugLevel;
        };
        pam = {debug_level = toString cfg.debugLevel;};
        "domain/authentik" = sssdDomain;
      };
    };

    # The PAM stacks are generated from this, and sssd must not race them.
    security.pam.services.sddm.makeHomeDir = true;
    security.pam.services.login.makeHomeDir = true;
    security.pam.services.sshd.makeHomeDir = true;

    # Give sudo to members of the Infra group.  sssd lowercases group names, so both
    # spellings are kept — that is what makes this rule work, not belt-and-braces.
    security.sudo.extraRules = [
      {
        groups = ["infra" "Infra"];
        commands = [
          {
            command = "ALL";
            options = ["NOPASSWD"];
          }
        ];
      }
    ];

    # Ordered after the network.  The restart policy is deliberately left to nixpkgs: its sssd
    # unit sets `RestartSec = 50s` so a directory that is merely slow at boot gets left alone,
    # and overriding it at the same priority is not even a merge — `nix eval
    # .#nixosConfigurations.<box>.config.system.build.toplevel.drvPath` refuses outright with a
    # string-list merge error between systemd.nix's "0" and sssd.nix's "50s".  Tightening that
    # 50 s to a few seconds would hammer a directory that is coming up, which is the opposite of
    # this fleet's "never give up" pattern.
    systemd.services.sssd = {
      wants = ["network-online.target"];
      after = ["network-online.target"];
    };
  };
}
