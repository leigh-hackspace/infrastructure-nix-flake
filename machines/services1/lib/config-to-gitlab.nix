# config-to-gitlab.nix
(
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
)

# # Test function
# testConfigToGitLabString = config:
#   let
#     result = configToGitLabString config;
#     expected = "external_url 'http://gitlab.example.com';\ngitlab_rails['lfs_enabled'] = true;";
#   in
#   if result == expected then
#     "TEST PASSED"
#   else
#     "TEST FAILED\nExpected: ${expected}\nGot: ${result}";

# # Exports
# {
#   inherit configToGitLabString testConfigToGitLabString;
# }
