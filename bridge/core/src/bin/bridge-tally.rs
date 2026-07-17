// Copyright (c) 2024 The Botho Foundation

//! `bridge-tally` — the deterministic bridge-election tally CLI
//! (ADR 0010 option C / A1).
//!
//! A thin, dependency-free wrapper over the pure [`bth_bridge_core::election`]
//! library. Given a JSON bundle of `{ params, snapshot, ledger }`, it runs the
//! `approval-top-N-v1` tally and prints the `elected`-status v2 term document
//! (or the failure mode). It also builds/signs the two memo formats, so the
//! #1063 drill and operators can produce ballots without re-implementing the
//! canonical encoding.
//!
//! Everything is deterministic: same input → same output, byte for byte.
//!
//! ```text
//! bridge-tally tally <input.json> [--ranking] [--out <file>]
//! bridge-tally build-nomination --node-id <id> --term <n> [--secret-key-hex <hex>]
//! bridge-tally build-ballot --node-id <id> --term <n> --approve <a,b,c> [--secret-key-hex <hex>]
//! ```
//!
//! Exit codes for `tally`: 0 elected, 3 no-quorum, 4 insufficient candidates,
//! 1 usage/parse error.

use std::process::ExitCode;

use bth_bridge_core::election::{
    assemble_elected_term_doc, canonical_ballot_memo, canonical_nomination_memo,
    sign_election_memo_ed25519, tally, CurationSnapshot, ElectionParams, MemoTransaction,
    TallyResult, TallyStatus,
};
use ed25519_dalek::SigningKey;
use serde::Deserialize;

/// The JSON input bundle for the `tally` subcommand.
#[derive(Debug, Deserialize)]
struct TallyInput {
    params: ElectionParams,
    snapshot: CurationSnapshot,
    ledger: Vec<MemoTransaction>,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("{}", USAGE);
        return ExitCode::from(1);
    }

    let result = match args[0].as_str() {
        "tally" => cmd_tally(&args[1..]),
        "build-nomination" => cmd_build_nomination(&args[1..]),
        "build-ballot" => cmd_build_ballot(&args[1..]),
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        other => Err(format!("unknown subcommand `{other}`\n\n{USAGE}")),
    };

    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}

const USAGE: &str = "\
bridge-tally — deterministic bridge-election tally (ADR 0010 option C / A1)

USAGE:
  bridge-tally tally <input.json> [--ranking] [--out <file>]
  bridge-tally build-nomination --node-id <id> --term <n> [--secret-key-hex <hex>]
  bridge-tally build-ballot --node-id <id> --term <n> --approve <a,b,c> [--secret-key-hex <hex>]

The `tally` input is a JSON object { \"params\": {...}, \"snapshot\": {...}, \"ledger\": [...] }.
On an elected result it prints the elected-status v2 term document to stdout.

EXIT CODES (tally): 0 elected, 3 no-quorum, 4 insufficient candidates, 1 error.";

fn cmd_tally(args: &[String]) -> Result<ExitCode, String> {
    let mut input_path: Option<String> = None;
    let mut out_path: Option<String> = None;
    let mut show_ranking = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--ranking" => show_ranking = true,
            "--out" => {
                i += 1;
                out_path = Some(args.get(i).ok_or("--out needs a value")?.clone());
            }
            other if other.starts_with("--") => return Err(format!("unknown flag `{other}`")),
            other => {
                if input_path.is_some() {
                    return Err(format!("unexpected argument `{other}`"));
                }
                input_path = Some(other.to_string());
            }
        }
        i += 1;
    }

    let path = input_path.ok_or("tally needs an <input.json> path")?;
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let input: TallyInput =
        serde_json::from_str(&raw).map_err(|e| format!("invalid tally input JSON: {e}"))?;

    let result = tally(&input.snapshot, &input.params, &input.ledger)?;

    if show_ranking {
        eprintln!("ranking:");
        for c in &result.ranking {
            eprintln!("  {:>2}. {} ({} approvals)", c.rank, c.node_id, c.approvals);
        }
        eprintln!(
            "turnout {}/{} (quorum {}), status {:?}, resultHash {}",
            result.turnout,
            result.eligible,
            result.quorum_required,
            result.status,
            result.result_hash
        );
    }

    match result.status {
        TallyStatus::Elected => {
            let doc = assemble_elected_term_doc(&input.snapshot, &input.params, &result)?;
            let json = doc.to_json_pretty();
            emit(&json, out_path.as_deref())?;
            Ok(ExitCode::SUCCESS)
        }
        TallyStatus::NoQuorum => {
            report_void(&result, "no-quorum");
            Ok(ExitCode::from(3))
        }
        TallyStatus::InsufficientCandidates => {
            report_void(&result, "insufficient-candidates");
            Ok(ExitCode::from(4))
        }
    }
}

fn report_void(result: &TallyResult, tag: &str) {
    eprintln!(
        "election VOID ({tag}): turnout {}/{} (quorum {}), {} candidate(s), resultHash {}",
        result.turnout,
        result.eligible,
        result.quorum_required,
        result.ranking.len(),
        result.result_hash
    );
}

fn emit(json: &str, out_path: Option<&str>) -> Result<(), String> {
    match out_path {
        Some(p) => {
            std::fs::write(p, format!("{json}\n")).map_err(|e| format!("cannot write {p}: {e}"))
        }
        None => {
            println!("{json}");
            Ok(())
        }
    }
}

fn cmd_build_nomination(args: &[String]) -> Result<ExitCode, String> {
    let opts = parse_kv(args)?;
    let node_id = opts.get("node-id").ok_or("--node-id is required")?;
    let term = parse_term(&opts)?;
    let memo = canonical_nomination_memo(node_id, term);
    print_memo(&memo, opts.get("secret-key-hex"))?;
    Ok(ExitCode::SUCCESS)
}

fn cmd_build_ballot(args: &[String]) -> Result<ExitCode, String> {
    let opts = parse_kv(args)?;
    let node_id = opts.get("node-id").ok_or("--node-id is required")?;
    let term = parse_term(&opts)?;
    let approvals: Vec<String> = opts
        .get("approve")
        .map(|s| {
            s.split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let memo = canonical_ballot_memo(node_id, term, &approvals);
    print_memo(&memo, opts.get("secret-key-hex"))?;
    Ok(ExitCode::SUCCESS)
}

fn parse_term(opts: &std::collections::HashMap<String, String>) -> Result<u64, String> {
    opts.get("term")
        .ok_or("--term is required")?
        .parse::<u64>()
        .map_err(|_| "--term must be a non-negative integer".to_string())
}

/// Print a memo (and, if a secret key is supplied, its detached signature) as
/// a JSON object ready to drop into a ledger fixture.
fn print_memo(memo: &str, secret_key_hex: Option<&String>) -> Result<(), String> {
    let signature_hex = match secret_key_hex {
        Some(hex_str) => {
            let raw = hex::decode(hex_str.trim()).map_err(|_| "secret key is not valid hex")?;
            let bytes: [u8; 32] = raw
                .as_slice()
                .try_into()
                .map_err(|_| "ed25519 secret key must be 32 bytes")?;
            let key = SigningKey::from_bytes(&bytes);
            Some(sign_election_memo_ed25519(memo, &key))
        }
        None => None,
    };
    match signature_hex {
        Some(sig) => println!(
            "{{\"memo\":{},\"signatureHex\":{}}}",
            serde_json::to_string(memo).unwrap(),
            serde_json::to_string(&sig).unwrap()
        ),
        None => println!("{{\"memo\":{}}}", serde_json::to_string(memo).unwrap()),
    }
    Ok(())
}

/// Parse `--key value` pairs into a map (keys without the `--` prefix).
fn parse_kv(args: &[String]) -> Result<std::collections::HashMap<String, String>, String> {
    let mut map = std::collections::HashMap::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        let key = arg
            .strip_prefix("--")
            .ok_or_else(|| format!("expected a --flag, got `{arg}`"))?;
        let value = args
            .get(i + 1)
            .ok_or_else(|| format!("flag `--{key}` needs a value"))?;
        map.insert(key.to_string(), value.clone());
        i += 2;
    }
    Ok(map)
}
