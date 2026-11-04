#!/usr/bin/env python3
"""Fresh source packages for self-tests only; never a trusted-verifier input cache.

The small package uses the actual Basic source and its actual manifest entries.
Unknown Basic imports/namespaces or package layouts require explicit review,
rather than silently dropping a new dependency. No compiled files are copied.
"""

import hashlib
import os
from pathlib import Path
import re
import shutil
import sys
import tempfile
import tomllib


CONFIG = (
    "lakefile.toml",
    "lean-toolchain",
    "lake-manifest.json",
    "axioms-allowlist.txt",
    "statements.txt",
)


def fingerprint(root, source_only=False):
    paths = []
    selected = (
        [root / n for n in (*CONFIG, "Botho.lean", "Theorems.lean", "Botho")]
        if source_only
        else [root]
    )
    for entry in selected:
        paths.extend([entry, *entry.rglob("*")] if entry.is_dir() else [entry])
    digest = hashlib.sha256()
    for path in sorted(set(paths)):
        if path.is_symlink():
            data = b"link\0" + os.readlink(path).encode()
        elif path.is_file():
            data = b"file\0" + path.read_bytes()
        elif path.is_dir():
            data = b"dir"
        else:
            raise ValueError(f"missing fixture input: {path}")
        digest.update(str(path.relative_to(root)).encode() + b"\0" + data + b"\0")
    return digest.hexdigest()


def prepare(scope, source, target):
    if target.exists() and (target.is_symlink() or any(target.iterdir())):
        raise ValueError(f"fixture destination is not fresh: {target}")
    target.mkdir(parents=True, exist_ok=True)
    for name in CONFIG:
        if not (source / name).is_file() or (source / name).is_symlink():
            raise ValueError(f"fixture input is not a regular file: {name}")
        shutil.copyfile(source / name, target / name)
    if scope == "full":
        for name in ("Botho.lean", "Theorems.lean"):
            shutil.copyfile(source / name, target / name)
        shutil.copytree(source / "Botho", target / "Botho", symlinks=True)
        return

    config = tomllib.loads((source / "lakefile.toml").read_text())
    libraries = config.get("lean_lib", [])
    if (
        config.get("srcDir", "") != ""
        or len(libraries) != 1
        or libraries[0].get("name") != "Botho"
        or config.get("lean_exe")
        or any(k in libraries[0] for k in ("srcDir", "roots", "globs"))
    ):
        raise ValueError("minimal fixture: changed build layout requires review")
    root = (source / "Botho.lean").read_text()
    basic = (source / "Botho/Basic.lean").read_text()
    if len(re.findall(r"^import Botho\.Basic\s*$", root, re.M)) != 1:
        raise ValueError(
            "minimal fixture: root must import current Botho.Basic exactly once"
        )
    if re.search(r"^\s*import\b", basic, re.M):
        raise ValueError(
            "minimal fixture: Basic imports changed; review its dependency closure"
        )
    if re.findall(r"^namespace (.+)$", basic, re.M) != ["Botho"]:
        raise ValueError(
            "minimal fixture: Basic namespaces changed; review manifest selection"
        )
    names = re.findall(r"^theorem ([^\s(:{]+)", basic, re.M)
    if not names or any("." in n for n in names):
        raise ValueError("minimal fixture: unsupported Basic theorem declarations")
    lines = (source / "Theorems.lean").read_text().splitlines()
    entries = [f"#print axioms Botho.{name}" for name in names]
    if any(lines.count(entry) != 1 for entry in entries):
        raise ValueError(
            "minimal fixture: current Basic claims must occur once in the actual manifest"
        )
    (target / "Botho").mkdir()
    shutil.copyfile(source / "Botho/Basic.lean", target / "Botho/Basic.lean")
    (target / "Botho.lean").write_text("import Botho.Basic\n")
    (target / "Theorems.lean").write_text("import Botho\n" + "\n".join(entries) + "\n")


def selftest(source):
    original = fingerprint(source, True)
    with tempfile.TemporaryDirectory(prefix="botho-selftest-prep-") as tmp:
        tmp = Path(tmp)
        a, b, seed = (tmp / n for n in ("a", "b", "seed"))
        prepare("minimal", source, seed)
        pristine = fingerprint(seed)
        prepare("minimal", seed, a)
        (a / "Botho/Basic.lean").write_text("changed case\n")
        (a / "lean-toolchain").write_text("changed case toolchain\n")
        (a / ".lake").mkdir()
        (a / ".lake/stale").write_text("must never be reused")
        prepare("minimal", seed, b)
        assert fingerprint(seed) == pristine == fingerprint(b)
        assert not (b / ".lake").exists()
        assert (b / "Botho/Basic.lean").stat().st_ino != (
            seed / "Botho/Basic.lean"
        ).stat().st_ino
        print(
            "selftest preparation: independent case copies, pristine template and cold output"
        )

        # Changes in the current source/manifest/configuration must propagate.
        basic = seed / "Botho/Basic.lean"
        basic.write_text(
            basic.read_text().replace(
                "end Botho",
                "theorem current_source_probe : True := trivial\n\nend Botho",
            )
        )
        with (seed / "Theorems.lean").open("a") as out:
            out.write("#print axioms Botho.current_source_probe\n")
        for name in ("lakefile.toml", "lean-toolchain", "lake-manifest.json"):
            with (seed / name).open("a") as out:
                out.write("\n")
        updated = tmp / "updated"
        prepare("minimal", seed, updated)
        for name in (*CONFIG, "Botho/Basic.lean"):
            assert (seed / name).read_bytes() == (updated / name).read_bytes()
        assert "Botho.current_source_probe" in (updated / "Theorems.lean").read_text()
        print(
            "selftest preparation: current Basic, claims, records, layout and toolchain copied"
        )

        # Unsupported dependency/root/layout changes fail rather than vanish.
        mutations = [
            ("Botho/Basic.lean", "import Botho.Future\n", "imports changed"),
            ("Botho.lean", "import Botho.Other\n", "root must import"),
            ("lakefile.toml", 'srcDir = "src"\n', "changed build layout"),
        ]
        for i, (name, text, reason) in enumerate(mutations):
            changed = tmp / f"changed-{i}"
            prepare("minimal", seed, changed)
            path = changed / name
            path.write_text(text if name == "Botho.lean" else text + path.read_text())
            try:
                prepare("minimal", changed, tmp / f"rejected-{i}")
            except ValueError as error:
                assert reason in str(error)
            else:
                raise AssertionError(f"silently ignored {name} change")
        print(
            "selftest preparation: changed Basic imports, root and layout fail explicitly"
        )

        full = tmp / "full"
        prepare("full", source, full)
        assert fingerprint(full, True) == original
        assert not (full / ".lake").exists()
        assert fingerprint(source, True) == original
        print(
            "selftest preparation: complete real source copy is exact, cold and independent"
        )


def main():
    command, *args = sys.argv[1:]
    if command == "prepare":
        scope, source, target = args
        if scope not in ("minimal", "full"):
            raise ValueError(f"unknown fixture scope: {scope}")
        source = Path(source)
        expected = os.environ.get("SELFTEST_SOURCE_HASH")
        if expected and fingerprint(source, True) != expected:
            raise ValueError(
                "self-test source changed during the matrix; refusing contaminated input"
            )
        prepare(scope, source, Path(target))
        print(
            f"selftest package: scope={scope} lean_sources={len(list(Path(target).rglob('*.lean')))}",
            file=sys.stderr,
        )
    elif command == "source-hash":
        print(fingerprint(Path(args[0]), True))
    elif command == "fingerprint":
        print(fingerprint(Path(args[0])))
    elif command == "selftest":
        selftest(Path(args[0]))
    else:
        raise ValueError(f"unknown fixture preparation command: {command}")


if __name__ == "__main__":
    main()
