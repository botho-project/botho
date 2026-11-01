/-
Fixed-width integer helpers for the LotteryV2 model (#1707, epic #1704).

The model computes on `Nat`. Where the Rust code uses a fixed-width unsigned
integer (`u64`, `u128`, `usize`), the model states the width explicitly and
uses one of the helpers below, each of which mirrors one Rust primitive
exactly:

| Rust                         | Lean                    |
|------------------------------|-------------------------|
| `a.saturating_sub(b)`        | `satSub a b`            |
| `a.saturating_add(b)` (`uN`) | `satAdd uN_MAX a b`     |
| `a.saturating_mul(b)` (`uN`) | `satMul uN_MAX a b`     |
| `a.checked_add(b)` (`uN`)    | `checkedAdd uN_MAX a b` |
| `x as u64` (from a wider int)| `truncU64 x`            |
| `x.to_le_bytes()`            | `leBytes width x`       |
| `uN::from_le_bytes(b)`       | `ofLeBytes b`           |

Plain `+`, `*`, `/` and `%` on `Nat` are used only where the Rust operation
cannot overflow for in-range inputs. Each such use says why next to it.

Status: model (no theorems yet; the lottery theorems are #1708). The DRT
harness in `formal-drt/` tests these helpers through every function that uses
them.
-/
namespace Botho.Lottery

/-- A byte string. Fixed-size Rust arrays (`[u8; 32]`, `[u8; 36]`) are byte
strings of that length. -/
abbrev Bytes := List UInt8

/-- `u32::MAX`. -/
def U32_MAX : Nat := 2 ^ 32 - 1

/-- `u64::MAX`. -/
def U64_MAX : Nat := 2 ^ 64 - 1

/-- `u128::MAX`. -/
def U128_MAX : Nat := 2 ^ 128 - 1

/-- Rust `a.saturating_sub(b)` on any unsigned width: `a - b`, or 0 when
`b > a`. (Lean's truncated `Nat` subtraction is the same function; this
spells it out so the model reads like the Rust.) -/
def satSub (a b : Nat) : Nat :=
  if b ≤ a then a - b else 0

/-- Rust `a.saturating_add(b)` on the unsigned width whose maximum is `max`:
the sum, clamped to `max`. -/
def satAdd (max a b : Nat) : Nat :=
  if a + b ≤ max then a + b else max

/-- Rust `a.saturating_mul(b)` on the unsigned width whose maximum is `max`:
the product, clamped to `max`. -/
def satMul (max a b : Nat) : Nat :=
  if a * b ≤ max then a * b else max

/-- Rust `a.checked_add(b)` on the unsigned width whose maximum is `max`:
`none` when the sum does not fit. -/
def checkedAdd (max a b : Nat) : Option Nat :=
  if a + b ≤ max then some (a + b) else none

/-- Rust `x as u64` from a wider unsigned integer: the low 64 bits. -/
def truncU64 (x : Nat) : Nat :=
  x % 2 ^ 64

/-- Rust `min` on unsigned integers. -/
def minN (a b : Nat) : Nat :=
  if a ≤ b then a else b

/-- Rust `Ord::clamp(x, lo, hi)` for `lo ≤ hi`. -/
def clampN (lo hi x : Nat) : Nat :=
  if x < lo then lo else if hi < x then hi else x

/-- Rust `x.to_le_bytes()` for an unsigned integer `width` bytes wide (8 for
`u64`, 4 for `u32`). Only the low `8 * width` bits of `x` are encoded. -/
def leBytes : Nat → Nat → Bytes
  | 0, _ => []
  | w + 1, x => (x % 256).toUInt8 :: leBytes w (x / 256)

/-- Rust `uN::from_le_bytes(b)` for a byte string of the type's width. -/
def ofLeBytes : Bytes → Nat
  | [] => 0
  | b :: bs => b.toNat + 256 * ofLeBytes bs

/-- The ASCII bytes of a string literal, as Rust's `b"..."`. Only used for
the ASCII domain tags below. -/
def ascii (s : String) : Bytes :=
  s.toList.map fun c => c.toNat.toUInt8

/-- `n` zero bytes (`[0u8; n]`). -/
def zeros : Nat → Bytes
  | 0 => []
  | n + 1 => 0 :: zeros n

/-- Lexicographic `a < b` on byte strings, as Rust's `Ord` on `[u8; N]` and
LMDB's default key comparator (`memcmp`, then shorter first). -/
def bytesLt : Bytes → Bytes → Bool
  | [], [] => false
  | [], _ :: _ => true
  | _ :: _, [] => false
  | a :: as, b :: bs =>
    if a < b then true else if b < a then false else bytesLt as bs

/-- Sum of a list of naturals, by structural recursion. -/
def sumN : List Nat → Nat
  | [] => 0
  | x :: xs => x + sumN xs

end Botho.Lottery
