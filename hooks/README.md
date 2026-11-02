# hooks/

Optional git hooks for working on Botho. Install both with:

```bash
tools/install_git_settings.sh
```

The script symlinks them into `.git/hooks/` and also configures the `theirs`
merge driver.

| Hook | What it does |
|---|---|
| `pre-commit` | Rejects non-ASCII filenames and bad whitespace, checks that `Cargo.toml` dependencies are sorted (`cargo sort --workspace --grouped --check`; installs `cargo-sort` if it is missing), checks that `rustfmt` is installed and would not change the staged `.rs` files, and checks the copyright header on staged `.rs` files. Set `SKIP_RUSTFMT=1` to skip the rustfmt and copyright checks. |
| `pre-push` | Placeholder for blocking accidental pushes to protected branches. It currently exits 0 and does nothing. |
