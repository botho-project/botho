namespace Botho
/-- An initializer and nothing else: the verifier must reject it without
running it. -/
def canary : IO Unit := do
  if let some p ← IO.getEnv "BOTHO_INIT_CANARY" then
    IO.FS.writeFile p "initializer ran"
@[init canary] opaque canaryInit : Unit
end Botho
