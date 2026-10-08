# Injects the shared crane helpers (common/crane.nix) into every module as the
# `CRANE` argument, built with that machine's own `pkgs`.
#
# Modules then just list `CRANE` in their function arguments:
#
#   { pkgs, CRANE, ... }: CRANE.cached { pname = "dns-sync"; ... }
#
# Importing common/crane.nix by hand in each module (seven times, with three
# different relative paths) was the alternative; this keeps the call sites
# honest about what they use while having the wiring in one place.
#
# The value depends only on `pkgs` and the `crane` flake input (both external
# module arguments), never on `config`, so reading `_module.args.CRANE` cannot
# introduce an evaluation cycle.
{
  pkgs,
  crane,
  ...
}: {
  _module.args.CRANE = import ./crane.nix {inherit pkgs crane;};
}
