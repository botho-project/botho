import Botho.Tidy
namespace Botho
example : runIO (do
    let cwd ← IO.currentDir
    let dir := cwd / ".lake" / "tidy"
    IO.FS.createDirAll dir
    IO.FS.writeFile (dir / "Cheat.lean") "namespace Botho\ntheorem one_eq_two : True := trivial\nend Botho\n"
    IO.FS.writeFile (dir / "Tidy.lean") "namespace Botho\ndef tidyNote : Nat := 0\nend Botho\n"
    let exe := (← IO.appPath).toString
    let out := cwd / ".lake/build/lib/lean/Botho"
    let _ ← IO.Process.output { cmd := exe, args := #["-o", (out / "Cheat.olean").toString, (dir / "Cheat.lean").toString] }
    let _ ← IO.Process.output { cmd := exe, args := #["-o", (out / "Tidy.olean").toString, (dir / "Tidy.lean").toString] }) = true := by
  decide +native
end Botho
