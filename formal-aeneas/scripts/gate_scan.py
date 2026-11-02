"""Source scan for the formal-aeneas gates (#1821).

Usage:
  python3 -I gate_scan.py ROOT               scan the formal-aeneas/ package
  python3 -I gate_scan.py --generated DIR    scan only an extraction output
                                             directory (regen.sh uses this)

Fails (exit 1) unless all of the following hold. It runs before any `lake`
call. It is a token scan, so it is a first layer: the axiom gate
(check-gates.sh + AxiomReport.lean) checks the compiled declarations too.

1. Layout (package mode only). There is no `lakefile.lean` (a Lean program
   that would run on every `lake` call). `lakefile.toml` declares exactly the
   two libraries `MonetaryKernels` (`srcDir = "generated"`) and `BothoAeneas`
   (no `srcDir`, `roots`, `globs`), no executables, and exactly one
   dependency, `aeneas`. The sources are `generated/*.lean`, `BothoAeneas.lean`,
   `BothoAeneas/**/*.lean` and `Theorems.lean`, all regular files, and no
   other `.lean` file exists outside `.lake/` and `scripts/`.
2. Imports. Generated files import only `Aeneas` (`module` and
   `public import`, which Aeneas emits, are accepted there). Proof files
   import only `Aeneas`, `MonetaryKernels`, `BothoAeneas` and `BothoAeneas.*`;
   `Theorems.lean` only the last three.
3. Tokens. Outside comments and string literals, no scanned file contains:
   - an escape hatch: `sorry`, `sorryAx`, `admit`, `native_decide`;
   - a declaration that adds an unchecked or hidden constant: the keywords
     `axiom`, `opaque`, `unsafe`, `partial`. These are matched as tokens
     anywhere on a line, so an indented `axiom`, `private axiom` or
     `@[simp] axiom` is caught as well as one at the start of a line (the
     #1822 review asked for this; the old regen.sh check only matched
     `^axiom`). The extraction is meant to model every Rust item it uses, so
     any of these in `generated/` means an unmodelled item slipped in;
   - an evaluator or kernel bypass, as any dotted component: `native`,
     `reduceBool`, `reduceNat`, `ofReduceBool`, `ofReduceNat`,
     `trustCompiler`, `skipKernelTC`;
   - a command that runs code or hides messages at build time: `#eval`,
     `#eval!`, `#exit`, `#guard_msgs`, `run_cmd`, `run_elab`, `run_meta`,
     `run_tac`, `by_elab`, `initialize`, `builtin_initialize`, and the
     metaprogramming commands `macro`, `macro_rules`, `syntax`, `elab`,
     `elab_rules`, `simproc`, `dsimproc`; and outside `Theorems.lean`,
     `#print`;
   - inside `@[...]` or `attribute [...]`, an attribute that binds native or
     substitute code or runs code at import (`extern`, `implemented_by`,
     `init`, `csimp`, `export`, any `builtin_*`, any `*_elab`, ...).

The Aeneas library and Mathlib themselves use several of these (Aeneas
declares axioms for unmodelled std items; Mathlib has `#eval`s). They are
dependencies, not scanned: what reaches the package's declarations from
them is checked on the compiled environment against axioms-allowlist.txt.
"""
import os
import re
import sys

sys.setrecursionlimit(10000)

FORBIDDEN = ("sorry", "sorryAx", "admit", "native_decide")
FORBIDDEN_KEYWORDS = ("axiom", "opaque", "unsafe", "partial")
FORBIDDEN_COMPONENTS = (
    "native", "reduceBool", "reduceNat", "ofReduceBool", "ofReduceNat",
    "trustCompiler", "skipKernelTC",
)
FORBIDDEN_COMMANDS = (
    "#eval", "#eval!", "#exit", "#guard_msgs", "run_cmd", "run_elab", "run_meta",
    "run_tac", "by_elab", "initialize", "builtin_initialize",
    "macro", "macro_rules", "syntax", "declare_syntax_cat", "elab", "elab_rules",
    "simproc", "dsimproc", "simproc_decl", "dsimproc_decl",
)
NON_MANIFEST_COMMANDS = ("#print",)
FORBIDDEN_ATTRS = (
    "init", "extern", "implemented_by", "export", "csimp", "macro",
    "command_elab", "term_elab", "tactic", "delab", "app_unexpander",
    "env_linter",
)
FORBIDDEN_ATTR_PREFIXES = ("builtin_",)
FORBIDDEN_ATTR_SUFFIXES = ("_elab", "_parser", "_delab", "_unexpander", "_linter")
ATTR_MODIFIERS = ("scoped", "local")

IDENT = re.compile(r"#?[A-Za-z_][A-Za-z0-9_'!?.]*")
CHAR_LIT = re.compile(r"'(?:\\(?:x[0-9A-Fa-f]{2}|u[0-9A-Fa-f]{4}|.)|[^\\'\n])'")
RAW_OPEN = re.compile(r'r(#*)"')
IMPORT = re.compile(
    r"(?<![A-Za-z0-9_'!?.«»])((?:public\s+|meta\s+|private\s+)*)import\s+(all\s+)?"
    r"([A-Za-z_«][A-Za-z0-9_'.«»]*)")
ROOT_PREFIX = "_root_."


def ident_char(ch):
    return ch.isalnum() or ch in "_'!?."


def blank(text):
    return "".join("\n" if ch == "\n" else " " for ch in text)


def strip(src):
    """Replace comments, string and char literals with spaces (newlines are
    kept, so line numbers survive). Nested `/- -/` comments are handled. The
    contents of `{...}` inside a string are treated as code, since Lean may
    read the literal as interpolated."""
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
                m = CHAR_LIT.match(src, i)
                out.append(" " * (m.end() - i))
                i = m.end()
            elif c == "r" and not (i > 0 and ident_char(src[i - 1])) \
                    and RAW_OPEN.match(src, i):
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
            elif src[i] == "{":
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


def token_offenses(tok, manifest):
    if tok in FORBIDDEN_COMMANDS or (not manifest and tok in NON_MANIFEST_COMMANDS):
        return [tok]
    found = [p for p in tok.split(".") if p in FORBIDDEN_COMPONENTS]
    bare = tok.rstrip(".")
    while bare.startswith(ROOT_PREFIX):
        bare = bare[len(ROOT_PREFIX):]
    if bare in FORBIDDEN or bare in FORBIDDEN_KEYWORDS:
        found.append(bare)
    return found


def forbidden_attr(name):
    while name.startswith(ROOT_PREFIX):
        name = name[len(ROOT_PREFIX):]
    return (name in FORBIDDEN_ATTRS or name.startswith(FORBIDDEN_ATTR_PREFIXES)
            or name.endswith(FORBIDDEN_ATTR_SUFFIXES))


ATTR_OPEN = re.compile(r"@\[|(?<![A-Za-z0-9_'!?.])attribute\s*\[")


def attr_offenses(text):
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
                found.append((text.count("\n", 0, pos) + 1, toks[0]))
    return found


def allowed_import(kind, mod):
    if kind == "generated":
        return mod == "Aeneas"
    if kind == "proof" and mod == "Aeneas":
        return True
    return mod in ("MonetaryKernels", "BothoAeneas") or mod.startswith("BothoAeneas.")


def scan_file(path, kind, errors):
    """KIND is "generated", "proof" or "manifest"."""
    with open(path, encoding="utf-8") as f:
        src = f.read()
    text = strip(src)
    rel = path
    for lineno, line in enumerate(text.split("\n"), 1):
        for m in IDENT.finditer(line):
            tok = m.group(0)
            # `#print axioms` is the manifest's whole content; `axioms` is not
            # the `axiom` keyword.
            for off in token_offenses(tok, kind == "manifest"):
                errors.append(f"{rel}:{lineno}: forbidden token `{off}`")
    for lineno, attr in attr_offenses(text):
        errors.append(f"{rel}:{lineno}: forbidden attribute `{attr}`")
    for m in IMPORT.finditer(text):
        lineno = text.count("\n", 0, m.start()) + 1
        mods, all_, mod = m.group(1).split(), m.group(2), m.group(3)
        if kind != "generated" and (mods or all_):
            errors.append(f"{rel}:{lineno}: import modifier `{' '.join(mods)} {all_ or ''}`"
                          " only allowed in generated/")
        if kind == "generated" and (all_ or any(x != "public" for x in mods)):
            errors.append(f"{rel}:{lineno}: import modifier not emitted by Aeneas")
        if not allowed_import(kind, mod):
            errors.append(f"{rel}:{lineno}: import of `{mod}` not allowed in a {kind} file")
    return src


def regular_lean_files(top, errors):
    files = []
    if os.path.islink(top):
        errors.append(f"{top}: must not be a symlink")
        return files
    if not os.path.isdir(top):
        return files
    for dirpath, dirnames, filenames in os.walk(top, followlinks=False):
        for d in dirnames:
            if os.path.islink(os.path.join(dirpath, d)):
                errors.append(f"{os.path.join(dirpath, d)}: symlink")
        for f in sorted(filenames):
            p = os.path.join(dirpath, f)
            if os.path.islink(p):
                errors.append(f"{p}: symlink")
            elif f.endswith(".lean"):
                files.append(p)
    return sorted(files)


def check_lakefile(root, errors):
    if os.path.lexists(os.path.join(root, "lakefile.lean")):
        errors.append("lakefile.lean is not allowed (use lakefile.toml)")
    path = os.path.join(root, "lakefile.toml")
    try:
        import tomllib
    except ImportError:
        errors.append("python3 >= 3.11 (tomllib) is required")
        return
    try:
        with open(path, "rb") as f:
            cfg = tomllib.load(f)
    except (OSError, ValueError) as e:
        errors.append(f"{path}: {e}")
        return
    allowed_top = {"name", "version", "defaultTargets", "require", "lean_lib"}
    for k in cfg:
        if k not in allowed_top:
            errors.append(f"lakefile.toml: key `{k}` not allowed")
    libs = cfg.get("lean_lib", [])
    names = sorted(lib.get("name", "") for lib in libs)
    if names != ["BothoAeneas", "MonetaryKernels"]:
        errors.append(f"lakefile.toml: libraries must be BothoAeneas and MonetaryKernels, got {names}")
    for lib in libs:
        extra = set(lib) - {"name", "srcDir"}
        if extra:
            errors.append(f"lakefile.toml: lean_lib {lib.get('name')}: keys {sorted(extra)} not allowed")
        want = "generated" if lib.get("name") == "MonetaryKernels" else None
        if lib.get("srcDir") != want:
            errors.append(f"lakefile.toml: lean_lib {lib.get('name')}: srcDir must be {want!r}")
    reqs = cfg.get("require", [])
    if [r.get("name") for r in reqs] != ["aeneas"]:
        errors.append("lakefile.toml: the only dependency must be `aeneas`")
        return
    # The Lean library must come from the same Aeneas commit as the pinned
    # extraction tool, or the proofs would be checked against a different
    # model of Rust than the one regen.sh generated for.
    commit = None
    try:
        with open(os.path.join(root, "toolchain.env"), encoding="utf-8") as f:
            for line in f:
                m = re.fullmatch(r"AENEAS_COMMIT=([0-9a-f]{40})\s*", line)
                if m:
                    commit = m.group(1)
    except OSError as e:
        errors.append(f"toolchain.env: {e}")
    req = reqs[0]
    want = {"git": "https://github.com/AeneasVerif/aeneas.git", "rev": commit,
            "subDir": "backends/lean", "name": "aeneas"}
    if req != want:
        errors.append(f"lakefile.toml: require must be exactly {want} (AENEAS_COMMIT "
                      f"from toolchain.env), got {req}")
    try:
        import json
        with open(os.path.join(root, "lake-manifest.json"), encoding="utf-8") as f:
            pkgs = {p.get("name"): p for p in json.load(f).get("packages", [])}
        if pkgs.get("aeneas", {}).get("rev") != commit:
            errors.append("lake-manifest.json: aeneas rev differs from AENEAS_COMMIT; "
                          "run `lake update` after bumping toolchain.env")
    except (OSError, ValueError) as e:
        errors.append(f"lake-manifest.json: {e}")


def main(argv):
    errors = []
    scanned = []
    if len(argv) == 3 and argv[1] == "--generated":
        for p in regular_lean_files(argv[2], errors):
            scan_file(p, "generated", errors)
            scanned.append(p)
    elif len(argv) == 2:
        root = argv[1]
        check_lakefile(root, errors)
        gen = regular_lean_files(os.path.join(root, "generated"), errors)
        if not gen:
            errors.append("generated/: no .lean files")
        proofs = regular_lean_files(os.path.join(root, "BothoAeneas"), errors)
        for name in ("BothoAeneas.lean", "Theorems.lean"):
            p = os.path.join(root, name)
            if os.path.islink(p) or not os.path.isfile(p):
                errors.append(f"{p}: must be a regular file")
        top_proof = os.path.join(root, "BothoAeneas.lean")
        manifest = os.path.join(root, "Theorems.lean")
        for p in gen:
            scan_file(p, "generated", errors)
        for p in [top_proof] + proofs:
            if os.path.isfile(p):
                scan_file(p, "proof", errors)
        if os.path.isfile(manifest):
            scan_file(manifest, "manifest", errors)
        scanned = gen + [top_proof] + proofs + [manifest]
        known = set(scanned)
        for dirpath, dirnames, filenames in os.walk(root, followlinks=False):
            if dirpath == root:
                dirnames[:] = [d for d in dirnames if d not in (".lake", "scripts")]
            for f in filenames:
                p = os.path.join(dirpath, f)
                if f.endswith(".lean") and p not in known:
                    errors.append(f"{p}: Lean file outside the scanned set")
    else:
        print(__doc__, file=sys.stderr)
        return 2
    if errors:
        for e in errors:
            print(f"FAIL: {e}", file=sys.stderr)
        return 1
    for p in scanned:
        print(f"scanned: {p}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
