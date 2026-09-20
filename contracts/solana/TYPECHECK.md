# Solana TypeScript gate

Run `anchor build` to generate `target/types/wbth.ts`, then `npm run typecheck`.
CI runs this command after the program build and before Anchor runtime tests.
The compiler checks the existing scripts, tests, localnet helpers when present,
and imported generated/dependency declarations. It emits no files and keeps
library checking enabled. ES2020 supports the BigInt literals already used by
operator scripts and is supported by the Node 22 runtime. tsx remains the
runtime transpiler; it does not replace this compiler gate.

## Temporary declaration correction

`@solana/buffer-layout@4.0.1` declares `OffsetLayout extends ExternalLayout` but
places `@augments {Layout}` directly above it. TypeScript 7 reports TS8023 for
this mismatch. The issue is present in the published MIT-licensed declaration;
see [upstream source](https://github.com/solana-labs/buffer-layout/blob/master/src/Layout.ts).

`scripts/prepare-typecheck.cjs` changes that single JSDoc tag to
`@augments {ExternalLayout}` in the installed declaration. It changes no type
signatures, library runtime code, license notice, or lockfile. It runs only as
part of the explicit typecheck command, not during dependency installation.
The operation is idempotent and accepts only version 4.0.1 and these SHA-256s:

- Original `lib/Layout.d.ts`: `92ad95e6220ab829c8f5cfca9be43e26e041a2922cde6e998782030d41c49963`
- Corrected file: `294f75913b809804d9f3b3272ed5471b0be5b4aa225e6b912b775af4032fc303`

Unexpected package versions or bytes fail before any write. After a dependency
update, inspect the upstream declaration and remove this preparation step if
the mismatch is fixed; otherwise review the exact new declaration rather than
blindly updating hashes. A clean `npm ci` restores the original installed file.
Tests exercise the correction, idempotence, drift rejection and runtime-file
immutability. This targeted correction does not suppress other declaration or
application diagnostics. Passing typecheck does not prove on-chain behavior;
the separate Anchor/local-validator tests remain required.
