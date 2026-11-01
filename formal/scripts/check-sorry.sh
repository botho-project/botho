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
#           `#eval`, `#eval!`, `run_cmd`, `run_elab`, `run_meta`, `run_tac`,
#           `by_elab`, `#guard_msgs`, `initialize`, `builtin_initialize`;
#         - a metaprogramming command (`macro`, `macro_rules`, `syntax`,
#           `declare_syntax_cat`, `elab`, `elab_rules`), which can expand to
#           any of the above without naming it;
#         - inside `@[...]` or `attribute [...]`, an attribute that runs code
#           at build or import time or extends the elaborator: `init`,
#           `builtin_*`, `extern`, `implemented_by`, `export`, `macro`,
#           `*_elab`, `*_parser`, `tactic`, `delab`, ... (see FORBIDDEN_ATTRS);
#         - the option `debug.skipKernelTC` (admits unchecked declarations).
#
# The scan runs first, before `lake build`: model code runs during the build
# (for example an `@[init]` initializer) and could rewrite the sources to hide
# itself. The whole script is one function, so bash parses it completely
# before running anything. See "Limits" in formal/README.md.
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

main() {
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
files=()

# (c) runs first, before any `lake` call, so code that runs during the build
# cannot edit what is scanned.
if [[ "$scan" == 1 ]]; then
  echo "== sorry gate: source scan ($root)"
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
    "#eval", "#eval!", "run_cmd", "run_elab", "run_meta", "run_tac", "by_elab",
    "#guard_msgs", "initialize", "builtin_initialize",
    # Metaprogramming: a macro can expand to any of the commands above built
    # from raw syntax nodes, so the banned keyword never appears in the source.
    "macro", "macro_rules", "syntax", "declare_syntax_cat", "elab", "elab_rules",
)
# Attributes that run code when a module is built or imported, bind
# declarations to native or substitute code, or extend the elaborator,
# parser or pretty-printer. Matched only inside `@[...]` and `attribute [...]`
# (a bare `init` is a common variable name). `@[init f] opaque x : Unit` runs
# `f : IO Unit` whenever the module is imported, with no `import Lean`.
FORBIDDEN_ATTRS = (
    "init", "extern", "implemented_by", "export", "macro", "command_elab",
    "term_elab", "tactic", "delab", "app_unexpander", "command_parser",
    "term_parser", "tactic_parser", "formatter", "parenthesizer",
    "combinator_formatter", "combinator_parenthesizer", "env_linter",
    "code_action_provider", "server_rpc_method", "widget_module",
    "run_builtin_parser_attribute_hooks", "run_parser_attribute_hooks",
)
FORBIDDEN_ATTR_PREFIXES = ("builtin_",)  # builtin_init, builtin_command_elab, ...
FORBIDDEN_ATTR_SUFFIXES = (
    "_elab", "_parser", "_delab", "_unexpander", "_formatter",
    "_parenthesizer", "_linter", "_code_action", "_rpc_method",
)
# Modifiers that may precede an attribute name inside the brackets.
ATTR_MODIFIERS = ("scoped", "local")
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


def forbidden_attr(name):
    while name.startswith(ROOT_PREFIX):
        name = name[len(ROOT_PREFIX):]
    return (
        name in FORBIDDEN_ATTRS
        or name.startswith(FORBIDDEN_ATTR_PREFIXES)
        or name.endswith(FORBIDDEN_ATTR_SUFFIXES)
    )


ATTR_OPEN = re.compile(r"@\[|(?<![A-Za-z0-9_'!?.])attribute\s*\[")


def attr_offenses(text):
    """(line, attribute) for each forbidden attribute named in an `@[...]` or
    `attribute [...]` list of the stripped source TEXT. Every comma-separated
    entry is checked, after `scoped` / `local` / `-` prefixes."""
    found = []
    for m in ATTR_OPEN.finditer(text):
        i, depth, start, entries = m.end(), 0, m.end(), []
        while i < len(text):
            c = text[i]
            if c in "([{":
                depth += 1
            elif c in ")}" or (c == "]" and depth > 0):
                depth -= 1
            elif c == "]":
                break
            elif c == "," and depth == 0:
                entries.append((start, text[start:i]))
                start = i + 1
            i += 1
        entries.append((start, text[start:i]))
        for pos, entry in entries:
            toks = IDENT.findall(entry.replace("-", " "))
            while toks and toks[0] in ATTR_MODIFIERS:
                toks = toks[1:]
            if toks and forbidden_attr(toks[0]):
                found.append((text.count("\n", 0, pos + len(entry) - len(entry.lstrip())) + 1, toks[0]))
    return found


bad = 0
for path in sys.argv[1:]:
    with open(path, encoding="utf-8") as fh:
        raw = fh.read()
    raw_lines = raw.splitlines()
    reported = set()
    for interp in (False, True):
        # `«sorryAx»` and `@[«init» f]` name the same things as the bare
        # tokens, so guillemets are dropped (line numbers are unchanged).
        stripped = strip(raw, interp).replace("\u00ab", "").replace("\u00bb", "")
        for lineno, name in attr_offenses(stripped):
            if (lineno, "@" + name) in reported:
                continue
            reported.add((lineno, "@" + name))
            original = raw_lines[lineno - 1] if lineno <= len(raw_lines) else ""
            print(f"{path}:{lineno}: forbidden attribute '{name}': {original.strip()}")
            bad = 1
        for lineno, line in enumerate(stripped.splitlines(), 1):
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
fi

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

if [[ "$fail" != 0 ]]; then
  exit 1
fi
if [[ "$scan" == 1 ]]; then
  echo "OK: sorry gate passed (${#files[@]} files scanned)"
else
  echo "OK: sorry gate passed (no source scan)"
fi
}

main "$@"
