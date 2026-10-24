# Runtime secret files for the router, provisioned by sops-nix from
# secrets/secrets.yaml into /run/secrets (the same layout machines/services1/sops.nix
# documents: root:secrets, 0440, referenced by path and never read into a
# transcript — AGENTS.md's rule, and the reason neither secret's contents
# appears anywhere in this machine's config).
#
# The asymmetry to know about: sops-nix fails *activation*, not evaluation, when a
# key is missing from secrets/secrets.yaml, so a machine can build happily and then
# refuse to come up.  That is why a secret is declared only when something on this
# box reads it.  The tunnel's password exists in secrets/secrets.yaml — it was read
# off the old box on 2026-10-17 while the old box was still reachable, which was
# the last chance to get it — and ./uplink.nix dials the tunnel on every boot.
{
  sops.secrets = {
    # AAISP's PPP password, read from the old box's `/conf/config.xml`
    # (`<ppps><password>`) on 2026-10-17 and written into secrets/secrets.yaml.
    # It is a single line, not a chat script: ./uplink.nix's unit writes it into
    # pppd's pap-secrets/chap-secrets files at start, so the password never
    # appears in a process listing or in the Nix store.
    aaisp_ppp_password = {
      group = "secrets";
      mode = "0440";
    };

    # The tailnet's join key (a headscale pre-auth key — the network runs its own
    # control server, so this is not a Tailscale account key).  services1 declares
    # the same secret for headplane; the router needs it because it is the box that
    # carries the network's tailnet presence (./uplink.nix).
    #
    # Reusing one pre-auth key for two boxes is what the existing secret makes
    # possible, and it is a question rather than a decision: a per-device reusable
    # key is the tidier answer and costs one `headscale preauthkeys create`.
    headplane_pre_authkey = {
      group = "secrets";
      mode = "0440";
    };

    # The shared env-file secret (`CONFIG.ENV_FILE`), needed here for exactly one of
    # its keys: `DO_AUTH_TOKEN`, which ./http.nix's ACME DNS challenge presents to
    # DigitalOcean to prove this box may issue for `int.leighhack.org`
    # (docs/router-front-door-migration.md, consequence 1 — the cert moves with the
    # nginx that uses it, and the token that answers for the zone moves with the
    # cert).  It is a *runtime* file read by PID 1 at issuance time, so the token
    # reaches no derivation and no process listing, and it is never printed or copied
    # off services1 (AGENTS.md, "Secrets").
    #
    # Say why, because the file is the network's shared one: it is a dozen unrelated
    # `KEY=value` secrets, so this line grants the router read access to all of them
    # the moment sops renders it.  That is the argument for the sentence above being
    # true and for nothing else from machines/services1/sops.nix coming across with
    # it — a router that grows a secret because "the front door has one" has stopped
    # being a router and started being a second services1.  The front door does grow from
    # here — the `nginx-sso` gate and `anubis` arrive with the public names in phase 2/3 of
    # the migration — and each of those owes its own line below with the same shape: name
    # the key, name the unit that reads it.  "The front door needs secrets" is not a reason;
    # "this oneshot dials the DO API with this token" is.
    env_file = {
      group = "secrets";
      mode = "0440";
    };
  };
}
