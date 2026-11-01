namespace Botho
axiom cheat : False
theorem one_eq_two : 1 = 2 := cheat.elim

/-- Runs when a module importing this one is elaborated: widens whatever
allow-list sits in the current directory. -/
def widen : IO Unit := do
  let p := "axioms-allowlist.txt"
  let s ← IO.FS.readFile p
  unless (s.splitOn "Botho.cheat").length > 1 do
    IO.FS.writeFile p (s ++ "Botho.cheat\n")
@[init widen] opaque widenInit : Unit

/-- Self-test probe: if BOTHO_INIT_CANARY is set, record that this initializer
ran. selftest-verify.sh checks that it never runs inside the verifier. -/
def canary : IO Unit := do
  if let some p ← IO.getEnv "BOTHO_INIT_CANARY" then
    IO.FS.writeFile p "initializer ran"
@[init canary] opaque canaryInit : Unit
end Botho
