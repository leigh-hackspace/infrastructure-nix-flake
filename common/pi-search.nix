# MODULE — NixOS module; imported by `common/pi-agent.nix`, never by a machine directly.
#
# `programs.pi-agent.search.brave` gives every machine this flake deploys the same
# web-search answer `pi` was pointed at on 2026-11-16: the third-party
# `pi-web-access` package, pinned to an exact version, configured by a file this flake
# writes, authenticating with a key that is **never in the Nix store, never in a
# process's arguments, and never in an environment variable a session inherits**.
# `docs/pi-web-search.md` owns the narrative, the review, and the failure modes; this
# file owns the four moving parts:
#
#   1. `sops.secrets.brave_api_key`  → `/run/secrets/brave_api_key`, 0440, one group
#   2. `users.groups.<group>`        → whose members may read it (`programs.pi-agent.users`)
#   3. a world-readable `webSearch` object + `piPackages` list, contributed to the
#      generated config `common/pi-agent.nix` renders and `pi-agent-sync` merges into
#      `~/.pi/agent/web-search.json` and `~/.pi/agent/settings.json`
#   4. nothing else — no unit, no port, no vhost, no secret in a derivation
#
# ## Why a package and not a program of our own
#
# The first attempt at this was `crates/brave-search`, a Rust client for the Brave API
# plus a vendored skill (~1,000 lines, one credential decision, one HTTP client). It
# was deleted the same day, because `pi-web-access` already solves the same problem and
# solves more of it — search with a rate-limit-aware queue, `fetch_content`, PDFs,
# bounded result retrieval — and, decisively, because its config accepts a credential
# *source* rather than a credential:
#
#     "braveApiKey": "!/run/current-system/sw/bin/cat /run/secrets/brave_api_key"
#
# The extension runs that command per request (five-second timeout, 16 KiB cap, a
# minimised environment) and uses what it prints. So the boundary is the file's group
# membership — the same decision `machines/gw/conntrack-reader.nix` and
# `./lease-reader.nix` make for root-owned files a service must read (AGENTS.md
# rule 28) — and there is no credential in the store, in `argv` (world-readable on
# Linux), or in the environment of every process a session starts. A second copy of
# that logic in a crate we maintain would be rule 2 in a different hat.
#
# ## What the group is, and what it is not
#
# The group has exactly the members of `programs.pi-agent.users`, and it exists so that
# `cat` can read the key *as the person running pi*. It is not `secrets`: `secrets`
# holds every service credential on the box (authentik's secret key, postgres, the DO
# token), so putting a person's search key in it — or that person in it — would be a
# far larger grant than the one being made. Renaming this group to `secrets` later is
# the mistake to refuse.
#
# ## What is deliberately NOT configured
#
# The extension's own defaults are the safe ones and this module leaves them alone:
# browser-cookie extraction stays off, hosted page-fetch fallbacks stay off, and the
# SSRF guard that blocks private/reserved address ranges stays fully armed — which is
# what stops `fetch_content` from being a route from an agent session to the internal
# boxes on this network. `extraSettings` exists for a per-machine addition; it is
# rendered into a **world-readable store path**, so the assertion below refuses any
# `*ApiKey`/`*Token`/`*Secret` value that is a literal rather than a `$NAME` or
# `!command` source. That is AGENTS.md rule 13 as an evaluation error.
{
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.programs.pi-agent.search.brave;
  piCfg = config.programs.pi-agent;

  # `cat` from coreutils, by absolute path: the credential command runs with a
  # minimised environment, and "use absolute executable paths" is the package's own
  # requirement. Store path, so it is a different inode per nixpkgs bump and nothing
  # else can shadow it.
  cat = "${pkgs.coreutils}/bin/cat";
  keyFile = "/run/secrets/${cfg.secretName}";

  # The object whose keys the flake owns in `web-search.json`. Everything here was
  # verified live on 2026-11-16 against the pinned package (receipts in
  # `docs/pi-web-search.md`); keys the package does not know are its business, not
  # this file's.
  managedSettings =
    {
      # `auto` — the package's default — walks a fallback chain that starts with
      # Exa's hosted MCP endpoint, i.e. it sends the query somewhere else before it
      # sends it to Brave. Pinning the provider is also what makes "which provider
      # answered?" a fact in the flake rather than a fact about which keys happen to
      # be present on the box.
      provider = "brave";

      # A *source*: a command, not a value. Never put the key here; this object is
      # rendered into a world-readable store path by `common/pi-agent.nix`.
      ${cfg.keySetting} = "!${cat} ${keyFile}";

      # Raw results, no curator window and no second model call to summarise them.
      # The package's default today, pinned because a default that changes turns a
      # headless agent on a server into one that opens a browser.
      workflow = "none";

      # Pinned for the same reason: enabling this reads a browser's cookie database
      # to ask Google a question. Nothing here needs that, and an upstream flip of
      # the default would be invisible from here.
      allowBrowserCookies = false;
    }
    // cfg.extraSettings;

  # Every credential-shaped key must name a source. `ApiKey`, `Token`, `Secret`,
  # `Auth` — the names the package uses for its forty-odd providers — and a value
  # that is neither `"$NAME"`/`"${NAME}"` nor a `"!command"` is a literal, and a
  # literal here is a secret in the store, on every box that substitutes this system,
  # forever, by name (`common/sops.nix`, `infra.buildTimeSecrets`).
  credentialKeys = lib.filterAttrs (n: v: lib.any (s: lib.hasSuffix s n) ["ApiKey" "Token" "Secret" "Auth"]) managedSettings;
  isSource = v: lib.isString v && (lib.hasPrefix "!" v || lib.hasPrefix "$" v);
  literalCredentials = lib.attrNames (lib.filterAttrs (n: v: !(isSource v)) credentialKeys);
in {
  options.programs.pi-agent.search = {
    brave = {
      enable = lib.mkOption {
        type = lib.types.bool;
        # Web search follows the thing it serves: a box that manages pi's config at
        # all gets search, so "all machines benefit" is one line in
        # `common/pi-endpoints.nix`'s neighbourhood rather than five. Set false per
        # machine to withdraw it.
        default = piCfg.enable;
        defaultText = lib.literalExpression "config.programs.pi-agent.enable";
        description = ''
          Whether pi on this machine gets Brave web search: the pinned
          `pi-web-access` package declared, `web-search.json` written, and the API key
          provisioned.
        '';
      };

      packageSource = lib.mkOption {
        type = lib.types.str;
        default = "npm:pi-web-access@0.48.0";
        description = ''
          The pi package declaration, pinned to an exact version: a pi package is
          third-party code that runs inside every agent session on this machine, so an
          unpinned source would let it change underneath without a commit. Bumping it
          is a review of the new version's changelog, in the same shape as
          `cameras/thingino-manifest.sh` pins firmware (AGENTS.md rule 30) — see
          `docs/follow-ups.md` for the open review.
        '';
      };

      secretName = lib.mkOption {
        type = lib.types.str;
        default = "brave_api_key";
        description = ''
          The name in `secrets/secrets.yaml`, which is also its path under
          `/run/secrets` (sops-nix) and the file the credential command reads. One
          name, three uses, so they cannot disagree.
        '';
      };

      keySetting = lib.mkOption {
        type = lib.types.str;
        default = "braveApiKey";
        description = ''
          Which key of `web-search.json` carries the credential source. Named rather
          than hardcoded so the credential-key assertion below stays true if the
          package renames it.
        '';
      };

      extraSettings = lib.mkOption {
        type = with lib.types; attrsOf (oneOf [bool int str]);
        default = {};
        example = {numResults = 8;};
        description = ''
          Per-machine additions to the `webSearch` object, one level deep (a nested
          routing table is a decision to make when there is a real one in front of it).
          They land in a world-readable store path, so a credential here must also be a
          `"!command"` source or a `"$NAME"` one — the assertion says so.
        '';
      };
    };
  };

  config = lib.mkIf cfg.enable {
    # Both checks are evaluation-time, which is the only moment anyone can act on them
    # without a failed activation: `pi-agent-sync` runs as a unit an activation starts,
    # and the whole argument in `docs/pi-agent-models.md` about what that costs is that
    # a mistake should never reach that point.
    assertions = [
      {
        assertion = literalCredentials == [];
        message = ''
          programs.pi-agent.search.brave: ${lib.concatStringsSep ", " (map (n: "`${n}`") literalCredentials)}
          in the rendered web-search config carry a literal value rather than a source.
          That object is written into a world-readable Nix store path by
          common/pi-agent.nix, so a literal there is a credential in the store, on every
          box that builds or substitutes this system, forever, by name — the failure
          docs/pi-agent-models.md §Credentials documents. Make it a "!command" that
          reads a file (see braveApiKey in common/pi-search.nix) or a "$NAME" source.
        '';
      }
      {
        assertion = builtins.match "[A-Za-z0-9:@/._+-]+" cfg.packageSource != null;
        message = ''
          programs.pi-agent.search.brave.packageSource = "${cfg.packageSource}" is not a
          simple package source. It is merged verbatim into ~/.pi/agent/settings.json,
          and pi-agent-sync refuses a package entry containing whitespace — rightly, so
          the flake refuses it too, before five machines get a broken package list.
        '';
      }
    ];

    # The key itself. Runtime-only: nothing here goes through `config.sopsSecretText`,
    # so no part of it reaches the store (`common/sops.nix`, AGENTS.md "Secrets").
    sops.secrets.${cfg.secretName} = {
      mode = "0440";
      group = "brave-search-readers";
    };

    # One group, whose members are exactly the accounts whose pi may search. Same
    # caveat as `programs.pi-agent.users`: name only accounts this flake declares,
    # because these members are written by sysusers at boot and an SSS/authentik POSIX
    # account (`docs/auth-posix-logins.md`) is not resolvable that early.
    users.groups.brave-search-readers.members = piCfg.users;

    # The two contributions to the generated config `common/pi-agent.nix` renders and
    # `pi-agent-sync` merges. Neither is a secret: the store path holds a command that
    # *reads* a secret.
    programs.pi-agent.extraConfigSections = {
      piPackages = [cfg.packageSource];
      webSearch = managedSettings;
    };
  };
}
