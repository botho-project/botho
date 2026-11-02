#!/usr/bin/env python3
"""Statement record for the formal gates (#1805).

The verify job checks compiled .olean files that an untrusted build uploaded.
Nothing else ties them to the sources, so each theorem listed in
Theorems.lean is bound to a reviewed record, formal/statements.txt. The
verify job reads that record from the BASE commit (never from the pull
request), recomputes each theorem's entry from the compiled library, and
fails unless the base record holds an identical entry.

An entry ("block") looks like this:

    theorem Botho.satAdd_le_cap sha256:<64 hex digits>
      : ∀ (cap a b : Nat), instLENat.le (Botho.satAdd cap a b) cap
      def Botho.satAdd : Nat → Nat → Nat → Nat
        := fun cap a b => instMinNat.min (instHAdd.hAdd a b) cap

The hash is SHA-256 over AxiomReport.lean's exact serialization of the
theorem's statement closure: its type and every library declaration that
type reaches, with the bodies of definitions (see `statementClosure` in
AxiomReport.lean). The indented lines are the same closure pretty-printed,
for the reviewer. The whole block is compared as text, so the readable part
cannot drift from the hash; the hash is what binds the exact terms (the
pretty-printer hides implicit arguments and proofs).

A theorem may have more than one block in the record. That is how a
statement changes: a first pull request adds the new block next to the old
one (the model is unchanged, so its verify job passes against the old
block); once that is on the base branch, the pull request that changes the
model passes against the new block and may delete the old one.

Usage (python3 3.11 or later; stdlib only):

  statements.py check REPORT RECORD NAMES
      The verify job. NAMES is a file with one manifest theorem per line,
      REPORT the output of AxiomReport.lean for those names, RECORD the
      base commit's statements.txt. Exit 1 if RECORD is missing or
      malformed, or if any theorem has no block equal to its compiled one.
      Prints the compiled block of each failing theorem (a proposal for a
      reviewer, computed from the artifact under test, not a fact).
  statements.py render REPORT NAMES
      Print a complete statements.txt for the theorems in NAMES.
  statements.py diff REPORT RECORD NAMES
      As `check`, and also list blocks of RECORD that match no compiled
      theorem (pending or stale). Used by propose-statements.sh.
"""
import hashlib
import os
import re
import sys

HEADER_RE = re.compile(r"^theorem (\S+) sha256:([0-9a-f]{64})$")

RECORD_HEADER = """\
# Statement record for the formal model (#1805). Reviewed, base-owned.
#
# The verify job (scripts/verify-artifacts.sh) reads this file from the BASE
# commit of a pull request, recomputes the block of every theorem listed in
# Theorems.lean from the compiled library, and fails unless this file holds
# an identical block. A pull request's own edits to this file do not count
# until they are merged. See "Statement binding" in formal/README.md.
#
# Each block: `theorem NAME sha256:HASH`, then the theorem's statement and
# the library declarations it depends on, pretty-printed (instances shown,
# no notation). HASH is SHA-256 over the exact serialization of the same
# declarations (AxiomReport.lean, `statementClosure`). Generate blocks with
# scripts/propose-statements.sh; review the text against the sources.
"""


def unescape(s):
    """Inverse of AxiomReport.lean's `escField`."""
    out, i = [], 0
    while i < len(s):
        c = s[i]
        if c == "\\" and i + 1 < len(s):
            nxt = s[i + 1]
            out.append({"n": "\n", "t": "\t", "r": "\r", "\\": "\\"}.get(nxt, "\\" + nxt))
            i += 2
        else:
            out.append(c)
            i += 1
    return "".join(out)


def read_names(path):
    with open(path, encoding="utf-8") as fh:
        return [n for n in fh.read().split("\n") if n]


def parse_report(path):
    """name -> (serialization, [(kind, decl, type, value), ...])"""
    stmts, pps = {}, {}
    with open(path, encoding="utf-8") as fh:
        for row in fh.read().split("\n"):
            parts = row.split("\t")
            if parts[0] == "STMT" and len(parts) == 3:
                if parts[1] in stmts:
                    sys.exit(f"FAIL: statement: two STMT records for {parts[1]}")
                stmts[parts[1]] = parts[2]
            elif parts[0] == "STMTPP" and len(parts) == 6:
                pps.setdefault(parts[1], []).append(
                    (parts[2], unescape(parts[3]), unescape(parts[4]), unescape(parts[5])))
            elif parts[0] in ("STMT", "STMTPP"):
                sys.exit(f"FAIL: statement: malformed report line: {row[:120]!r}")
    return {n: (s, pps.get(n, [])) for n, s in stmts.items()}


def text_lines(prefix, text):
    """`text` (possibly multi-line) after `prefix`; continuation lines are
    indented further so that they can never start a block."""
    lines = text.split("\n")
    out = [prefix + lines[0]]
    out += ["      " + ln for ln in lines[1:]]
    return out


def render_block(name, ser, pps):
    digest = hashlib.sha256(ser.encode("utf-8")).hexdigest()
    lines = [f"theorem {name} sha256:{digest}"]
    for i, (kind, decl, ty, val) in enumerate(pps):
        if i == 0:
            lines += text_lines("  : ", ty)
            if val:
                lines += text_lines("  := ", val)
        elif kind == "recursor":
            lines.append(f"  recursor {decl}")
        else:
            lines += text_lines(f"  {kind} {decl} : ", ty)
            if val:
                lines += text_lines("    := ", val)
    return tuple(ln.rstrip() for ln in lines)


def parse_record(path):
    """name -> [block, ...]; exits on a missing or malformed record."""
    if not os.path.isfile(path) or os.path.islink(path):
        sys.exit(f"FAIL: statement record: {path} is missing or not a regular file "
                 "(the base commit must have formal/statements.txt)")
    try:
        with open(path, encoding="utf-8") as fh:
            rows = fh.read().split("\n")
    except (OSError, UnicodeDecodeError) as e:
        sys.exit(f"FAIL: statement record: cannot read {path}: {e}")
    blocks, cur = {}, None
    for lineno, raw in enumerate(rows, 1):
        line = raw.rstrip()
        if line.startswith("  ") and cur is not None:
            cur[1].append(line)
            continue
        if cur is not None:
            blocks.setdefault(cur[0], []).append(tuple(cur[1]))
            cur = None
        if not line or line.startswith("#"):
            continue
        m = HEADER_RE.match(line)
        if not m:
            sys.exit(f"FAIL: statement record: {path}:{lineno}: not a comment, a "
                     f"blank line or a block: {line[:100]!r}")
        cur = (m.group(1), [line])
    if cur is not None:
        blocks.setdefault(cur[0], []).append(tuple(cur[1]))
    return blocks


def compare(report_path, record_path, names_path, list_unmatched):
    names = read_names(names_path)
    compiled = parse_report(report_path)
    record = parse_record(record_path)
    errors, proposals, ok = [], [], []
    matched = set()
    for name in names:
        if name not in compiled:
            errors.append(f"FAIL: statement: {name}: no compiled statement in the report "
                          "(not a declaration of the compiled library?)")
            continue
        block = render_block(name, *compiled[name])
        candidates = record.get(name, [])
        if block in candidates:
            matched.add((name, block))
            ok.append(f"  {name}: statement {block[0].split()[-1][:19]}... matches the record")
            continue
        if not candidates:
            errors.append(f"FAIL: statement: {name}: no entry in the statement record "
                          f"({record_path}); a new theorem needs its block on the base "
                          "branch first")
        else:
            have = ", ".join(c[0].split()[-1][:19] + "..." for c in candidates)
            errors.append(f"FAIL: statement: {name}: the compiled statement "
                          f"({block[0].split()[-1][:19]}...) matches no entry in the "
                          f"statement record ({record_path}; recorded: {have})")
        proposals.append(block)
    for line in ok:
        print(line)
    if list_unmatched:
        for name, blocks in sorted(record.items()):
            for b in blocks:
                if (name, b) not in matched:
                    print(f"  note: record block {b[0]} matches no compiled theorem "
                          "(pending or stale)")
    if errors:
        for e in errors:
            print(e, file=sys.stderr)
        print("  Compiled blocks (proposals for a reviewer; computed from the library "
              "under test):", file=sys.stderr)
        for b in proposals:
            print("", file=sys.stderr)
            for ln in b:
                print("    " + ln, file=sys.stderr)
        return 1
    print(f"OK: statements match the record ({len(names)} theorems)")
    return 0


def render(report_path, names_path):
    names = read_names(names_path)
    compiled = parse_report(report_path)
    out = [RECORD_HEADER]
    for name in names:
        if name not in compiled:
            sys.exit(f"FAIL: statement: {name}: no compiled statement in the report")
        out.append("\n".join(render_block(name, *compiled[name])) + "\n")
    sys.stdout.write("\n".join(out))
    return 0


def main(argv):
    if len(argv) == 5 and argv[1] == "check":
        return compare(argv[2], argv[3], argv[4], False)
    if len(argv) == 5 and argv[1] == "diff":
        return compare(argv[2], argv[3], argv[4], True)
    if len(argv) == 4 and argv[1] == "render":
        return render(argv[2], argv[3])
    print(__doc__.split("Usage", 1)[1], file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
