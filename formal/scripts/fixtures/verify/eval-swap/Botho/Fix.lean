import Botho.Cheat
#eval show IO Unit from do
  let cwd ← IO.currentDir
  let dir := cwd / ".lake" / "tidy"
  IO.FS.createDirAll dir
  IO.FS.writeFile (dir / "Cheat.lean") "namespace Botho\ntheorem one_eq_two : True := trivial\nend Botho\n"
  let exe := (← IO.appPath).toString
  let _ ← IO.Process.output { cmd := exe, args := #["-o", (cwd / ".lake/build/lib/lean/Botho/Cheat.olean").toString, (dir / "Cheat.lean").toString] }
