#!/usr/bin/env bash
# Sorry gate for the Botho formal model (#1705, hardened in #1724 and #1770).
#
# Fails (exit 1) if:
#   (a) `lake build` fails or warns "declaration uses `sorry`" (any quote
#       style, so the check survives Lean changing its message);
#   (b) the sweep finds a declaration of the Botho library that reaches
#       `sorryAx`. AxiomReport.lean walks the compiled declarations directly,
#       so this holds even when the warning in (a) was suppressed (for example
#       by `#guard_msgs in theorem ... := by stop ...`); or
#   (c) a source scan of Botho.lean, Botho/**/*.lean and Theorems.lean finds,
#       outside comments and string literals (but inside `{...}` string
#       interpolation):
#         - a proof escape: `sorry`, `sorryAx`, `admit`, `native_decide`;
#         - a command that runs code or hides messages at compile time:
#           `#eval`, `#eval!`, `run_cmd`, `run_elab`, `run_meta`,
#           `#guard_msgs`, `initialize`, `builtin_initialize`;
#         - the option `debug.skipKernelTC` (admits unchecked declarations).
#
# `native_decide` trusts the compiler and adds compiler-trust axioms (such as
# `Lean.ofReduceBool`).
# It is allowed on one line only when that line carries the marker comment
# `-- formal-gate: allow native_decide`. The axiom gate still has to accept the
# resulting axiom, so the marker alone is not enough to pass CI.
#
# Usage: check-sorry.sh [--no-build] [--no-scan] [--no-sweep] [FORMAL_ROOT]
#   FORMAL_ROOT defaults to the directory above this script.
#   --no-build  skip (a) and (b); scan the source only.
#   --no-scan   skip (c).
#   --no-sweep  skip (b).
#   The self-test uses these to prove each check works on its own.
# Environment: LAKE overrides the lake binary (default: lake).
set -euo pipefail

build=1
scan=1
sweep=1
while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-build) build=0; sweep=0; shift ;;
    --no-scan) scan=0; shift ;;
    --no-sweep) sweep=0; shift ;;
    *) break ;;
  esac
done
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="${1:-$(cd "$here/.." && pwd)}"
root="$(cd "$root" && pwd)"
lake="${LAKE:-lake}"
# The library the gates check: `Botho` and every `Botho.*` module.
gate_lib=Botho
# shellcheck disable=SC1091  # gate-common.sh sits next to this script
. "$here/gate-common.sh"
fail=0
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

if [[ "$build" == 1 ]]; then
  lakefile_preflight
  echo "== sorry gate: lake build ($root)"
  log="$tmp/build.log"
  if ! (cd "$root" && "$lake" build) >"$log" 2>&1; then
    cat "$log"
    echo "FAIL: lake build failed" >&2
    exit 1
  fi
  cat "$log"
  if grep -Eq "declaration uses ['\`\"]?sorry" "$log"; then
    echo "FAIL: lake build reported a declaration that uses 'sorry'" >&2
    fail=1
  fi
fi

if [[ "$sweep" == 1 ]]; then
  echo "== sorry gate: sweep $gate_lib for sorryAx (AxiomReport.lean)"
  run_report "$tmp/report"
  if grep -q $'^SORRY\t' "$tmp/report"; then
    grep $'^SORRY\t' "$tmp/report" | cut -f2 | sed 's/^/  reaches sorryAx: /' >&2
    echo "FAIL: a declaration in $gate_lib reaches sorryAx (see above)" >&2
    fail=1
  fi
fi

if [[ "$scan" == 0 ]]; then
  if [[ "$fail" != 0 ]]; then exit 1; fi
  echo "OK: sorry gate passed (no source scan)"
  exit 0
fi

echo "== sorry gate: source scan ($root)"
files=()
[[ -f "$root/Botho.lean" ]] && files+=("$root/Botho.lean")
[[ -f "$root/Theorems.lean" ]] && files+=("$root/Theorems.lean")
if [[ -d "$root/Botho" ]]; then
  while IFS= read -r f; do files+=("$f"); done \
    < <(find "$root/Botho" -name '*.lean' -type f | LC_ALL=C sort)
fi
if [[ ${#files[@]} -eq 0 ]]; then
  echo "FAIL: no Lean sources found under $root" >&2
  exit 1
fi

if ! python3 - "${files[@]}" <<'PY'
import re
import sys

sys.setrecursionlimit(10000)

# Proof escapes, matched as whole identifiers (after `_root_.` and any
# trailing `.` from universe syntax such as `sorryAx.{0}` are removed).
FORBIDDEN = ("sorry", "sorryAx", "admit", "native_decide")
# Commands that run code or swallow messages at compile time. A model has no
# use for them, and they are the tools for faking gate output (printing a
# fake `#print axioms` line, hiding "declaration uses sorry").
FORBIDDEN_COMMANDS = (
    "#eval", "#eval!", "run_cmd", "run_elab", "run_meta", "#guard_msgs",
    "initialize", "builtin_initialize",
)
# Forbidden as any dotted component (catches `set_option debug.skipKernelTC`).
FORBIDDEN_COMPONENTS = ("skipKernelTC",)
ALLOW_MARKER = "-- formal-gate: allow native_decide"
IDENT = re.compile(r"#?[A-Za-z_][A-Za-z0-9_'!?.]*")
# A Lean char literal starting at a `'`: one plain character, or an escape
# (`\n`, `\'`, `\xHH`, `\uHHHH`, ...). Only tried where the `'` cannot be
# part of an identifier (primed names such as `h'` keep their prime).
CHAR_LIT = re.compile(r"'(?:\\(?:x[0-9A-Fa-f]{2}|u[0-9A-Fa-f]{4}|.)|[^\\'\n])'")
# A Lean raw string opener: `r"`, `r#"`, `r##"`, ... (closed by `"` followed by
# the same number of `#`; no escapes inside, no interpolation).
RAW_OPEN = re.compile(r'r(#*)"')
# `_root_.sorryAx` (and friends) name the same constant as the bare token.
ROOT_PREFIX = "_root_."


def ident_char(ch):
    return ch.isalnum() or ch in "_'!?."


def blank(text):
    return "".join("\n" if ch == "\n" else " " for ch in text)


def strip(src, interp):
    """Replace comments and string literals with spaces, keeping newlines so
    line numbers survive. Handles nested /- -/ block comments.

    With interp=True every ordinary string literal is read as an interpolated
    string: `{...}` inside it is code (scanned), and may itself contain nested
    strings. Lean decides from the surrounding syntax (`s!`, `m!`, `f!`,
    `throwError`, `dbg_trace`, user macros, ...) whether a literal is
    interpolated, which a token scan cannot know. So the caller scans both
    readings and reports the union."""
    out = []
    n = len(src)

    def code(i, in_braces):
        depth = 0
        while i < n:
            c = src[i]
            two = src[i:i + 2]
            if two == "/-":
                j, nest = i + 2, 1
                while j < n and nest:
                    if src.startswith("/-", j):
                        nest += 1
                        j += 2
                    elif src.startswith("-/", j):
                        nest -= 1
                        j += 2
                    else:
                        j += 1
                out.append(blank(src[i:j]))
                i = j
            elif two == "--":
                j = src.find("\n", i)
                j = n if j < 0 else j
                out.append(blank(src[i:j]))
                i = j
            elif c == "'" and not (i > 0 and ident_char(src[i - 1])) \
                    and CHAR_LIT.match(src, i):
                # Char literal such as '"' -- blank it so its quote cannot open
                # a phantom string that hides the code after it.
                m = CHAR_LIT.match(src, i)
                out.append(" " * (m.end() - i))
                i = m.end()
            elif c == "r" and not (i > 0 and ident_char(src[i - 1])) \
                    and RAW_OPEN.match(src, i):
                # Raw string r#"..."# (any number of #, including none).
                m = RAW_OPEN.match(src, i)
                close = '"' + m.group(1)
                end = src.find(close, m.end())
                end = n if end < 0 else end + len(close)
                out.append(blank(src[i:end]))
                i = end
            elif c == '"':
                out.append(" ")
                i = string(i + 1)
            elif in_braces and c == "{":
                depth += 1
                out.append(c)
                i += 1
            elif in_braces and c == "}":
                if depth == 0:
                    return i
                depth -= 1
                out.append(c)
                i += 1
            else:
                out.append(c)
                i += 1
        return i

    def string(i):
        while i < n and src[i] != '"':
            if src[i] == "\\" and i + 1 < n:
                out.append(blank(src[i:i + 2]))
                i += 2
            elif interp and src[i] == "{":
                out.append(" ")
                i = code(i + 1, True)
                if i < n:
                    out.append(" ")
                    i += 1
            else:
                out.append(blank(src[i]))
                i += 1
        if i < n:
            out.append(" ")
            i += 1
        return i

    code(0, False)
    return "".join(out)


def offenses(tok):
    """Forbidden names in one identifier-like token, or []."""
    if tok in FORBIDDEN_COMMANDS:
        return [tok]
    found = [p for p in tok.split(".") if p in FORBIDDEN_COMPONENTS]
    tok = tok.rstrip(".")
    while tok.startswith(ROOT_PREFIX):
        tok = tok[len(ROOT_PREFIX):]
    if tok in FORBIDDEN:
        found.append(tok)
    return found


bad = 0
for path in sys.argv[1:]:
    with open(path, encoding="utf-8") as fh:
        raw = fh.read()
    raw_lines = raw.splitlines()
    reported = set()
    for interp in (False, True):
        for lineno, line in enumerate(strip(raw, interp).splitlines(), 1):
            for tok in IDENT.findall(line):
                for name in offenses(tok):
                    original = raw_lines[lineno - 1] if lineno <= len(raw_lines) else ""
                    if name == "native_decide" and ALLOW_MARKER in original:
                        continue
                    if (lineno, name) in reported:
                        continue
                    reported.add((lineno, name))
                    print(f"{path}:{lineno}: forbidden token '{name}': {original.strip()}")
                    bad = 1
sys.exit(bad)
PY
then
  echo "FAIL: forbidden token in source (see above)" >&2
  fail=1
fi

if [[ "$fail" != 0 ]]; then
  exit 1
fi
echo "OK: sorry gate passed (${#files[@]} files scanned)"
