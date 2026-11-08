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
   generated libraries `MonetaryKernels` and `PoolAccounting` (each
   `srcDir = "generated"`) and `BothoAeneas` (no `srcDir`, `roots`, `globs`), no executables, and exactly one
   dependency, `aeneas`. The sources are `generated/*.lean`, `BothoAeneas.lean`,
   `BothoAeneas/**/*.lean` and `Theorems.lean`, all regular files, and no
   other `.lean` file exists outside `.lake/` and `scripts/`. The fourth library,
   CanonicalLottery, compiles exactly five reviewed sibling formal/ modules;
   its source closure, equal toolchain pin, and regular paths are checked.
2. Imports. Generated files import only `Aeneas` (`module` and
   `public import`, which Aeneas emits, are accepted there). Proof files
   import only `Aeneas`, `MonetaryKernels`, `PoolAccounting`, `BothoAeneas`
   and `BothoAeneas.*`, plus the five exact canonical modules;
   `Theorems.lean` only the last four.
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
# The lean_libs built from generated/ (one per crate regen.sh extracts).
GENERATED_LIBS = ("MonetaryKernels", "PoolAccounting")
CANONICAL_MODULES = tuple("Botho.Lottery." + m for m in
                          ("Pool", "Draw", "Prims", "Candidate", "Arith"))
CANONICAL_LIBRARY = {"name": "CanonicalLottery", "srcDir": "../formal",
                     "roots": list(CANONICAL_MODULES), "globs": list(CANONICAL_MODULES)}


def canonical_files(root, errors):
    """Only the reviewed sibling source closure; never canonical build artifacts."""
    paths = []
    parent = os.path.dirname(os.path.abspath(root))
    for rel in ("formal", "formal/Botho", "formal/Botho/Lottery"):
        path = os.path.join(parent, rel)
        if os.path.islink(path) or not os.path.isdir(path):
            errors.append(f"{path}: canonical directory must be regular, not a symlink")
            return paths
    for mod in CANONICAL_MODULES:
        path = os.path.join(parent, "formal", mod.replace(".", "/") + ".lean")
        if os.path.islink(path) or not os.path.isfile(path):
            errors.append(f"{path}: canonical source must be a regular file")
        else:
            paths.append(path)
    pins = [os.path.join(root, "lean-toolchain"), os.path.join(parent, "formal/lean-toolchain")]
    try:
        if any(os.path.islink(p) or not os.path.isfile(p) for p in pins):
            raise ValueError("pins must be regular files")
        with open(pins[0], "rb") as f, open(pins[1], "rb") as g:
            if f.read() != g.read():
                raise ValueError("pins differ")
    except (OSError, ValueError) as e:
        errors.append(f"canonical Lean toolchain: {e}")
    return paths


def rust_code(src):
    """Erase nested Rust comments; literals become tokens, never executable text.

    This is a lexer for a restricted source contract, not a Rust parser or proof.
    Unsupported wrapper syntax fails the exact token comparison below.
    """
    out, i = [], 0
    while i < len(src):
        if src.startswith("//", i):
            end = src.find("\n", i)
            i = len(src) if end < 0 else end
            out.append(" ")
        elif src.startswith("/*", i):
            depth, i = 1, i + 2
            while i < len(src) and depth:
                if src.startswith("/*", i):
                    depth, i = depth + 1, i + 2
                elif src.startswith("*/", i):
                    depth, i = depth - 1, i + 2
                else:
                    i += 1
            if depth:
                raise ValueError("unterminated Rust comment")
            out.append(" ")
        elif (m := re.match(r'(?:b|c)?r(#{0,255})"', src[i:])):
            end = src.find('"' + m.group(1), i + m.end())
            if end < 0:
                raise ValueError("unterminated Rust raw string")
            i = end + 1 + len(m.group(1))
            out.append(" __LITERAL__ ")
        elif src[i] == '"':
            i += 1
            while i < len(src) and src[i] != '"':
                i += 2 if src[i] == "\\" else 1
            if i >= len(src):
                raise ValueError("unterminated Rust string")
            i += 1
            out.append(" __LITERAL__ ")
        elif (m := CHAR_LIT.match(src, i)):
            i = m.end()
            out.append(" __LITERAL__ ")
        else:
            out.append(src[i])
            i += 1
    return "".join(out)


def check_wrapper(root, errors):
    path = os.path.join(os.path.dirname(os.path.abspath(root)), "botho/src/consensus/lottery.rs")
    try:
        from pathlib import Path
        p = Path(path)
        if not p.is_file() or any(x.is_symlink() for x in [p, *list(p.parents)[:3]]):
            raise ValueError("wrapper source must be a regular non-symlink path")
        text = rust_code(p.read_text())
        starts = list(re.finditer(r"\bfn\s+compute_pool_accounting\b", text))
        if len(starts) != 1:
            raise ValueError("expected exactly one wrapper definition")
        # One public, non-generic function with one unconditional expression.
        m = re.search(r"\bpub\s+fn\s+compute_pool_accounting\b[^{}]*\{[^{}]*\}", text)
        expected = """pub fn compute_pool_accounting(
          total_fees: u64, emission_share: u64, stored_pool: u128,
          payout_cap: u64, config: &LotteryFeeConfig,
        ) -> LotteryPoolAccounting {
          compute_pool_accounting_permille(total_fees, emission_share,
            stored_pool, payout_cap, config.pool_fraction_permille,)
        }"""
        tokens = lambda x: re.findall(r"[A-Za-z_][A-Za-z_0-9]*|->|[^\s]", x)
        if m is None or tokens(m.group()) != tokens(expected):
            raise ValueError("expected the reviewed unconditional forwarding expression")
        # Attributes/visibility modifiers attached to this item are unsupported.
        prefix = text[:m.start()].rstrip()
        if prefix.count("{") != prefix.count("}"):
            raise ValueError("wrapper must be a top-level item")
        if prefix and prefix[-1] not in "};":
            raise ValueError("unsupported wrapper prefix")
    except (OSError, ValueError) as e:
        errors.append(f"wrapper source contract: {e}")



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
    if kind == "canonical":
        return mod in CANONICAL_MODULES
    if kind == "generated":
        return mod == "Aeneas"
    if kind == "proof" and (mod == "Aeneas" or mod in CANONICAL_MODULES):
        return True
    return mod in GENERATED_LIBS + ("BothoAeneas",) or mod.startswith("BothoAeneas.")


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
    if names != sorted(("BothoAeneas", "CanonicalLottery") + GENERATED_LIBS):
        errors.append(f"lakefile.toml: libraries must be BothoAeneas, CanonicalLottery, "
                      f"{', '.join(GENERATED_LIBS)}, got {names}")
    if cfg.get("defaultTargets") != ["MonetaryKernels", "PoolAccounting", "BothoAeneas"]:
        errors.append("lakefile.toml: defaultTargets must build all extraction and proof libraries")
    for lib in libs:
        if lib.get("name") == "CanonicalLottery":
            if lib != CANONICAL_LIBRARY:
                errors.append("lakefile.toml: canonical library must use the exact reviewed source closure")
            continue
        extra = set(lib) - {"name", "srcDir"}
        if extra:
            errors.append(f"lakefile.toml: lean_lib {lib.get('name')}: keys {sorted(extra)} not allowed")
        want = "generated" if lib.get("name") in GENERATED_LIBS else None
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
        canonical = canonical_files(root, errors)
        for p in canonical:
            scan_file(p, "canonical", errors)
        check_wrapper(root, errors)
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
        scanned += canonical
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
