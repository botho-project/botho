#!/usr/bin/env python3
"""Emit the #1987 m2 run list: one line per run, `<outfile>\t<cli args...>`.

Every run is one `cluster-tax-sim` call printing a JSON line per snapshot
(10 and 20 yr). Seeds 1696, 2, 3; mining wealth/egalitarian; tx-scale
1/100/1000; candidate rule #1790 (no flag) and node (--spent-eligible);
honest whale.
"""
import sys

out_dir = sys.argv[1]
SEEDS = [1696, 2, 3]
MINING = ["wealth", "egalitarian"]
SCALES = [1, 100, 1000]
RULES = [("1790", []), ("node", ["--spent-eligible"])]

# name -> (subcommand args, run)
CONFIGS = {
    # The four §7.1 shapes.
    "S1": (["redistribution-shapes", "--part", "m2", "--shape", "factor-price"], "on"),
    "S2": (["redistribution-shapes", "--part", "m2", "--shape", "churn-fee"], "on"),
    "S3a": (["redistribution-shapes", "--part", "m2", "--shape", "demurrage-miners"], "on"),
    "S3b": (["redistribution-shapes", "--part", "m2", "--shape", "settle-miners"], "on"),
    "S4a": (["redistribution-shapes", "--part", "m2", "--shape", "window-30d"], "on"),
    "S4b": (["redistribution-shapes", "--part", "m2", "--shape", "window-30d-value"], "on"),
    # Same-commit references (#1801 rules) and counterfactuals.
    "R0": (["redistribution-shapes", "--part", "m2", "--shape", "stock"], "on"),
    "R2": (["redistribution-redesign", "--part", "m2", "--variant", "inverse-factor"], "on"),
    "R3": (["redistribution-redesign", "--part", "m2", "--variant", "ticket-price"], "on"),
    "OFF": (["redistribution-shapes", "--part", "m2", "--shape", "stock"], "off"),
}

# Fast-path validation: the S4 incremental-weight path at the node's own
# window must reproduce R0 / R2 within seed noise.
VALIDATION = {
    "V-R0": (["redistribution-shapes", "--part", "m2", "--shape", "window-30d",
              "--window", "720:10000"], "on"),
    "V-R2": (["redistribution-shapes", "--part", "m2", "--shape", "window-30d-value",
              "--window", "720:10000"], "on"),
}

lines = []
for name, (args, run) in CONFIGS.items():
    for rule, flags in RULES:
        for mining in MINING:
            for scale in SCALES:
                for seed in SEEDS:
                    tag = f"{name}__{mining}__x{scale}__s{seed}__{rule}__{run}"
                    argv = args + ["--mining", mining, "--seed", str(seed),
                                   "--tx-scale", str(scale), "--run", run] + flags
                    lines.append((tag, argv))
for name, (args, run) in VALIDATION.items():
    for rule, flags in RULES:
        for mining in MINING:
            for scale in SCALES:
                seed = 1696
                tag = f"{name}__{mining}__x{scale}__s{seed}__{rule}__{run}"
                argv = args + ["--mining", mining, "--seed", str(seed),
                               "--tx-scale", str(scale), "--run", run] + flags
                lines.append((tag, argv))

# Slowest first (1000x, value-weighted, long window) so the tail is short.
def cost(item):
    tag, argv = item
    c = {"x1": 1, "x100": 3, "x1000": 8}[tag.split("__")[2]]
    if tag.startswith(("S4", "V-")):
        c *= 3
    if tag.startswith(("S2", "S4b", "R2", "V-R2")):
        c *= 2
    if tag.endswith("__off"):
        c *= 0.5
    return -c

lines.sort(key=cost)
for tag, argv in lines:
    print(f"{out_dir}/{tag}.json\t" + "\t".join(argv))
