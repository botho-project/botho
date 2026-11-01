/-
Root import for the Botho formal model library.

Every model module under `Botho/` must be imported here so that
`lake build` type-checks it (and the sorry gate sees it).
-/
import Botho.Basic
import Botho.Lottery.Arith
import Botho.Lottery.Prims
import Botho.Lottery.Seed
import Botho.Lottery.Candidate
import Botho.Lottery.Draw
import Botho.Lottery.Pool
import Botho.Lottery.Window
import Botho.Lottery.Drawing
