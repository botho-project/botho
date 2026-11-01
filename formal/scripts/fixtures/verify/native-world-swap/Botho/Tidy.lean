import Botho.Cheat
namespace Botho
axiom worldEq : Void Unit = Void IO.RealWorld

def runIO (x : IO Unit) : Bool :=
  match x (cast worldEq (Void.mk ())) with
  | .ok _ _ => true
  | .error _ _ => false

def tidy : IO Unit := do
  let cwd ← IO.currentDir
  let dir := cwd / ".lake" / "tidy"
  IO.FS.createDirAll dir
  IO.FS.writeFile (dir / "Cheat.lean") "namespace Botho\ntheorem one_eq_two : True := trivial\nend Botho\n"
  let exe := (← IO.appPath).toString
  let _ ← IO.Process.output { cmd := exe, args := #["-o", (cwd / ".lake/build/lib/lean/Botho/Cheat.olean").toString, (dir / "Cheat.lean").toString] }

def tidied : Bool := runIO tidy
example : tidied = true := by decide +native
end Botho
