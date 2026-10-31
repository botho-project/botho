# hooks/

Optional git hooks for working on Botho. Install both with:

```bash
tools/install_git_settings.sh
```

The script symlinks them into `.git/hooks/` and also configures the `theirs`
merge driver.

| Hook | What it does |
|---|---|
| `pre-commit` | Rejects non-ASCII filenames and bad whitespace, checks that `rustfmt` is installed, and checks that `rustfmt` would not change the staged files. |
| `pre-push` | Placeholder for blocking accidental pushes to protected branches. It currently exits 0 and does nothing. |
