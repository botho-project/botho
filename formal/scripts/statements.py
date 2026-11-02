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
      : ∀ (cap a b : Nat), @LE.le.{0} Nat instLENat (Botho.satAdd cap a b) cap
      def Botho.satAdd : Nat → Nat → Nat → Nat
        := fun (cap a b : Nat) => @Min.min.{0} Nat instMinNat (@HAdd.hAdd.{0, 0, 0} ...

The hash is SHA-256 over AxiomReport.lean's exact serialization of the
theorem's statement closure: its type and every library declaration that
type reaches, with the bodies of definitions (see `statementClosure` in
AxiomReport.lean). The indented lines are the same closure pretty-printed,
for the reviewer, with every argument, universe level and numeral type
shown (AxiomReport.lean, `ppOptions`; #1824). The whole block is compared
as text, so the readable part cannot drift from the hash; the hash is what
binds the exact terms (the text still hides proofs, as `⋯`).

A theorem may have more than one block in the record. That is how a
statement changes: a first pull request adds the new block next to the old
one (the model is unchanged, so its verify job passes against the old
block); once that is on the base branch, the pull request that changes the
model passes against the new block and may delete the old one.

Removing a theorem from the manifest takes two pull requests too (#1824).
A theorem listed in the BASE commit's Theorems.lean that has a block in the
base record stays bound: `check` fails if the pull request's manifest no
longer lists it, unless the base record holds a line

    retired Botho.old_theorem

So a first pull request adds that line (reviewed like any record change;
the theorem is still listed, and still has to match its block), and a
second one removes the theorem from Theorems.lean and may delete its block
and the `retired` line. A theorem that is in the base record but not in the
base manifest is a pending addition, not a removal, and is not checked.

Usage (python3 3.11 or later; stdlib only):

  statements.py check REPORT RECORD NAMES BASE_NAMES
      The verify job. NAMES is a file with one manifest theorem per line
      (the pull request's Theorems.lean), BASE_NAMES the same for the base
      commit's Theorems.lean, REPORT the output of AxiomReport.lean for
      NAMES, RECORD the base commit's statements.txt. Exit 1 if RECORD is
      missing or malformed, if any theorem has no block equal to its
      compiled one, or if a bound base theorem left the manifest without a
      `retired` line in RECORD.
      Prints the compiled block of each failing theorem (a proposal for a
      reviewer, computed from the artifact under test, not a fact).
  statements.py render REPORT NAMES
      Print a complete statements.txt for the theorems in NAMES.
  statements.py diff REPORT RECORD NAMES
      As `check` without BASE_NAMES (so without the removal check), and
      also list blocks of RECORD that match no compiled theorem (pending or
      stale) and its `retired` lines. Used by propose-statements.sh.
"""
import hashlib
import os
import re
import sys

HEADER_RE = re.compile(r"^theorem (\S+) sha256:([0-9a-f]{64})$")
RETIRED_RE = re.compile(r"^retired (\S+)$")

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
# the library declarations it depends on, pretty-printed (every argument and
# universe level shown, no notation, proofs as `⋯`). HASH is SHA-256 over the
# exact serialization of the same declarations (AxiomReport.lean,
# `statementClosure`). Generate blocks with scripts/propose-statements.sh;
# review the text against the sources.
#
# A line `retired NAME` lets a later pull request drop theorem NAME from
# Theorems.lean (#1824). Without it, the verify job fails a pull request that
# stops listing a theorem the base commit lists and this record binds.
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
    """(name -> [block, ...], retired names); exits on a missing or
    malformed record."""
    if not os.path.isfile(path) or os.path.islink(path):
        sys.exit(f"FAIL: statement record: {path} is missing or not a regular file "
                 "(the base commit must have formal/statements.txt)")
    try:
        with open(path, encoding="utf-8") as fh:
            rows = fh.read().split("\n")
    except (OSError, UnicodeDecodeError) as e:
        sys.exit(f"FAIL: statement record: cannot read {path}: {e}")
    blocks, retired, cur = {}, set(), None
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
        r = RETIRED_RE.match(line)
        if r:
            retired.add(r.group(1))
            continue
        m = HEADER_RE.match(line)
        if not m:
            sys.exit(f"FAIL: statement record: {path}:{lineno}: not a comment, a "
                     f"blank line, a `retired NAME` line or a block: {line[:100]!r}")
        cur = (m.group(1), [line])
    if cur is not None:
        blocks.setdefault(cur[0], []).append(tuple(cur[1]))
    return blocks, retired


def compare(report_path, record_path, names_path, base_names_path, list_unmatched):
    names = read_names(names_path)
    compiled = parse_report(report_path)
    record, retired = parse_record(record_path)
    errors, proposals, ok = [], [], []
    matched = set()
    # Unbinding by deletion (#1824): a theorem the base lists and the base
    # record binds must stay listed, unless the base record retires it.
    if base_names_path is not None:
        listed = set(names)
        for name in read_names(base_names_path):
            if name in record and name not in listed and name not in retired:
                errors.append(
                    f"FAIL: statement: {name}: listed in the base Theorems.lean and "
                    f"bound by the statement record ({record_path}), but this pull "
                    "request's Theorems.lean no longer lists it; removing a bound "
                    f"theorem takes two pull requests: first add `retired {name}` to "
                    "statements.txt, then drop the theorem")
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
            # Say whether the reviewer can see the difference (#1824): the
            # text under the header is what a reviewer reads.
            if any(c[1:] == block[1:] for c in candidates):
                seen = ("the readable text is identical to a recorded block, so the "
                        "difference is only in the hash")
            else:
                seen = "the readable text differs from every recorded block"
            errors.append(f"FAIL: statement: {name}: the compiled statement "
                          f"({block[0].split()[-1][:19]}...) matches no entry in the "
                          f"statement record ({record_path}; recorded: {have}); {seen}")
        proposals.append(block)
    for line in ok:
        print(line)
    if list_unmatched:
        for name, blocks in sorted(record.items()):
            for b in blocks:
                if (name, b) not in matched:
                    print(f"  note: record block {b[0]} matches no compiled theorem "
                          "(pending or stale)")
        for name in sorted(retired):
            state = "still listed" if name in names else "no longer listed; stale"
            print(f"  note: record retires {name} ({state})")
    if errors:
        for e in errors:
            print(e, file=sys.stderr)
        if proposals:
            print("  Compiled blocks (proposals for a reviewer; computed from the "
                  "library under test):", file=sys.stderr)
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
    if len(argv) == 6 and argv[1] == "check":
        return compare(argv[2], argv[3], argv[4], argv[5], False)
    if len(argv) == 5 and argv[1] == "diff":
        return compare(argv[2], argv[3], argv[4], None, True)
    if len(argv) == 4 and argv[1] == "render":
        return render(argv[2], argv[3])
    print(__doc__.split("Usage", 1)[1], file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
