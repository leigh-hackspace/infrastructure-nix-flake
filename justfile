# https://just.systems

# The recipes live in `just/`, one file per job, imported below.  This file was 2338
# lines — past AGENTS.md rule 26 — and the guard for that rule could not see it:
# `check-source-size` scans tracked source by extension, and a bare `justfile` has no
# extension, so rule 26 was blind to the largest source file in the tree.  It scans
# justfiles now, which is why the split is the fix rather than an exemption.
#
# Two facts about the split worth knowing before editing one of these files:
#   * `just` searches for *this* file and then loads the imports, so every recipe is
#     still resolved from the repo root, and a recipe in one module can be a
#     prerequisite of a recipe in another (`_build-host-guard` is in `deploy.just` and
#     half the build-shaped recipes call it).
#   * `just --list` prints a recipe's doc comment, which is the comment block directly
#     above it (its **last line** is what is printed), so every recipe's comment has to stay
#     attached to it — that is why the one-line summary sits at the *end* of these blocks.
#     Section headers belong to the module, not to a recipe; several of them had drifted into
#     the middle of somebody else's docstring, which is the one thing the doc-pointer gate
#     cannot see: it can prove a recipe name exists, not that the block above it is about it.

default:
    @ {{ just_executable() }} --list --justfile {{ justfile() }} --unsorted

# The one variable a recipe in a module uses; kept here because just evaluates a
# recipe's `{{ }}` expansions against the justfile that was searched, not the one the
# recipe was written in.
SOPS_KEY_FILE := "{{env_var_or_error('HOME')}}/.config/sops/age/keys.txt"

import "just/deploy.just"
import "just/format.just"
import "just/rust.just"
import "just/ssh.just"
import "just/wan.just"
import "just/ups.just"
import "just/netboot.just"
import "just/printers.just"
import "just/checks.just"
import "just/build.just"
import "just/tests.just"
import "just/secrets.just"
import "just/pi.just"
import "just/slack.just"
import "just/voice.just"
import "just/cameras.just"
import "just/pbx.just"
import "just/ha.just"
import "just/radios.just"
import "just/netd.just"
