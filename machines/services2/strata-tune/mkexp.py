"""Write /tmp/exp.json: the installed Strata run config with --expert-cache and
--prefill overridden, plus optional extra env vars and extra engine args.
services2's copy of machines/aibox/strata-tune/mkexp.py, with two differences:

  * the experiment server binds loopback and gets `"api_key": "none"`, so no key
    is ever written to a world-readable path or printed anywhere (the deployed
    key stays inside the store config; the key check is not on the timing path,
    and every arm is measured the same way);
  * it prints the args and the *names* of the env keys, never the config.

  strata-prep mkexp.py <expert-cache> <prefill> [extra-env-json] [extra-args-json]
  (the installed config path comes from $STRATA_CONFIG, set by strata-exp.sh)
"""

import json
import os
import stat
import sys

ec, pf = sys.argv[1], sys.argv[2]
extra_env = json.loads(sys.argv[3]) if len(sys.argv) > 3 and sys.argv[3] else {}
extra_args = json.loads(sys.argv[4]) if len(sys.argv) > 4 and sys.argv[4] else []

cfg = json.load(open(os.environ["STRATA_CONFIG"]))
args = cfg["args"]
args[args.index("--expert-cache") + 1] = ec
args[args.index("--prefill") + 1] = pf

# An extra flag replaces the deployed one of the same name rather than duplicating it:
# the engine reads left to right so a duplicate would win anyway, but a doubled flag
# in the printed arm line is a trap for the next reader.
for extra in [a for a in extra_args if a.startswith("--")]:
    while extra in args:
        i = args.index(extra)
        drop = 1 if i + 1 >= len(args) or args[i + 1].startswith("--") else 2
        del args[i : i + drop]
args += extra_args
cfg["args"] = args
cfg["log"] = "/tmp/exp-strata.log"
cfg["cwd"] = "/tmp"
cfg["api_key"] = "none"
cfg["env"].update(extra_env)
json.dump(cfg, open("/tmp/exp.json", "w"))
os.chmod("/tmp/exp.json", stat.S_IRUSR | stat.S_IWUSR)
print("engine args:", " ".join(args))
print("engine env keys:", " ".join(sorted(cfg["env"])))
