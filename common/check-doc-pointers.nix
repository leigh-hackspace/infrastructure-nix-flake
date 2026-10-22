# CHECK — plain-eval assertion file: `nix eval --impure --raw -f` prints ok or throws; run by `just check` and by flake.nix's `checks`.
#
# Assertions over the *documentation*: the three files that tell a reader what to do —
# AGENTS.md, README.md, the justfile — and the recipe names, repo paths and doc links
# they promise.
#
#   nix eval --impure --raw -f common/check-doc-pointers.nix
#
# It prints "ok" or throws with every finding, each one carrying `file:line`. `just
# check` runs it, and flake.nix's `checks.<system>.doc-pointers` forces the same value,
# so `nix flake check` cannot pass on a tree whose instructions point at things that are
# not there.
#
# WHY AN EVALUATOR AND NOT THE RUST BINARY §7 OF THE AUDIT PROPOSED
# (docs/flake-simplification-audit-2026-10-22.md: "Add a small Rust binary in crates/ —
# call it docs-lint"): the same paragraph requires it to run "as the last step of
# `just check` (~2 s, no evaluation, no build, so it is safe on gw by construction)",
# and a binary has to be *compiled* before it can check anything — the one thing that
# recipe promises never to do, and the whole reason rule 22 exists. Neither can an
# evaluation ask `just --summary` what the recipes are, so the list is parsed out of the
# justfile below. This is the shape the repo already uses for assertions over source
# text: ./check-printer-states.nix substring-tests .nix and .rs files the same way, for
# the same reason — a finding at eval time costs nobody a build. If the rules ever
# outgrow `builtins.match`, the binary is still the right call, as `just docs-lint`:
# guarded like the other build-shaped recipes, with this evaluator covering what has to
# run everywhere including on the router.
#
# WHAT IT CHECKS, and the receipt behind each rule:
#
#   1. every `` `just <name>` `` in the instruction files is a recipe in the justfile.
#      `just dns-sync-sync` sat in AGENTS.md rule 14 for months after the recipe was
#      deleted on 2026-10-22, and rule 14 is the half of the file an agent obeys without
#      reading the rest of it;
#   2. every repo path with a file extension in those files exists — `secrets/.env.sample`
#      is really machines/services1/secrets/.env.sample, `machines/constants.nix` is
#      machines/gw/constants.nix, and the note a justfile comment sent readers to
#      (`machines/services1/network-status.nix`) moved to gw on 2026-10-21;
#   3. every `docs/*.md` is read by something — the two entry points, another doc, or a
#      machine's README — so a write-up cannot be created and then forgotten;
#   4. AGENTS.md never describes the router as a shadow: neither `10.3.1.254` nor
#      "co-exist"/"coexist" outside a line that is *about* the deletion. That sentence
#      was not stale documentation, it was an inverted safety model for the box that
#      leases every address in the building, and it was written with total confidence.
#
# What it does not check: the dated write-ups in docs/, which are properly full of files
# that no longer exist — that is what a post-mortem is. Only the instruction files are
# scanned, and `historical` is what lets those files talk about the past.
let
  root = ../.;

  instructionFiles = [
    "AGENTS.md"
    "README.md"
    "justfile"
  ];

  read = f: builtins.readFile (root + "/${f}");

  instructionText = builtins.concatStringsSep "\n" (map read instructionFiles);

  # builtins has no hasPrefix/optionalAttrs, and these files are evaluated with `-f`
  # and nothing else: no lib, no flake, no derivation.
  hasPrefix = pre: s: builtins.substring 0 (builtins.stringLength pre) s == pre;

  # A successful builtins.match with no captured group, or a failed one, is []/null;
  # every extraction below goes through here.
  firstMatch = l:
    if l == null || l == []
    then null
    else builtins.head l;

  # ... and the pair they are always used as: extract, then drop what did not match.
  matched = ls: builtins.filter (x: x != null) (map firstMatch ls);

  has = sub: s: builtins.replaceStrings [sub] [""] s != s;

  uniq = builtins.foldl' (acc: x:
    if builtins.elem x acc
    then acc
    else acc ++ [x]) [];

  # A line may name a thing that is gone when it says so in the line. This is what makes
  # the lint a hard check rather than a nag: history stays writable, and the exception
  # is a phrase in the sentence rather than a flag in a config file someone forgets to
  # clear. `docs-lint:allow` is the escape hatch for the residue, and it is greppable.
  historyWords = [
    "used to"
    "was deleted"
    "were deleted"
    "is gone"
    "gone"
    "deleted"
    "moved"
    "no longer"
    "retired"
    "formerly"
    "previously"
    "history"
    "removed"
    "stopped being"
    "went to"
    "since "
    "cut over"
    "the old box"
    "old router"
    "was here"
    "shadow"
    "it became"
    "now "
    "is history"
    "was on "
    "off services2"
    "off the router"
    "was refused"
    "no such"
    "in place of"
    "instead of"
    "rather than"
    "was the"
    "were the"
    "before"
    "had been"
    "it was"
    "that is gone"
    "were deleted"
    "was removed"
    "are gone"
  ];

  historical = line: builtins.any (w: has w line) historyWords || has "docs-lint:allow" line;

  # --- the justfile's own recipe list -------------------------------------------
  #
  # In just, a recipe or variable definition starts at column 0 and every body line is
  # indented, so an anchored expression separates the two. The middle group is a
  # parameter list — `build-frontend crate="all": …` — which is why it may hold anything
  # except the colon that ends the head. Variables (`SOPS_KEY_FILE := …`) fall out as
  # names too; harmless, since an extra name can only accept a mention, never invent one.
  recipeOf = line: builtins.match "([A-Za-z_][A-Za-z0-9_-]*)([^:]*):(.*)" line;

  definedNames = uniq (matched (map recipeOf (linesOf (read "justfile"))));

  linesOf = s: builtins.filter builtins.isString (builtins.split "\n" (s + "\n"));

  # --- check 1: `just <name>` mentions ------------------------------------------
  #
  # Per line, not per file: a finding has to say where. Split on the literal "`just " and
  # read the identifier after it. `just <name>` and `just --summary` have no plain name
  # and fall out as null; `just test|clippy|…` yields its first entry; `just filestore-*`
  # yields `filestore-`, which is a real family, and `just dns-sync-*` did not.
  recipeFindings = let
    # The identifier right after "`just ", if the line offers one.
    tokensIn = line:
      matched (
        map
        (builtins.match "([A-Za-z][A-Za-z0-9_.-]*)([^A-Za-z0-9_.-].*)?")
        (builtins.tail (builtins.filter builtins.isString (builtins.split "`just " line)))
      );
    known = t: builtins.elem t definedNames || builtins.any (r: hasPrefix t r) definedNames;
    walk = f: ls: n:
      if ls == [] || ls == null
      then []
      else let
        l = builtins.head ls;
        rest = builtins.tail ls;
        bad = builtins.filter (t: !known t) (tokensIn l);
        mine = map (t: "${f}:${toString n}: no recipe `just ${t}` in the justfile") (
          if historical l
          then []
          else bad
        );
      in
        mine ++ walk f rest (n + 1);
  in
    builtins.concatLists (map (f: walk f (linesOf (read f)) 1) instructionFiles);

  # --- check 2: repo paths with a file extension --------------------------------
  #
  # For each top-level directory, every run of path characters after it is a candidate;
  # keep those ending in a known extension. A colon is not a path character, so
  # `machines/gw/lib/check-network-tables.nix:19` loses its line number, and a `*` marks
  # a glob and is skipped. `builtins.pathExists` over `root + "/…"` is pure evaluation:
  # no store copy, no network, nothing built.
  pathFindings = let
    dirs = [
      "common/"
      "machines/"
      "crates/"
      "docs/"
      "secrets/"
    ];
    extensions = [
      ".nix"
      ".md"
      ".yaml"
      ".sample"
      ".sh"
      ".toml"
      ".rs"
      ".json"
      ".conf"
      ".service"
      ".ignore"
      ".txt"
    ];
    # A run of path characters at the start of whatever followed the directory, with
    # trailing punctuation off the end: prose writes `common/hosts.nix.` and a greedy
    # class would go looking for a file named "hosts.nix.".
    pathToken = s: let
      raw = builtins.match "([A-Za-z0-9._/-]+)(.*)" s;
      trim = builtins.match "(.*[A-Za-z0-9_-])(.*)" (
        if raw == null || raw == []
        then ""
        else builtins.head raw
      );
    in
      if raw == null || raw == []
      then null
      else if trim == null || trim == []
      then builtins.head raw
      else builtins.head trim;
    candidatesIn = line:
      builtins.concatLists (
        map
        (d: map (t: d + t) (builtins.filter (x: x != null) (map pathToken (builtins.tail (builtins.filter builtins.isString (builtins.split d line))))))
        dirs
      );
    plausible = ps: builtins.filter (p: builtins.any (e: has e p) extensions) (uniq ps);
    # One path mentioned inside another is not a second reference:
    # `machines/services1/secrets/.env.sample` also contains "secrets/", and splitting
    # on each directory would invent `secrets/.env.sample`. Anything that is the tail
    # of a candidate that *does* exist is therefore that longer path seen from a
    # different angle, and is dropped.
    hasSuffix = suf: s: let
      ls = builtins.stringLength s;
      lf = builtins.stringLength suf;
    in
      ls >= lf && builtins.substring (ls - lf) lf s == suf;
    resolve = ps: let
      good = builtins.filter (p: builtins.pathExists (root + "/${p}")) ps;
    in
      builtins.filter (p: !builtins.any (q: q != p && hasSuffix "/${p}" q) good) ps;
    walk = f: ls: n:
      if ls == [] || ls == null
      then []
      else let
        l = builtins.head ls;
        rest = builtins.tail ls;
        bad = builtins.filter (p: !has "*" p && !builtins.pathExists (root + "/${p}")) (resolve (plausible (candidatesIn l)));
        mine = map (p: "${f}:${toString n}: no such file, referenced as ${p}") (
          if historical l
          then []
          else bad
        );
      in
        mine ++ walk f rest (n + 1);
  in
    builtins.concatLists (map (f: walk f (linesOf (read f)) 1) instructionFiles);

  # --- check 3: every doc is read by something ----------------------------------
  #
  # A doc counts as read when its filename appears in an entry point, in another doc, or
  # in a machine's README — but not merely in itself, which is why the haystack is built
  # per file.
  docsDir = builtins.readDir ../docs;
  docFiles = builtins.filter (f: docsDir.${f} or null == "regular") (builtins.attrNames docsDir);
  machinesDir = builtins.readDir ../machines;
  machineNames = builtins.filter (m: machinesDir.${m} or null == "directory") (builtins.attrNames machinesDir);
  machineReadmes = builtins.filter builtins.pathExists (map (m: root + "/machines/${m}/README.md") machineNames);

  haystackFor = self:
    builtins.concatStringsSep "\n" (
      [instructionText]
      ++ map (d: builtins.readFile (root + "/docs/${d}")) (builtins.filter (d: d != self) docFiles)
      ++ map builtins.readFile machineReadmes
    );

  orphanFindings = map (d: "docs/${d} is referenced by nothing: not AGENTS.md, not README.md, not another doc, not a machine README") (builtins.filter (d: !has d (haystackFor d)) docFiles);

  # --- check 4: the router is never a shadow ------------------------------------
  shadowWords = [
    "10.3.1.254"
    "co-exist"
    "coexist"
  ];
  shadowFindings = let
    walk = ls: n:
      if ls == [] || ls == null
      then []
      else let
        l = builtins.head ls;
        rest = builtins.tail ls;
        mine =
          if builtins.any (w: has w l) shadowWords && !historical l
          then ["AGENTS.md:${toString n}: calls the router a shadow: ${l}"]
          else [];
      in
        mine ++ walk rest (n + 1);
  in
    walk (linesOf (read "AGENTS.md")) 1;

  findings = recipeFindings ++ pathFindings ++ orphanFindings ++ shadowFindings;
in
  if findings == []
  then "ok\n"
  else
    throw ''
      check-doc-pointers: ${toString (builtins.length findings)} finding(s):
        - ${builtins.concatStringsSep "\n  - " findings}''
