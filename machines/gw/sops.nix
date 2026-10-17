# Runtime secret files for the router, provisioned by sops-nix from
# secrets/secrets.yaml into /run/secrets (the same layout machines/services1/sops.nix
# documents: root:secrets, 0440, referenced by path and never read into a
# transcript — AGENTS.md's rule, and the reason neither of these two secrets'
# contents appears anywhere in this machine's config).
#
# The asymmetry to know about: sops-nix fails *activation*, not evaluation, when a
# key is missing from secrets/secrets.yaml, so a machine can build happily and then
# refuse to come up.  That is why the tunnel's secret below is declared only when
# there is a tunnel to dial: it has never existed, because the old box kept the
# AAISP credentials in OPNsense's own config and nobody has read them off it
# (./README.md says how), and declaring it on a box that does not dial anything
# would trade a missing fact for a failed boot.
let
  COEXIST = import ./coexist.nix;

  # One secret per half of ./uplink.nix's route out, and each is gated on the
  # thing that reads it.
  tunnelSecrets =
    if COEXIST.enable
    then {}
    else {
      # AAISP's tunnel credentials, as a chat script pppd reads (./uplink.nix).
      # NOT IN secrets/secrets.yaml YET — until it is added, the tunnel's unit
      # fails at start with the network's never-give-up policy retrying it, which
      # is the visible state ./README.md tells an operator to expect.
      #
      # The file's shape is pppd's chat-script form (the `ogc:`/`oc:`/`op:` presets
      # AAISP's own script expects), so the secret is AAISP's format, not the
      # network's — which is why it is a file and not three EnvironmentFile lines.
      aaisp_ppp_chat_secrets = {
        group = "secrets";
        mode = "0440";
      };
    };
in {
  sops.secrets =
    {
      # The tailnet's join key (a headscale pre-auth key — the network runs its own
      # control server, so this is not a Tailscale account key).  services1 declares
      # the same secret for headplane; the router needs it because it is the box that
      # carries the network's tailnet presence (./uplink.nix).
      #
      # Reusing one pre-auth key for two boxes is what the existing secret makes
      # possible, and it is a question rather than a decision: a per-device reusable
      # key is the tidier answer and costs one `headscale preauthkeys create`.
      #
      # Not gated on ./coexist.nix, unlike the tunnel: the box joins the tailnet in
      # both modes (it is how it is reachable when its own LAN config is what is
      # broken); only what it *advertises* on arrival changes.
      headplane_pre_authkey = {
        group = "secrets";
        mode = "0440";
      };
    }
    // tunnelSecrets;
}
