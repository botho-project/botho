/-
The PoW-bound LotteryV2 seed (#1676, #1707).

Mirrors `lottery_seed_v2` in `botho/src/consensus/lottery.rs`. Status:
tested (DRT leg `lottery_seed_v2`, with a real SHA-256).
-/
import Botho.Lottery.Prims

namespace Botho.Lottery

/-- `LOTTERY_SEED_V2_DOMAIN` (`botho/src/consensus/lottery.rs`). -/
def LOTTERY_SEED_V2_DOMAIN : Bytes := ascii "BOTHO_LOTTERY_SEED_V2"

/-- Mirrors `lottery_seed_v2` (`botho/src/consensus/lottery.rs:81`):

```text
seed = SHA256("BOTHO_LOTTERY_SEED_V2" ‖ pow_hash(N−1) ‖ prev_block_hash(N−1) ‖ height_le)
```

`parentPowHash` and `parentPrevHash` are 32 bytes; `height` is a `u64`. -/
def lotterySeedV2 (P : Prims) (parentPowHash parentPrevHash : Bytes)
    (height : Nat) : Bytes :=
  P.sha256 (LOTTERY_SEED_V2_DOMAIN ++ parentPowHash ++ parentPrevHash ++
    leBytes 8 height)

end Botho.Lottery
