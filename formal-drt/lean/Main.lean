/-
`botho-model`: the Lean side of the DRT harness (#1706, #1707).

Reads JSON-lines requests on stdin and writes one JSON-lines response per
request on stdout, in order, flushing after each (the Rust runner keeps this
process alive for a whole run and streams batches through it). See
`Drt/Codec.lean` for the wire format.
-/
import Drt.Codec

partial def loop (stdin : IO.FS.Stream) (stdout : IO.FS.Stream) : IO Unit := do
  let line ← stdin.getLine
  if line.isEmpty then return
  let line := String.ofList (line.toList.filter fun c => c != '\n' && c != '\r')
  if !line.isEmpty then
    stdout.putStrLn (Drt.handleLine line).compress
    stdout.flush
  loop stdin stdout

def main : IO UInt32 := do
  if !Drt.Sha256.selfTest then
    IO.eprintln "botho-model: SHA-256 self-test failed"
    return 1
  loop (← IO.getStdin) (← IO.getStdout)
  return 0
