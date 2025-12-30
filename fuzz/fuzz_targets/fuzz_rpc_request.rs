#![no_main]

//! Fuzzing target for JSON-RPC request parsing.
//!
//! Security rationale: The RPC endpoint is exposed to external clients (exchanges,
//! wallets, web interfaces). Malformed requests must not crash the server, leak
//! information, or cause resource exhaustion.

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ============================================================================
// JSON-RPC Request Structure (mirroring botho::rpc types)
// ============================================================================

/// JSON-RPC 2.0 request format
#[derive(Debug, Deserialize, Serialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    method: String,
    params: Value,
    id: Value,
}

// ============================================================================
// Structured Fuzzing Types
// ============================================================================

/// Fuzz mode selector
#[derive(Debug, Arbitrary)]
enum FuzzMode {
    /// Raw JSON bytes
    RawJson(Vec<u8>),
    /// Malformed JSON variations
    MalformedJson(MalformedJson),
    /// Valid structure with fuzzed content
    StructuredRequest(FuzzRequest),
    /// Method name fuzzing
    MethodFuzz(MethodFuzz),
}

/// Malformed JSON test cases
#[derive(Debug, Arbitrary)]
enum MalformedJson {
    /// Truncated JSON
    Truncated(Vec<u8>, u8),
    /// Invalid UTF-8
    InvalidUtf8(Vec<u8>),
    /// Nested objects (potential stack overflow)
    DeepNesting(u8),
    /// Very long strings
    LongString(u16),
    /// Binary data in JSON
    BinaryInJson(Vec<u8>),
}

/// Structured request for fuzzing
#[derive(Debug, Arbitrary)]
struct FuzzRequest {
    /// JSON-RPC version (should be "2.0")
    version: FuzzVersion,
    /// Method name
    method: String,
    /// Parameter object
    params: FuzzParams,
    /// Request ID
    id: FuzzId,
}

#[derive(Debug, Arbitrary)]
enum FuzzVersion {
    Valid,
    Empty,
    Wrong(String),
    Number(i64),
    Null,
}

#[derive(Debug, Arbitrary)]
enum FuzzParams {
    Null,
    EmptyObject,
    EmptyArray,
    Object(Vec<(String, FuzzValue)>),
    Array(Vec<FuzzValue>),
}

#[derive(Debug, Arbitrary)]
enum FuzzValue {
    Null,
    Bool(bool),
    Number(i64),
    String(String),
    Hex(Vec<u8>),
}

#[derive(Debug, Arbitrary)]
enum FuzzId {
    Null,
    Number(i64),
    String(String),
    Float(f64),
}

/// Method name fuzzing
#[derive(Debug, Arbitrary)]
struct MethodFuzz {
    /// Base method name
    base: String,
    /// Prefix
    prefix: Option<String>,
    /// Suffix
    suffix: Option<String>,
    /// Include special characters
    special_chars: bool,
}

// ============================================================================
// Fuzz Target
// ============================================================================

fuzz_target!(|mode: FuzzMode| {
    match mode {
        FuzzMode::RawJson(data) => {
            fuzz_raw_json(&data);
        }
        FuzzMode::MalformedJson(malformed) => {
            fuzz_malformed_json(malformed);
        }
        FuzzMode::StructuredRequest(request) => {
            fuzz_structured_request(&request);
        }
        FuzzMode::MethodFuzz(method) => {
            fuzz_method(&method);
        }
    }
});

/// Fuzz raw JSON bytes
fn fuzz_raw_json(data: &[u8]) {
    // Try to parse as JSON
    let _ = serde_json::from_slice::<Value>(data);

    // Try to parse as JSON-RPC request specifically
    let _ = serde_json::from_slice::<JsonRpcRequest>(data);

    // Try UTF-8 conversion first
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = serde_json::from_str::<Value>(s);
        let _ = serde_json::from_str::<JsonRpcRequest>(s);
    }
}

/// Fuzz malformed JSON variations
fn fuzz_malformed_json(malformed: MalformedJson) {
    match malformed {
        MalformedJson::Truncated(data, cut_point) => {
            let cut = (cut_point as usize) % (data.len() + 1);
            let truncated = &data[..cut];
            let _ = serde_json::from_slice::<Value>(truncated);
        }

        MalformedJson::InvalidUtf8(data) => {
            // This might not be valid UTF-8, but parsing should not panic
            let _ = serde_json::from_slice::<Value>(&data);
        }

        MalformedJson::DeepNesting(depth) => {
            // Limit depth to prevent stack overflow in fuzzer itself
            let depth = (depth % 50) as usize;
            let mut json = String::new();
            for _ in 0..depth {
                json.push_str("{\"a\":");
            }
            json.push_str("null");
            for _ in 0..depth {
                json.push('}');
            }
            let _ = serde_json::from_str::<Value>(&json);
        }

        MalformedJson::LongString(len) => {
            // Limit length to prevent OOM
            let len = (len % 10000) as usize;
            let long_str = format!("{{\"method\":\"{}\"}}", "a".repeat(len));
            let _ = serde_json::from_str::<JsonRpcRequest>(&long_str);
        }

        MalformedJson::BinaryInJson(data) => {
            // Embed binary as hex in a JSON string
            let hex_data = hex::encode(&data);
            let json = format!("{{\"method\":\"test\",\"params\":{{\"data\":\"{}\"}},\"id\":1,\"jsonrpc\":\"2.0\"}}", hex_data);
            let _ = serde_json::from_str::<JsonRpcRequest>(&json);
        }
    }
}

/// Fuzz structured requests
fn fuzz_structured_request(request: &FuzzRequest) {
    // Build JSON object
    let version = match &request.version {
        FuzzVersion::Valid => Value::String("2.0".to_string()),
        FuzzVersion::Empty => Value::String(String::new()),
        FuzzVersion::Wrong(s) => Value::String(s.clone()),
        FuzzVersion::Number(n) => Value::Number((*n).into()),
        FuzzVersion::Null => Value::Null,
    };

    let params = match &request.params {
        FuzzParams::Null => Value::Null,
        FuzzParams::EmptyObject => Value::Object(serde_json::Map::new()),
        FuzzParams::EmptyArray => Value::Array(vec![]),
        FuzzParams::Object(pairs) => {
            let mut map = serde_json::Map::new();
            for (k, v) in pairs.iter().take(10) {
                map.insert(k.clone(), fuzz_value_to_json(v));
            }
            Value::Object(map)
        }
        FuzzParams::Array(items) => {
            let arr: Vec<Value> = items.iter().take(10).map(fuzz_value_to_json).collect();
            Value::Array(arr)
        }
    };

    let id = match &request.id {
        FuzzId::Null => Value::Null,
        FuzzId::Number(n) => Value::Number((*n).into()),
        FuzzId::String(s) => Value::String(s.clone()),
        FuzzId::Float(f) => {
            if let Some(n) = serde_json::Number::from_f64(*f) {
                Value::Number(n)
            } else {
                Value::Null
            }
        }
    };

    let json_obj = serde_json::json!({
        "jsonrpc": version,
        "method": request.method,
        "params": params,
        "id": id
    });

    // Serialize and parse back
    if let Ok(json_str) = serde_json::to_string(&json_obj) {
        let _ = serde_json::from_str::<JsonRpcRequest>(&json_str);
    }
}

fn fuzz_value_to_json(v: &FuzzValue) -> Value {
    match v {
        FuzzValue::Null => Value::Null,
        FuzzValue::Bool(b) => Value::Bool(*b),
        FuzzValue::Number(n) => Value::Number((*n).into()),
        FuzzValue::String(s) => Value::String(s.clone()),
        FuzzValue::Hex(data) => Value::String(hex::encode(data)),
    }
}

/// Fuzz method names
fn fuzz_method(method: &MethodFuzz) {
    let mut name = String::new();

    if let Some(prefix) = &method.prefix {
        name.push_str(&prefix.chars().take(50).collect::<String>());
    }

    name.push_str(&method.base.chars().take(100).collect::<String>());

    if let Some(suffix) = &method.suffix {
        name.push_str(&suffix.chars().take(50).collect::<String>());
    }

    if method.special_chars {
        // Add some special characters that might break parsing
        for c in ['\\', '"', '\n', '\r', '\t', '\0'].iter() {
            let test_name = format!("{}{}", name, c);
            let json = format!(
                "{{\"jsonrpc\":\"2.0\",\"method\":{},\"params\":{{}},\"id\":1}}",
                serde_json::to_string(&test_name).unwrap_or_default()
            );
            let _ = serde_json::from_str::<JsonRpcRequest>(&json);
        }
    }

    // Test the method name in a request
    let json = format!(
        "{{\"jsonrpc\":\"2.0\",\"method\":{},\"params\":{{}},\"id\":1}}",
        serde_json::to_string(&name).unwrap_or_default()
    );
    let _ = serde_json::from_str::<JsonRpcRequest>(&json);
}
