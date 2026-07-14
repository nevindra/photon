//! Shared OTLP `AnyValue`/id helpers used by both the logs and traces mappers.

use opentelemetry_proto::tonic::common::v1::{any_value::Value, AnyValue};

/// Stringify an `AnyValue`, consuming it. Nested kinds are flattened to a best-effort textual
/// form. For the overwhelmingly common `StringValue` case this moves the inner `String` out
/// instead of cloning it — callers that already own the `AnyValue` should prefer this.
pub(crate) fn any_value_into_string(v: AnyValue) -> String {
    match v.value {
        Some(Value::StringValue(s)) => s,
        Some(Value::BoolValue(b)) => b.to_string(),
        Some(Value::IntValue(i)) => i.to_string(),
        Some(Value::DoubleValue(d)) => d.to_string(),
        Some(Value::BytesValue(b)) => bytes_to_hex(&b),
        Some(Value::ArrayValue(arr)) => {
            let items: Vec<String> = arr.values.into_iter().map(any_value_into_string).collect();
            format!("[{}]", items.join(","))
        }
        Some(Value::KvlistValue(kv)) => {
            let items: Vec<String> = kv
                .values
                .into_iter()
                .map(|entry| {
                    let val = entry.value.map(any_value_into_string).unwrap_or_default();
                    format!("{}={}", entry.key, val)
                })
                .collect();
            format!("{{{}}}", items.join(","))
        }
        None => String::new(),
    }
}

/// Nibble → lowercase hex digit lookup, avoiding a `format!` allocation per byte.
const HEX: &[u8; 16] = b"0123456789abcdef";

/// Lowercase hex encoding, e.g. for trace/span ids.
pub(crate) fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

pub(crate) fn bytes_to_hex_opt(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        None
    } else {
        Some(bytes_to_hex(bytes))
    }
}
