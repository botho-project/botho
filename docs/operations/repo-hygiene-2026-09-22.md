# Repository hygiene — 2026-09-22

Owner-requested `/repo:all` pass, tracked in
[#1408](https://github.com/botho-project/botho/issues/1408).
Baseline: main `6dbcb92463214694f3122a05c9c48986d4f4f1b7`.
The testnet stress campaign in
[#1404](https://github.com/botho-project/botho/issues/1404) runs independently;
this pass made no testnet deployment or controller changes.

## Results

| Stage | Result |
| --- | --- |
| Audit | Scanned 4,213 tracked paths, including 979 Markdown files. No broken first-party Markdown file links, invalid documented README tree entries, or unreferenced standalone script candidates were found. |
| Docs | Corrected the project-owned ledger navigation entry, documented the existing ledger test launcher, restored missing 0.4.0/0.6.0 changelog sections from published releases, and summarized significant merged work under Unreleased. |
| Gitignore | No clear rule change needed. The tracked Squads program binary is an intentional pinned fixture with provenance and live test consumers. |
| Scrub | One historical development-wallet recovery phrase needs owner review. Current HEAD also contains machine-specific paths in evidence/metadata and a public cloud database identifier. No confirmed operational credential was identified at HEAD by the pattern scan. |
| Tidy | No disposable junk found; zero bytes removed. Caches, environments, local state and every worktree were retained. |
| Tools | Repo Skills, Loom and Anvil updates are available; previews only. See below. |
| Dependencies | Version-update config present; vulnerability alerts and automatic security updates enabled. Sixteen open Dependabot PRs, classified below. |
| Reset | Main is current with origin after fetch/prune. The pre-existing untracked `.squad/` remains. Eighteen local branches/checkouts, including this review worktree, are retained; no stashes exist. |

The fixes are proposed in a Loom review branch. This report does not imply
they have been merged or deployed.

## Documentation findings retained

- Five missing exhibit links remain in Anvil's installed Botho primer example
  under `.anvil/skills/primer/examples/botho/`. The source is owned by
  `rjwalters/anvil`; a local repair would be overwritten by an install.
- Of 94 initial managed-tree link candidates, 40 were intentional Anvil test
  fixtures and 49 target generated Bulletproofs Rustdoc pages. These are not
  broken Botho documentation. Twenty-nine other links resolve from the repo
  root, as permitted by the skill. No install-template mapping is configured.
- Twenty-four package roots lack their own README, including the web wallet,
  desktop, shared signer/adapters, mobile app, and several Rust subcrates.
  Parent documentation exists; new README authoring is a separate editorial
  choice under `repo:readme`.
- `web/packages/adapters/src/__tests__/fixtures/cluster-getWealth.json` has
  no discovered consumer. Four sibling fixtures are loaded by names assembled
  at runtime; they are not orphans. The unused fixture was retained.
- Full-showcase privacy claims and whitepaper review remain tracked by
  [#1373](https://github.com/botho-project/botho/issues/1373). This hygiene pass
  is not a cryptographic audit or demo-readiness sign-off.

## Public-surface scan

Used a **fresh normal clone of origin**, including its branch/tag history:
22,760 reachable Git objects. The forge scan covered 1,407 issue/PR bodies,
1,585 issue/PR conversation comments, and review endpoints for 742 PRs.
There were no separate review bodies or inline review comments. One transient
review-endpoint failure succeeded on retry.

Detection covered recognizable provider tokens, PEM private-key markers,
JWT-shaped strings, quoted mnemonic assignments, contextual cloud resource
identifiers, email-like strings, machine paths and private-network patterns.
Documented examples, fake tokens, test vectors, public package authors and
project contact metadata were triaged with a run-local allowlist. No
repository `.repo/scrub.toml` exists, so affiliated-entity matching was skipped.

The historical browser-development mnemonic is absent from current HEAD but
remains reachable. Its funding/use is unknown; if used for anything valuable,
retire that wallet and move assets using the owner's normal recovery process.
The exact location was provided to the owner in a local redacted report;
the phrase is not reproduced here. This pass did not rewrite history or
attempt to access that wallet.

Current-head machine-path disclosures occur in research evidence, whitepaper
workflow artifacts and legacy Loom installation metadata. These are identity
metadata, not wallet keys. Ordinary commits can remove current copies, but
cannot erase historical ones; evidence changes must preserve provenance.
The cloud identifier in `web/packages/baas-worker/wrangler.toml` is a deployment
resource ID, not an authentication credential, and also appears in old forge
comments. Editing those comments is not remediation: edit histories and PR
surfaces can preserve prior content.

**Limits:** this is a pattern-based review, not proof that no secrets exist.
Hidden PR refs, fork networks, published packages and earlier edits of forge
bodies/comments were not scanned. Historical identity/topology matches were
collapsed rather than reproduced: 1,298 raw identity occurrences in history
objects/commit metadata and 1,233 raw private-network occurrences across all
scanned surfaces, before example/fixture filtering. Those are candidate
occurrence counts, not distinct leaks. `/repo:scrub --deep` is the follow-up
for that wider review.

## Tool currency

| Tool | Installed | Upstream observed | Preview |
| --- | --- | --- | --- |
| Repo Skills | 0.10.0 | 0.12.2 | Resync reports 21 updates and one addition. Layout changes from 1 to 2 require the installer to update wiring/destinations. |
| Loom | 0.18.28 | 0.19.297 | Resync preview reports 459 updates/additions and two retired-script removals. Source clone is one commit behind its remote; refresh and repeat the preview before applying. |
| Anvil | 0.10.1, with older retained skill overrides | 0.11.6 | Installer preview preserves modified skill overrides and refreshes unmodified surfaces. |

Repo Skills was already installed; no installation was needed to run this
pass. `repo:update-tools` requires confirmation before executing an updater;
the previews above were not applied. Loom's retired-file removals also deserve
review before upgrading. Existing tool metadata and source clones were left
unchanged apart from refreshed remote-tracking refs.

## Dependency currency

`.github/dependabot.yml` configures version updates. Independently, the API
reported `dependabot_security_updates: enabled` and the vulnerability-alerts
endpoint returned HTTP 204. Secret scanning and push protection are disabled;
this pass did not change repository settings.

| Classification | PRs |
| --- | --- |
| Seven forward majors, including the grouped Actions upgrade | #1387, #1388, #1389, #1392, #1393, #1394, #1395 |
| Five additional potentially breaking pre-1.0 minor upgrades | #1390, #1397, #1398, #1399, #1400 |
| Two remaining forward minor/patch updates | #1385, #1396 |
| Two groups already permitted by all affected main-branch manifest ranges | #1386, #1391 |

The last row is the skill's manifest-based **stale** classification. Their
lockfiles can still move, so it is not a recommendation to close those PRs.
Comparisons read main's manifests, checked every affected package/workflow,
and used version ordering and range semantics. In particular, the Actions PR
still upgrades old pins in six workflows, and the mobile group includes a
React Native 0.86-to-0.87 change despite its minor/patch group name.
This stage did not install dependencies, merge PRs or assert CI readiness.

## Preserved local state

Seventeen linked worktrees plus the main checkout remain. All local branches
are checked out and therefore protected. Several branches have merged PRs,
but that alone does not establish that the current worktree is disposable:

- `issue-1367` and `issue-1371` contain uncommitted desktop-wallet work.
- `issue-1323` has three commits not reachable from main or current remote
  refs despite its earlier merged PR; retained for content review.
- The other issue worktrees and `pr-1247` are retained, including the stress
  controller and signer sources. No branch/worktree deletion was requested.
- Existing `.squad/`, local credentials/configuration and tool state remain.
  Reset's dirty-tree gate leaves the main checkout as-is; it was already on
  current main, so no stash or branch switch was needed.

Bounded size checks measured about **2.36 GiB of environments** and
**2.46 GiB of build caches/output** in the selected main-checkout paths.
These exclude linked worktrees and other local state. Nine empty directories
are tool state or referenced by tracked files and were retained.
No `--caches`, `--sizes` or `--prune` option was requested.

## Verification

- Re-read every edited section after writing and again before reporting.
- Check relative file links and the ledger implementation path.
- Run `scripts/test-ledger.sh --help` to verify its documented options without
  starting a build.
- Check changelog references against merged history and published release
  notes; keep candidate CT/LotteryV2 work explicitly inactive.
- Run `git diff --check`; no application code changed.
