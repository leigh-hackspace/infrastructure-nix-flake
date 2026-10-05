"""Write /tmp/exp.json: the installed Strata run config with --expert-cache and
--prefill overridden, plus optional extra env vars and extra engine args.

  strata-prep mkexp.py <expert-cache> <prefill> [extra-env-json] [extra-args-json]
  (the installed config path comes from $STRATA_CONFIG, set by strata-exp.sh)
"""

import json
import os
import sys

ec, pf = sys.argv[1], sys.argv[2]
extra_env = json.loads(sys.argv[3]) if len(sys.argv) > 3 and sys.argv[3] else {}
extra_args = json.loads(sys.argv[4]) if len(sys.argv) > 4 and sys.argv[4] else []

cfg = json.load(open(os.environ["STRATA_CONFIG"]))
args = cfg["args"]
args[args.index("--expert-cache") + 1] = ec
args[args.index("--prefill") + 1] = pf
args += extra_args
cfg["args"] = args
cfg["log"] = "/tmp/exp-strata.log"
cfg["cwd"] = "/tmp"
cfg["env"].update(extra_env)
json.dump(cfg, open("/tmp/exp.json", "w"))
print("engine args:", " ".join(args))
print("engine env:", cfg["env"])
