#!/usr/bin/env python3
"""Aggregate the #1987 m2 runs into the study's tables (Markdown on stdout).

ΔGini = gini(off) - gini(on), the mechanism's own effect (#1790 bar), per
seed; a cell passes iff the 3-seed mean exceeds 0.05 at both 10 and 20 yr.

Usage:
    redistribution_shapes_tables.py docs/research/generated/redistribution-shapes-m2.jsonl
    redistribution_shapes_tables.py <run dir of *.json from redistribution_shapes_jobs.py>
"""
import glob
import json
import os
import sys

source = sys.argv[1]
SEEDS = [1696, 2, 3]
MINING = ["wealth", "egalitarian"]
SCALES = [1, 100, 1000]
RULES = ["1790", "node"]
BAR = 0.05

LABEL = {
    "R0": "R0 baseline (same commit)",
    "R2": "R2 inverse-factor (same commit)",
    "R3": "R3 ticket-price (same commit)",
    "S1": "S1 factor-dependent ticket price",
    "S2": "S2 R2 weight + 1% churn fee",
    "S3a": "S3a R3 + demurrage to block producers",
    "S3b": "S3b R3 + settle on spend + demurrage to block producers",
    "S4a": "S4a 30-day window, uniform",
    "S4b": "S4b 30-day window, value x (7 - factor)",
}
ORDER = ["R0", "R2", "R3", "S1", "S2", "S3a", "S3b", "S4a", "S4b"]

def tagged_records():
    if os.path.isfile(source):
        for line in open(source):
            if line.strip():
                r = json.loads(line)
                yield r["tag"], r
        return
    for f in glob.glob(os.path.join(source, "*.json")):
        tag = os.path.basename(f)[:-5]
        for line in open(f):
            if line.strip():
                yield tag, json.loads(line)


recs = {}
for tag, r in tagged_records():
    cfg, mining, scale, seed, rule, run = tag.split("__")
    recs[(cfg, mining, int(scale[1:]), int(seed[1:]), rule, run, r["yr"])] = r


def off_cfg(cfg):
    # Every shape's switches are inert with the mechanism off (tested), and
    # settle-on-spend only starts with a positive demurrage rate, which the
    # counterfactual never has: one counterfactual serves every shape.
    return "OFF"


def dgini(cfg, mining, scale, seed, rule, yr):
    on = recs.get((cfg, mining, scale, seed, rule, "on", yr))
    off = recs.get((off_cfg(cfg), mining, scale, seed, rule, "off", yr))
    if on is None or off is None:
        return None
    return off["gini"] - on["gini"]


def f4(x):
    return "n/a" if x is None else f"{x:+.4f}"


def mean(xs):
    xs = [x for x in xs if x is not None]
    return sum(xs) / len(xs) if len(xs) == len(SEEDS) else None


def cell(cfg, mining, scale, rule):
    per = {s: (dgini(cfg, mining, scale, s, rule, 10), dgini(cfg, mining, scale, s, rule, 20))
           for s in SEEDS}
    m10 = mean([per[s][0] for s in SEEDS])
    m20 = mean([per[s][1] for s in SEEDS])
    ok = None if m10 is None or m20 is None else (m10 > BAR and m20 > BAR)
    return per, m10, m20, ok


out = []
for rule in RULES:
    rule_name = ("#1790 candidate rule: spent outputs leave the draw" if rule == "1790"
                 else "node candidate rule: spent outputs stay eligible (--spent-eligible)")
    out.append(f"#### ΔGini summary, {rule_name}\n")
    out.append("| shape | mean ΔGini range, wealth | mean ΔGini range, egalitarian | best 3-seed mean | cells passing |")
    out.append("|---|---|---|---|---|")
    for cfg in ORDER:
        ranges = {}
        best = None
        passing = 0
        complete = True
        for mining in MINING:
            vals = []
            for scale in SCALES:
                _, m10, m20, ok = cell(cfg, mining, scale, rule)
                if ok is None:
                    complete = False
                    continue
                vals += [m10, m20]
                passing += 1 if ok else 0
            ranges[mining] = (min(vals), max(vals)) if vals else None
            if vals:
                best = max(vals) if best is None else max(best, max(vals))
        rng = lambda m: "n/a" if ranges[m] is None else f"{ranges[m][0]:+.4f} to {ranges[m][1]:+.4f}"
        out.append(f"| {LABEL[cfg]} | {rng('wealth')} | {rng('egalitarian')} | {f4(best)} | "
                   f"{passing}/6{'' if complete else ' (incomplete)'} |")
    out.append("")

for rule in RULES:
    out.append(f"#### ΔGini by cell, {'#1790 rule' if rule == '1790' else 'node rule'}\n")
    out.append("| shape | mining | tx scale | seed 1696 (10 / 20 yr) | seed 2 | seed 3 | 3-seed mean | > 0.05 |")
    out.append("|---|---|---:|---|---|---|---|---|")
    for cfg in ORDER:
        for mining in MINING:
            for scale in SCALES:
                per, m10, m20, ok = cell(cfg, mining, scale, rule)
                cols = " | ".join(f"{f4(per[s][0])} / {f4(per[s][1])}" for s in SEEDS)
                verdict = "n/a" if ok is None else ("PASS" if ok else "FAIL")
                out.append(f"| {cfg} | {mining} | {scale}x | {cols} | {f4(m10)} / {f4(m20)} | {verdict} |")
    out.append("")

COH = ["small", "middle", "merchant", "whale", "strategic"]
for rule in RULES:
    out.append(f"#### Where the money goes (seed 1696, 10 years, {'#1790 rule' if rule == '1790' else 'node rule'})\n")
    out.append("Cohort shares of lottery payouts; totals over 10 years in BTH. `ticket factor` = mean factor weight of the tickets bought (S1).\n")
    out.append("| shape | mining | tx scale | small | middle | merchant | whales (9) | strategic | lottery paid | demurrage | ticket burn | churn fee | to miners | ticket factor |")
    out.append("|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|")
    for cfg in ORDER:
        for mining in MINING:
            for scale in SCALES:
                r = recs.get((cfg, mining, scale, 1696, rule, "on", 10))
                if r is None:
                    continue
                c = r["cohorts"]
                sh = " | ".join(f"{100 * c[k]['lottery']:.1f}%" for k in COH)
                tf = r.get("ticket_mean_factor")
                out.append(
                    f"| {cfg} | {mining} | {scale}x | {sh} | {r['lottery_bth']:.3e} | "
                    f"{r['demurrage_bth']:.3e} | {r['ticket_burn_bth']:.3e} | "
                    f"{r.get('value_fee_bth', 0.0):.3e} | {r.get('miner_dividend_bth', 0.0):.3e} | "
                    f"{'—' if tf is None else f'{tf:.3f}'} |")
    out.append("")

out.append("#### Fast-path validation (seed 1696): the S4 incremental-weight path at the node's window\n")
out.append("| path | rule | mining | tx scale | default path ΔGini (10 / 20 yr) | fast path ΔGini (10 / 20 yr) | |Δ| max |")
out.append("|---|---|---|---:|---|---|---:|")
worst = 0.0
for v, ref in (("V-R0", "R0"), ("V-R2", "R2")):
    for rule in RULES:
        for mining in MINING:
            for scale in SCALES:
                a = [dgini(ref, mining, scale, 1696, rule, y) for y in (10, 20)]
                b = [dgini(v, mining, scale, 1696, rule, y) for y in (10, 20)]
                if None in a or None in b:
                    continue
                d = max(abs(a[0] - b[0]), abs(a[1] - b[1]))
                worst = max(worst, d)
                out.append(f"| {ref} | {rule} | {mining} | {scale}x | {f4(a[0])} / {f4(a[1])} | "
                           f"{f4(b[0])} / {f4(b[1])} | {d:.4f} |")
out.append(f"\nLargest fast-vs-default difference: {worst:.4f}.\n")

print("\n".join(out))
