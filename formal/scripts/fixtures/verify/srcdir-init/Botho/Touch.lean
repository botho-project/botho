namespace Botho
def touch : IO Unit := IO.FS.writeFile "PWNED_INIT" "x"
@[init touch] opaque touchInit : Unit

/-- Self-test probe: if BOTHO_INIT_CANARY is set, record that this initializer
ran. selftest-verify.sh checks that it never runs inside the verifier. -/
def canary : IO Unit := do
  if let some p ← IO.getEnv "BOTHO_INIT_CANARY" then
    IO.FS.writeFile p "initializer ran"
@[init canary] opaque canaryInit : Unit
end Botho
