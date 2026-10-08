# config-to-gitlab.nix
#
# Renders an attrset as a GITLAB_OMNIBUS_CONFIG string — the Ruby-ish
# `key 'value';` / `gitlab_rails['x'] = y;` form GitLab's omnibus config
# understands (used by services/gitlab.nix).
#
# Output is one line per top-level key, keys sorted, separated by spaces.
# Strings are wrapped in single quotes **verbatim**: no escaping is performed,
# because the values here are config literals written in this repo (URLs,
# booleans, ports, cert fingerprints).  A value containing a `'` would produce
# broken Ruby — `check-config-to-gitlab.nix` asserts that none of the values
# actually rendered contain a quote, so the assumption cannot rot silently.
#
# Nested attrsets use `parent['child']`; lists inside them use Ruby's `{...}`
# form for attrs (that is what omnibus expects for e.g. omniauth_providers).
config:
let
  # Convert path to GitLab array notation
  pathToKey =
    path:
    let
      key = builtins.head path;
      rest = builtins.tail path;
    in
    if builtins.length rest == 0 then key else "${key}['${builtins.concatStringsSep "']['" rest}']";

  convertSingleValue =
    value:
    if builtins.isBool value then
      "${if value then "true" else "false"}"
    else if builtins.isString value then
      "'${value}'"
    else if builtins.isList value then
      "[${builtins.concatStringsSep ", " (builtins.map (item: (convertSingleValue item)) value)}]"
    else if builtins.isAttrs value then
      "{ ${convertAttrs2 [ ] value} }"
    else
      "${toString value}";

  # Convert a value to GitLab config format
  convertValue2 = path: value: "${pathToKey path}: ${convertSingleValue value}";

  convertValue = path: value: "${pathToKey path} = ${convertSingleValue value}";

  convertAttrs2 =
    path: value:
    (
      let
        attrNames = builtins.attrNames value;
        pairs = builtins.map (name: (convertValue2 (path ++ [ name ]) value.${name})) attrNames;
      in
      builtins.concatStringsSep ", " pairs
    );

  convertAttrs =
    path: value:
    (
      let
        attrNames = builtins.attrNames value;
        pairs = builtins.map (name: ("${convertValue (path ++ [ name ]) value.${name}};")) attrNames;
      in
      builtins.concatStringsSep " " pairs
    );

  convertValueTop =
    path: value:
    if builtins.isAttrs value then
      # Recursively process nested attributes
      convertAttrs path value
    else if builtins.isBool value then
      "${pathToKey path} ${if value then "true" else "false"};"
    else if builtins.isString value then
      "${pathToKey path} '${value}';"
    else
      "${pathToKey path} ${toString value};";

  # Get all top-level keys and convert them
  keys = builtins.attrNames config;
  pairs = builtins.map (key: convertValueTop [ key ] config.${key}) keys;
in
builtins.concatStringsSep " " pairs
