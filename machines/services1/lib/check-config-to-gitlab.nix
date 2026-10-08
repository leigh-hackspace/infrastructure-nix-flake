# check-config-to-gitlab.nix
#
# Assertions over ./config-to-gitlab.nix — the hand-rolled
# Nix → GITLAB_OMNIBUS_CONFIG serializer that services/gitlab.nix calls (that
# module is currently disabled in services/default.nix; the attrset it passes is
# mirrored in ./gitlab-config-example.nix so these assertions still cover it).
# Run it with:
#
#   nix eval --impure --raw -f machines/services1/lib/check-config-to-gitlab.nix
#
# It prints "ok" or throws with the expected/actual token lists.  `just check`
# runs it.  Plain asserts on evaluated values: no derivation, no network, and it
# exercises the exact function the service uses.
let
  configToGitlab = import ./config-to-gitlab.nix;

  # The serializer joins top-level statements with single spaces, so comparing
  # whitespace-separated tokens lets the expectations below be written one per
  # line.  Token order is significant: the key sort order is part of what these
  # checks pin down.
  tokens = s:
    builtins.filter (t: builtins.isString t && t != "")
    (builtins.split "[ \n\t]+" (builtins.replaceStrings ["\n" "\t"] [" " " "] s));

  expect = name: cfg: wanted: let
    got = tokens (configToGitlab cfg);
    want = tokens wanted;
  in
    if got == want
    then true
    else
      builtins.throw ''
        ${name}:
          expected: ${builtins.toJSON want}
          got:      ${builtins.toJSON got}
      '';

  # Literal substring test.  builtins.match is POSIX ERE, where `\[` is an
  # invalid escape, so the assertions over the rendered config are done on
  # literal substrings rather than regexes.
  has = needle: s: builtins.replaceStrings [needle] [""] s != s;

  # Every string in the config is emitted inside '...' with no escaping, so a
  # value containing a quote would produce broken Ruby.  Collect the strings at
  # any depth so that assumption can be checked rather than just commented.
  collectStrings = value:
    if builtins.isString value
    then [value]
    else if builtins.isList value
    then builtins.concatLists (map collectStrings value)
    else if builtins.isAttrs value
    then builtins.concatLists (map collectStrings (builtins.attrValues value))
    else [];

  # The serializer's input, taken from services/gitlab.nix.  That module is
  # currently disabled in services/default.nix (the container is not deployed),
  # so the attrset is read out of the file rather than out of the evaluated
  # system — the check still covers the real config, and it keeps covering it
  # while the module is off.
  gitlabConfig = import ./gitlab-config-example.nix;
in
  assert expect "top-level string, bool and number" {
    external_url = "https://gitlab.example.com";
    boot = true;
    port = 8522;
  } ''
    boot true;
    external_url 'https://gitlab.example.com';
    port 8522;
  '';
  assert expect "nested attr becomes parent['child']" {
    gitlab_rails = {
      lfs_enabled = true;
      gitlab_shell_ssh_port = 8522;
    };
  } ''
    gitlab_rails['gitlab_shell_ssh_port'] = 8522;
    gitlab_rails['lfs_enabled'] = true;
  '';
  assert expect "list of strings" {
    nginx = {real_ip_trusted_addresses = ["10.88.0.0/24" "10.3.1.0/24"];};
  } ''
    nginx['real_ip_trusted_addresses'] = ['10.88.0.0/24', '10.3.1.0/24'];
  '';
  assert expect "list of attrs uses the ruby {...} form" {
    gitlab_rails = {
      omniauth_providers = [
        {
          name = "saml";
          args = {issuer = "https://gitlab.example.com";};
        }
      ];
    };
  } ''
    gitlab_rails['omniauth_providers'] = [{ args: { issuer: 'https://gitlab.example.com' }, name: 'saml' }];
  '';
  assert expect "keys are sorted, not insertion-ordered" {
    zeta = 1;
    alpha = 2;
  } "alpha 2; zeta 1;";
  # The documented limitation, pinned: escaping is not performed, so this is what
  # a quote in a value does.  If the serializer ever learns to escape, this
  # expectation changes deliberately rather than silently.
  assert expect "a value with a quote is emitted verbatim (documented limitation)" {
    gitlab_rails = {notice = "it's here";};
  } "gitlab_rails['notice'] = 'it's here';";
  # ... and the real config must not contain such a value.
  assert (
    let
      bad = builtins.filter (has "'") (collectStrings gitlabConfig);
    in
      bad == []
  );
  # The rendered result for the real config is omnibus-shaped.
  assert (
    let
      rendered = configToGitlab gitlabConfig;
    in
      has "external_url 'https://gitlab.leighhack.org';" rendered
      && has "gitlab_rails['lfs_enabled'] = true;" rendered
      && has "gitlab_rails['omniauth_providers'] = [{" rendered
      && has "nginx['real_ip_trusted_addresses'] = ['10.88.0.0/24'];" rendered
      && has "gitlab_rails['gitlab_shell_ssh_port'] = 8522;" rendered
  ); "ok\n"
