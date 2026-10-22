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
  };
}
