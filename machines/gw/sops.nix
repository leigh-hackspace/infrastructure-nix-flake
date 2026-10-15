# Runtime secret files for the router, provisioned by sops-nix from
# secrets/secrets.yaml into /run/secrets (the same layout machines/services1/sops.nix
# documents: root:secrets, 0440, referenced by path and never read into a
# transcript — AGENTS.md's rule, and the reason neither of these two secrets'
# contents appears anywhere in this machine's config).
#
# ONE OF THESE TWO SECRETS DOES NOT EXIST YET, and that is the difference between
# "evaluates" and "boots": sops-nix fails *activation*, not evaluation, when a key
# is missing from secrets/secrets.yaml, so this machine will build happily and then
# refuse to come up.  ./README.md says which one and what to put in it.
{
  sops.secrets = {
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

    # AAISP's tunnel credentials, as a chat script pppd reads (./uplink.nix).
    # NOT IN secrets/secrets.yaml YET — it has never existed, because the old box
    # kept them in OPNsense's own config and nobody has read them off it.  Until it
    # is added, `pppd-aaisp-tunnel` fails at start with the network's never-give-up
    # policy retrying it, which is the visible state ./README.md tells an operator
    # to expect.
    #
    # The file's shape is pppd's chat-script form (the `ogc:`/`oc:`/`op:` presets
    # AAISP's own script expects), so the secret is AAISP's format, not the
    # network's — which is why it is a file and not three EnvironmentFile lines.
    aaisp_ppp_chat_secrets = {
      group = "secrets";
      mode = "0440";
    };
  };
}
