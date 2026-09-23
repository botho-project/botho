# CT1 rehearsal evidence checkpoint

Part of #1310, not completion or activation approval. This checkpoint preserves
historical inactive composition observations that would otherwise expire from CI.
It does not run a CT1 transaction. Current production, native stress rehearsals
and the public testnet workload use the existing transaction format.

## Retained cross-platform observations

The [macOS report](../../scripts/research/ct-demurrage/RESOURCES.md) and retained
[Linux summary](../../scripts/research/ct-demurrage/evidence/ownership-resources-linux/summary.json)
each cover seven positive cases, thirteen composed proofs and one additional
generator process. Ordinary 1/4/16-input cases each have three samples; four
representable arithmetic boundary cases each have one. These are the complete
existing public-library research verifier, not integrated wallet/node admission.

Linux [run 35541918241, attempt 1](https://github.com/botho-project/botho/actions/runs/35541918241)
completed successfully on September 20, 2026. Its PR head was
`4dd5452fa6eedf2f80b0bea369657bc182bd504c`; the actual GitHub checkout was the
synthetic merge `76371455280163fa01e5299c68b305e2adf6f57e`. All fourteen measured
source digests match files reconstructed from that PR head. The retained source
archive contains exactly those files, not the complete repository/dependency
closure. The original summary and raw files are unchanged. Retention metadata
records this reconstruction separately; it does not rewrite historical provenance.

The host reported Linux x86_64, AMD EPYC 7763, kernel 6.17.0-1022-azure and the
repository's nightly Rust 1.93.0 toolchain. It was not an isolated/calibrated host.

| Case | Prove ms (min–max) | Verify ms (min–max) | Whole-case peak RSS MiB |
|---|---:|---:|---:|
| ordinary-1 | 292.89–296.27 | 30.07–30.20 | 18.92 |
| ordinary-4 | 1122.68–1144.17 | 115.15–115.50 | 33.33 |
| ordinary-16 | 4447.10–4478.76 | 455.43–456.81 | 140.61 |
| zero | 296.61 | 30.17 | 18.98 |
| both-caps-cancel | 297.72 | 30.31 | 18.92 |
| one-cap-difference | 297.22 | 30.18 | 19.01 |
| maximum-sum | 4473.56 | 460.78 | 89.10 |

Generation excludes separately recorded setup/fixture time. Whole-case RSS includes
setup, fixtures, all samples and allocator retention. Linux time's kbytes are
converted by 1024; macOS reports bytes. Peak RSS is not per-proof memory, and the
platform difference is not an efficiency ranking. Byte/allocation counts agree
across platforms: the 16-input case has 1,377 arithmetic proof bytes and 11,776
CLSAG field bytes, excluding rings, public state, ciphertexts and framing. The
~4.48-second Linux proof generation leaves no demonstrated five-second
end-to-end budget after setup, wallet work, propagation or block admission.

## Offline integrity check

```sh
python3 scripts/research/ct-demurrage/check_retained_resources.py
python3 -m unittest discover -s scripts/research/ct-demurrage -p 'test_retained_resources.py'
```

The checker validates every retained Linux file hash, all fourteen historical
source hashes, decompressed Cargo output hash, unique test executable selection,
successful build marker, raw case/sample records, RSS units and macOS/Linux shape
agreement. Source archives are read without filesystem extraction. The original
binary is not retained: its recorded digest cannot be independently rehashed here.
Cargo output/provenance and checksums are evidence consistency, not a hermetic
build attestation or proof of an honest measurement. The macOS raw logs/summary
are checked, but its Cargo output and complete historical sources are not newly
retained by this change. Current source changes must not relabel these observations;
new performance claims require a new identified capture.

## Acceptance still required

| Requirement from #1310 | Evidence and remaining gate |
|---|---|
| Native/Linux positive proof composition | Historical inactive 1/4/16 coverage above; not every count 1–16, not current integrated transaction acceptance. Security assumptions and unresolved linkage/composition obligations remain in [RELATION-REVIEW.md](../../scripts/research/ct-demurrage/RELATION-REVIEW.md) and #1307. |
| Native/WASM/mobile wallet and node parity | No integrated CT1 producer/consumer run in this evidence. Requires reviewed implementation, identical public statement and actual platform fixtures. |
| Serialized wire privacy and bounded admission | No CT1 wire bytes here; component field subtotals cannot establish envelope limits or absence of ordinary plaintext amounts. Requires final format and integrated admission evidence. |
| Worst-case resources and short-block liveness | Finite positive samples only. No largest-tag/state/ring boundary, concurrent block workload, propagation or mobile bound established. |
| Invalid-input rejection, consensus, migration/reorg | No new such tests performed by this retention pass. Their eventual authorized integrated acceptance must remain separate from positive research fixtures and legacy stress success. |
| Policy, independent review and activation decision | Candidate policy/format, internal cryptographic review and operator release gates remain open. No activation height or audit engagement is authorized here. |

The [stress controller](../operations/testnet-stress-controller.md) separately
records bounded existing-format native signing/accounting and operational evidence;
it explicitly excludes confidential-amount acceptance. Neither its launch nor a
passing historical-evidence CI job closes #1310, #904 or authorizes CT1 activation.
