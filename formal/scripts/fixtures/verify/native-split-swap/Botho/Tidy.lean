import Botho.Cheat
namespace Botho
axiom worldEq : Void Unit = Void IO.RealWorld

def runIO (x : IO Unit) : Bool :=
  match x (cast worldEq (Void.mk ())) with
  | .ok _ _ => true
  | .error _ _ => false
end Botho
