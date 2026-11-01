//! The Lean model process: one long-lived `botho-model` child, fed batches of
//! JSON-lines requests.

use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

pub struct Model {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    /// Requests sent so far (all legs).
    pub calls: u64,
}

impl Model {
    /// Start the executable at `path`.
    pub fn spawn(path: &Path) -> Result<Self, String> {
        let mut child = Command::new(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| {
                format!(
                    "cannot start the Lean model at {} ({e}); build it with \
                     `lake build` in formal-drt/lean",
                    path.display()
                )
            })?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        Ok(Self {
            child,
            stdin,
            stdout,
            calls: 0,
        })
    }

    /// Send every request line and read one response per request, in order.
    /// A writer thread feeds stdin while this thread drains stdout, so a batch
    /// larger than the pipe buffers cannot deadlock. Each response is the
    /// `ok` value, or `{"lean_err": message}` when the model rejected the
    /// request (a harness bug, which then shows up as a mismatch).
    pub fn call_batch(&mut self, lines: &[String]) -> Vec<Value> {
        let stdin = &mut self.stdin;
        let stdout = &mut self.stdout;
        let out = std::thread::scope(|s| {
            let writer = s.spawn(move || {
                for line in lines {
                    stdin
                        .write_all(line.as_bytes())
                        .and_then(|_| stdin.write_all(b"\n"))
                        .expect("write to the Lean model");
                }
                stdin.flush().expect("flush the Lean model's stdin");
            });
            let mut out = Vec::with_capacity(lines.len());
            let mut buf = String::new();
            for _ in lines {
                buf.clear();
                let n = stdout
                    .read_line(&mut buf)
                    .expect("read from the Lean model");
                assert!(n > 0, "the Lean model exited early");
                let v: Value = serde_json::from_str(&buf).expect("Lean model wrote invalid JSON");
                out.push(match (v.get("ok"), v.get("err")) {
                    (Some(ok), _) => ok.clone(),
                    (None, Some(err)) => serde_json::json!({ "lean_err": err }),
                    _ => panic!("malformed Lean response: {buf}"),
                });
            }
            writer.join().expect("writer thread");
            out
        });
        self.calls += lines.len() as u64;
        out
    }
}

impl Drop for Model {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
