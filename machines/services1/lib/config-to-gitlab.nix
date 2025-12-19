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

    # Convert a value to GitLab config format
    convertValue =
      path: value:
      if builtins.isBool value then
        "${pathToKey path} = ${if value then "true" else "false"};"
      else if builtins.isString value then
        "${pathToKey path} = '${value}';"
      else
        "${pathToKey path} = ${toString value};";

    convertValueTop =
      path: value:
      if builtins.isAttrs value then
        # Recursively process nested attributes
        let
          attrNames = builtins.attrNames value;
          pairs = builtins.map (name: convertValue (path ++ [ name ]) value.${name}) attrNames;
        in
        builtins.concatStringsSep " " pairs
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
