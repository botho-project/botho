"""Source scan for the Botho formal gates (#1705, #1724, #1770).

Usage: python3 -I gate_scan.py ROOT

Both gates run this before their first `lake` call. It fails (exit 1) unless
all of the following hold for the formal/ tree at ROOT.

1. Layout. The model is `Botho.lean` plus `Botho/**/*.lean`, and the manifest
   is `Theorems.lean`. These are regular files (no symlinks anywhere under
   `Botho/`), and no other `*.lean` file exists under ROOT outside `.lake/`
   and `scripts/`. (The lakefile, lean-toolchain and lake-manifest.json are
   pinned separately, in gate-common.sh, so Lake compiles exactly the modules
   reachable from `Botho`, from `Botho.lean` and `Botho/`.)
2. Import allow-list. The header of every scanned file is a sequence of plain
   `import M` commands where M is `Botho` or `Botho.X.Y` and resolves to a
   scanned file (`Botho.lean` or `Botho/X/Y.lean`). Nothing else may be
   imported: not `Lean`, `Std`, `Lake`, `Init.*`, Mathlib or any other
   package. `prelude`, `module` and import modifiers (`public import`,
   `meta import`, `import all`) are rejected, and so is an `import` anywhere
   after the header. Simprocs, elaborators, tactics, `deriving` handlers and
   other compiler extensions are written against the `Lean` API, so without
   `import Lean` a model cannot define one. The ways left to run code while
   the model is compiled are the prelude's (`#eval`, `initialize`, `@[init]`,
   `@[implemented_by]`, ...), and the token scan below bans those.
3. Token scan. Outside comments and string literals (but inside `{...}`
   string interpolation), no file contains:
     - a proof escape: `sorry`, `sorryAx`, `admit`, `native_decide`;
     - a command that runs code or hides messages at compile time: `#eval`,
       `#eval!`, `run_cmd`, `run_elab`, `run_meta`, `run_tac`, `by_elab`,
       `#guard_msgs`, `initialize`, `builtin_initialize`;
     - a metaprogramming command (`macro`, `macro_rules`, `syntax`,
       `declare_syntax_cat`, `elab`, `elab_rules`) or a simproc
       (`simproc`, `dsimproc`, `simproc_decl`, `dsimproc_decl`);
     - inside `@[...]` or `attribute [...]`, an attribute that runs code at
       build or import time or extends the elaborator (FORBIDDEN_ATTRS);
     - the option `debug.skipKernelTC`.
   `native_decide` is allowed on a line carrying the marker comment
   `-- formal-gate: allow native_decide` (the axiom gate must still accept the
   compiler-trust axiom it adds).

On success it prints one `scanned: PATH` line per scanned file.
"""
import os
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
    # Simprocs run user code while `simp` elaborates. They need `import Lean`,
    # which the import allow-list already rejects; this is a second layer.
    "simproc", "dsimproc", "simproc_decl", "dsimproc_decl",
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


# ---- Layout -----------------------------------------------------------------

# Directories under ROOT that hold no model sources: the build output and the
# gate scripts (scripts/AxiomReport.lean is the gate's own report program).
SKIP_DIRS = (".lake", "scripts")


def enumerate_sources(root, errors):
    """The files the gates scan: Botho.lean, Theorems.lean and every
    Botho/**/*.lean, sorted. Appends layout problems to ERRORS."""
    files = []
    for name in ("Botho.lean", "Theorems.lean"):
        path = os.path.join(root, name)
        if os.path.islink(path) or not os.path.isfile(path):
            errors.append(f"{path}: must be a regular file (not missing, not a symlink)")
        else:
            files.append(path)
    top = os.path.join(root, "Botho")
    if os.path.islink(top) or (os.path.exists(top) and not os.path.isdir(top)):
        errors.append(f"{top}: must be a directory, not a symlink or file")
    elif os.path.isdir(top):
        for dirpath, dirnames, filenames in os.walk(top, followlinks=False):
            for d in sorted(dirnames):
                if os.path.islink(os.path.join(dirpath, d)):
                    errors.append(f"{os.path.join(dirpath, d)}: symlink under Botho/")
            for f in sorted(filenames):
                path = os.path.join(dirpath, f)
                if os.path.islink(path):
                    errors.append(f"{path}: symlink under Botho/")
                elif f.endswith(".lean"):
                    if os.path.isfile(path):
                        files.append(path)
                    else:
                        errors.append(f"{path}: not a regular file")
    scanned = set(files)
    # Any other .lean file is outside the scan. With the pinned lakefile Lake
    # cannot reach it, but it has no business here, so it fails too.
    for dirpath, dirnames, filenames in os.walk(root, followlinks=False):
        if dirpath == root:
            dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        for f in filenames:
            path = os.path.join(dirpath, f)
            if f.endswith(".lean") and path not in scanned:
                errors.append(f"{path}: Lean file outside the scanned set "
                              "(Botho.lean, Botho/**/*.lean, Theorems.lean)")
    return sorted(scanned)


# ---- Import allow-list ------------------------------------------------------

# The only package a model may import is itself.
ALLOWED_IMPORT_ROOT = "Botho"
# Lean header keywords other than `import`. The module system they belong to
# (`module`, `public import`, `meta import`, `import all`) and `prelude` are
# not needed by the model, so they are rejected rather than modelled.
# `prelude` and `module` can only start a file. The modifiers can also start
# an ordinary command (`private def`), so they are header keywords only when
# an `import` follows them.
HEADER_STARTS = ("prelude", "module")
IMPORT_MODIFIERS = ("public", "meta", "private")
PLAIN_COMPONENT = re.compile(r"[A-Za-z_][A-Za-z0-9_']*\Z")
IMPORT_WORD = re.compile(r"(?<![A-Za-z0-9_'!?.«»])import(?![A-Za-z0-9_'!?»])")


def ident_start(ch):
    return ch.isalpha() or ch == "_"


def ident_rest(ch):
    return ch.isalnum() or ch in "_'!?"


class HeaderLexer:
    """A small lexer for the Lean module header: whitespace, `--` and nested
    `/- -/` comments, and identifiers whose components may be `«quoted»`.
    Anything else ends the header, as it does for Lean. Doc comments (`/--`,
    `/-!`) are tokens in Lean, not whitespace, so they end the header too."""

    def __init__(self, src):
        self.src, self.i, self.n = src, 0, len(src)

    def skip_space(self):
        src, n = self.src, self.n
        while self.i < n:
            c = src[self.i]
            if c.isspace():
                self.i += 1
            elif src.startswith("--", self.i):
                j = src.find("\n", self.i)
                self.i = n if j < 0 else j
            elif src.startswith("/-", self.i) and not (
                    src.startswith("/--", self.i) or src.startswith("/-!", self.i)):
                j, nest = self.i + 2, 1
                while j < n and nest:
                    if src.startswith("/-", j):
                        nest += 1
                        j += 2
                    elif src.startswith("-/", j):
                        nest -= 1
                        j += 2
                    else:
                        j += 1
                self.i = j
            else:
                return

    def ident(self):
        """Read an identifier at the cursor. Returns (components, quoted_any,
        start) or None when the next token is not an identifier. A component
        that is unterminated or empty yields None for that component."""
        self.skip_space()
        src, n, start = self.src, self.n, self.i
        comps, quoted = [], False
        while True:
            if self.i < n and src[self.i] == "«":
                j = src.find("»", self.i + 1)
                if j < 0:
                    comps.append(None)
                    self.i = n
                    break
                comps.append(src[self.i + 1:j])
                quoted = True
                self.i = j + 1
            elif self.i < n and ident_start(src[self.i]):
                j = self.i + 1
                while j < n and ident_rest(src[j]):
                    j += 1
                comps.append(src[self.i:j])
                self.i = j
            else:
                if not comps:
                    return None
                comps.append(None)  # trailing `.` or a bad character
                break
            if self.i < n and src[self.i] == "." and self.i + 1 < n and (
                    src[self.i + 1] == "«" or ident_start(src[self.i + 1])):
                self.i += 1
                continue
            break
        return comps, quoted, start


def line_of(src, pos):
    return src.count("\n", 0, pos) + 1


def check_imports(path, raw, root, scanned, errors):
    """Check the header of one file against the import allow-list."""
    lex = HeaderLexer(raw)
    header_end = 0
    while True:
        lex.skip_space()
        header_end = lex.i
        tok = lex.ident()
        if tok is None:
            break
        comps, quoted, start = tok
        word = comps[0] if len(comps) == 1 and not quoted else None
        if word in HEADER_STARTS:
            errors.append(f"{path}:{line_of(raw, start)}: header keyword '{word}' "
                          "is not allowed (use plain `import Botho...`)")
            continue
        if word in IMPORT_MODIFIERS:
            # `public import`, `meta import`, `public meta import`, ...
            while word in IMPORT_MODIFIERS:
                tok = lex.ident()
                word = None
                if tok is not None and len(tok[0]) == 1 and not tok[1]:
                    word = tok[0][0]
            if word != "import":
                lex.i = header_end
                break
            errors.append(f"{path}:{line_of(raw, start)}: import modifier "
                          f"'{raw[start:lex.i].split()[0]}' is not allowed "
                          "(use plain `import Botho...`)")
        elif word != "import":
            lex.i = header_end
            break
        mod = lex.ident()
        if mod is None:
            errors.append(f"{path}:{line_of(raw, start)}: `import` without a module name")
            break
        mcomps, _, mstart = mod
        shown = raw[mstart:lex.i]
        lineno = line_of(raw, mstart)
        if any(c is None or not PLAIN_COMPONENT.match(c) for c in mcomps):
            errors.append(f"{path}:{lineno}: malformed import '{shown}'")
            continue
        if mcomps[0] != ALLOWED_IMPORT_ROOT:
            errors.append(f"{path}:{lineno}: import of '{'.'.join(mcomps)}' is not "
                          f"allowed: model sources may import only "
                          f"{ALLOWED_IMPORT_ROOT} modules")
            continue
        if len(mcomps) == 1:
            target = os.path.join(root, ALLOWED_IMPORT_ROOT + ".lean")
        else:
            target = os.path.join(root, *mcomps[:-1], mcomps[-1] + ".lean")
        if target not in scanned:
            errors.append(f"{path}:{lineno}: import '{'.'.join(mcomps)}' does not "
                          f"resolve to a scanned file ({target})")
    # Lean rejects `import` after the header, but check anyway, on the source
    # with comments and strings removed so prose does not trip it.
    body = strip(raw, False)
    for m in IMPORT_WORD.finditer(body, header_end):
        errors.append(f"{path}:{line_of(raw, m.start())}: `import` after the "
                      "module header")


# ---- Token scan -------------------------------------------------------------


def scan_tokens(path, raw, errors):
    raw_lines = raw.splitlines()
    reported = set()
    for interp in (False, True):
        # `«sorryAx»` and `@[«init» f]` name the same things as the bare
        # tokens, so guillemets are dropped (line numbers are unchanged).
        stripped = strip(raw, interp).replace("«", "").replace("»", "")
        for lineno, name in attr_offenses(stripped):
            if (lineno, "@" + name) in reported:
                continue
            reported.add((lineno, "@" + name))
            original = raw_lines[lineno - 1] if lineno <= len(raw_lines) else ""
            errors.append(f"{path}:{lineno}: forbidden attribute '{name}': {original.strip()}")
        for lineno, line in enumerate(stripped.splitlines(), 1):
            for tok in IDENT.findall(line):
                for name in offenses(tok):
                    original = raw_lines[lineno - 1] if lineno <= len(raw_lines) else ""
                    if name == "native_decide" and ALLOW_MARKER in original:
                        continue
                    if (lineno, name) in reported:
                        continue
                    reported.add((lineno, name))
                    errors.append(f"{path}:{lineno}: forbidden token '{name}': {original.strip()}")


def main():
    if len(sys.argv) != 2:
        sys.exit("usage: gate_scan.py ROOT")
    root = os.path.realpath(sys.argv[1])
    errors = []
    files = enumerate_sources(root, errors)
    scanned = set(files)
    for path in files:
        try:
            with open(path, encoding="utf-8") as fh:
                raw = fh.read()
        except (OSError, UnicodeDecodeError) as e:
            errors.append(f"{path}: cannot read as UTF-8: {e}")
            continue
        check_imports(path, raw, root, scanned, errors)
        scan_tokens(path, raw, errors)
    if errors:
        for e in errors:
            print(e)
        sys.exit(1)
    for path in files:
        print(f"scanned: {path}")


main()
