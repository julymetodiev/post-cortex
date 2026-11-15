//! Type conversion functions for CEL values.

use std::sync::Arc;

use super::{time, EvalError, Value};

use super::evaluator::Evaluator;

impl<'a> Evaluator<'a> {
    pub(super) fn try_type_conversion(&self, name: &str, args: &[Value]) -> Option<Value> {
        if args.len() != 1 {
            return None;
        }

        let arg = &args[0];
        match name {
            "int" => Some(self.convert_to_int(arg)),
            "uint" => Some(self.convert_to_uint(arg)),
            "double" => Some(self.convert_to_double(arg)),
            "string" => Some(self.convert_to_string(arg)),
            "bytes" => Some(self.convert_to_bytes(arg)),
            "bool" => Some(self.convert_to_bool(arg)),
            "type" => Some(Value::Type(arg.type_value())),
            "dyn" => Some(arg.clone()),
            "timestamp" => Some(self.convert_to_timestamp(arg)),
            "duration" => Some(self.convert_to_duration(arg)),
            _ => None,
        }
    }

    fn convert_to_int(&self, value: &Value) -> Value {
        match value {
            Value::Int(i) => Value::Int(*i),
            Value::UInt(u) => {
                if *u > i64::MAX as u64 {
                    Value::error(EvalError::overflow("uint to int overflow"))
                } else {
                    Value::Int(*u as i64)
                }
            }
            Value::Double(d) => {
                if d.is_nan()
                    || d.is_infinite()
                    || *d >= (i64::MAX as f64)
                    || *d <= (i64::MIN as f64)
                {
                    Value::error(EvalError::overflow("double to int overflow"))
                } else {
                    Value::Int(*d as i64)
                }
            }
            Value::String(s) => s
                .parse::<i64>()
                .map(Value::Int)
                .unwrap_or_else(|_| Value::error(EvalError::invalid_conversion("string", "int"))),
            Value::Timestamp(t) => Value::Int(t.seconds),
            Value::Enum(e) => Value::Int(e.value as i64),
            _ => Value::error(EvalError::invalid_conversion(
                &value.cel_type().display_name(),
                "int",
            )),
        }
    }

    fn convert_to_uint(&self, value: &Value) -> Value {
        match value {
            Value::UInt(u) => Value::UInt(*u),
            Value::Int(i) => {
                if *i < 0 {
                    Value::error(EvalError::overflow("negative int to uint"))
                } else {
                    Value::UInt(*i as u64)
                }
            }
            Value::Double(d) => {
                if d.is_nan() || d.is_infinite() || *d < 0.0 || *d >= (u64::MAX as f64) {
                    Value::error(EvalError::overflow("double to uint overflow"))
                } else {
                    Value::UInt(*d as u64)
                }
            }
            Value::String(s) => s
                .parse::<u64>()
                .map(Value::UInt)
                .unwrap_or_else(|_| Value::error(EvalError::invalid_conversion("string", "uint"))),
            _ => Value::error(EvalError::invalid_conversion(
                &value.cel_type().display_name(),
                "uint",
            )),
        }
    }

    fn convert_to_double(&self, value: &Value) -> Value {
        match value {
            Value::Double(d) => Value::Double(*d),
            Value::Int(i) => Value::Double(*i as f64),
            Value::UInt(u) => Value::Double(*u as f64),
            Value::String(s) => s.parse::<f64>().map(Value::Double).unwrap_or_else(|_| {
                Value::error(EvalError::invalid_conversion("string", "double"))
            }),
            _ => Value::error(EvalError::invalid_conversion(
                &value.cel_type().display_name(),
                "double",
            )),
        }
    }

    fn convert_to_string(&self, value: &Value) -> Value {
        match value {
            Value::String(s) => Value::String(s.clone()),
            Value::Int(i) => Value::String(Arc::from(i.to_string())),
            Value::UInt(u) => Value::String(Arc::from(u.to_string())),
            Value::Double(d) => Value::String(Arc::from(format_double(*d))),
            Value::Bool(b) => Value::String(Arc::from(b.to_string())),
            Value::Bytes(b) => match std::str::from_utf8(b) {
                Ok(s) => Value::String(Arc::from(s)),
                Err(_) => Value::error(EvalError::invalid_conversion("bytes", "string")),
            },
            Value::Timestamp(t) => {
                // Format as RFC 3339 with nanoseconds
                Value::String(Arc::from(time::format_timestamp(t)))
            }
            Value::Duration(d) => {
                // Format as CEL duration string
                Value::String(Arc::from(time::format_duration(d)))
            }
            _ => Value::error(EvalError::invalid_conversion(
                &value.cel_type().display_name(),
                "string",
            )),
        }
    }

    fn convert_to_bytes(&self, value: &Value) -> Value {
        match value {
            Value::Bytes(b) => Value::Bytes(b.clone()),
            Value::String(s) => Value::Bytes(Arc::from(s.as_bytes())),
            _ => Value::error(EvalError::invalid_conversion(
                &value.cel_type().display_name(),
                "bytes",
            )),
        }
    }

    fn convert_to_bool(&self, value: &Value) -> Value {
        match value {
            Value::Bool(b) => Value::Bool(*b),
            Value::String(s) => match s.as_ref() {
                "true" | "True" | "TRUE" | "t" | "1" => Value::Bool(true),
                "false" | "False" | "FALSE" | "f" | "0" => Value::Bool(false),
                _ => Value::error(EvalError::invalid_conversion("string", "bool")),
            },
            _ => Value::error(EvalError::invalid_conversion(
                &value.cel_type().display_name(),
                "bool",
            )),
        }
    }

    fn convert_to_timestamp(&self, value: &Value) -> Value {
        match value {
            Value::Timestamp(t) => Value::Timestamp(*t),
            Value::String(s) => match time::parse_timestamp(s) {
                Ok(ts) => Value::Timestamp(ts),
                Err(e) => Value::error(EvalError::invalid_argument(e)),
            },
            Value::Int(i) => {
                let ts = super::Timestamp::from_seconds(*i);
                if ts.is_valid() {
                    Value::Timestamp(ts)
                } else {
                    Value::error(EvalError::range_error(
                        "timestamp out of range: must be between year 0001 and 9999",
                    ))
                }
            }
            _ => Value::error(EvalError::invalid_conversion(
                &value.cel_type().display_name(),
                "timestamp",
            )),
        }
    }

    fn convert_to_duration(&self, value: &Value) -> Value {
        match value {
            Value::Duration(d) => Value::Duration(*d),
            Value::String(s) => match time::parse_duration(s) {
                Ok(d) => Value::Duration(d),
                Err(e) => Value::error(EvalError::invalid_argument(e)),
            },
            Value::Int(i) => {
                let d = super::Duration::from_seconds(*i);
                if d.is_valid() {
                    Value::Duration(d)
                } else {
                    Value::error(EvalError::range_error(
                        "duration out of range: must be within approximately 10000 years",
                    ))
                }
            }
            _ => Value::error(EvalError::invalid_conversion(
                &value.cel_type().display_name(),
                "duration",
            )),
        }
    }
}

/// Format a double value according to CEL conventions.
pub(super) fn format_double(d: f64) -> String {
    if d.is_nan() {
        "NaN".to_string()
    } else if d.is_infinite() {
        if d.is_sign_positive() {
            "+infinity".to_string()
        } else {
            "-infinity".to_string()
        }
    } else if d.fract() == 0.0 && d.abs() < 1e15 {
        format!("{:.1}", d)
    } else {
        d.to_string()
    }
}
