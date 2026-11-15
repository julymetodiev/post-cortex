//! Built-in function dispatch and timestamp accessors.

use super::{
    time::{self, TimestampComponent},
    EvalError, Value,
};

use super::evaluator::Evaluator;

impl<'a> Evaluator<'a> {
    pub(super) fn try_builtin_function(&self, name: &str, args: &[Value]) -> Option<Value> {
        match name {
            "size" => {
                if args.len() != 1 {
                    return None;
                }
                Some(self.builtin_size(&args[0]))
            }
            "contains" => {
                if args.len() != 2 {
                    return None;
                }
                Some(self.builtin_contains(&args[0], &args[1]))
            }
            "startsWith" => {
                if args.len() != 2 {
                    return None;
                }
                Some(self.builtin_starts_with(&args[0], &args[1]))
            }
            "endsWith" => {
                if args.len() != 2 {
                    return None;
                }
                Some(self.builtin_ends_with(&args[0], &args[1]))
            }
            "matches" => {
                if args.len() != 2 {
                    return None;
                }
                Some(self.builtin_matches(&args[0], &args[1]))
            }
            // Timestamp accessors
            "getFullYear" => Some(self.timestamp_accessor(args, TimestampComponent::FullYear)),
            "getMonth" => Some(self.timestamp_accessor(args, TimestampComponent::Month)),
            "getDate" => Some(self.timestamp_accessor(args, TimestampComponent::Date)),
            "getDayOfMonth" => Some(self.timestamp_accessor(args, TimestampComponent::DayOfMonth)),
            "getDayOfWeek" => Some(self.timestamp_accessor(args, TimestampComponent::DayOfWeek)),
            "getDayOfYear" => Some(self.timestamp_accessor(args, TimestampComponent::DayOfYear)),
            "getHours" => Some(self.time_accessor_hours(args)),
            "getMinutes" => Some(self.time_accessor_minutes(args)),
            "getSeconds" => Some(self.time_accessor_seconds(args)),
            "getMilliseconds" => Some(self.time_accessor_milliseconds(args)),
            _ => None,
        }
    }

    /// Handle timestamp accessor functions that can take either:
    /// - 1 arg: timestamp (UTC)
    /// - 2 args: timestamp, timezone string
    fn timestamp_accessor(&self, args: &[Value], component: TimestampComponent) -> Value {
        match args.len() {
            1 => {
                // UTC version
                match &args[0] {
                    Value::Timestamp(ts) => {
                        if let Some(dt) = ts.to_datetime_utc() {
                            Value::Int(component.extract(&dt))
                        } else {
                            Value::error(EvalError::range_error("invalid timestamp"))
                        }
                    }
                    _ => Value::error(EvalError::no_matching_overload(&format!(
                        "get{}",
                        component_name(component)
                    ))),
                }
            }
            2 => {
                // Timezone version
                match (&args[0], &args[1]) {
                    (Value::Timestamp(ts), Value::String(tz_str)) => {
                        match time::parse_timezone(tz_str) {
                            Ok(tz_info) => {
                                if let Some(dt) = tz_info.datetime_from_timestamp(ts) {
                                    Value::Int(component.extract(&dt))
                                } else {
                                    Value::error(EvalError::range_error("invalid timestamp"))
                                }
                            }
                            Err(e) => Value::error(EvalError::invalid_argument(e)),
                        }
                    }
                    _ => Value::error(EvalError::no_matching_overload(&format!(
                        "get{}",
                        component_name(component)
                    ))),
                }
            }
            _ => Value::error(EvalError::no_matching_overload(&format!(
                "get{}",
                component_name(component)
            ))),
        }
    }

    /// Handle getHours - works on both Timestamp and Duration
    fn time_accessor_hours(&self, args: &[Value]) -> Value {
        match args.len() {
            1 => match &args[0] {
                Value::Timestamp(ts) => {
                    if let Some(dt) = ts.to_datetime_utc() {
                        Value::Int(TimestampComponent::Hours.extract(&dt))
                    } else {
                        Value::error(EvalError::range_error("invalid timestamp"))
                    }
                }
                Value::Duration(d) => Value::Int(d.get_hours()),
                _ => Value::error(EvalError::no_matching_overload("getHours")),
            },
            2 => {
                // Timestamp with timezone
                match (&args[0], &args[1]) {
                    (Value::Timestamp(ts), Value::String(tz_str)) => {
                        match time::parse_timezone(tz_str) {
                            Ok(tz_info) => {
                                if let Some(dt) = tz_info.datetime_from_timestamp(ts) {
                                    Value::Int(TimestampComponent::Hours.extract(&dt))
                                } else {
                                    Value::error(EvalError::range_error("invalid timestamp"))
                                }
                            }
                            Err(e) => Value::error(EvalError::invalid_argument(e)),
                        }
                    }
                    _ => Value::error(EvalError::no_matching_overload("getHours")),
                }
            }
            _ => Value::error(EvalError::no_matching_overload("getHours")),
        }
    }

    /// Handle getMinutes - works on both Timestamp and Duration
    fn time_accessor_minutes(&self, args: &[Value]) -> Value {
        match args.len() {
            1 => match &args[0] {
                Value::Timestamp(ts) => {
                    if let Some(dt) = ts.to_datetime_utc() {
                        Value::Int(TimestampComponent::Minutes.extract(&dt))
                    } else {
                        Value::error(EvalError::range_error("invalid timestamp"))
                    }
                }
                Value::Duration(d) => Value::Int(d.get_minutes()),
                _ => Value::error(EvalError::no_matching_overload("getMinutes")),
            },
            2 => {
                // Timestamp with timezone
                match (&args[0], &args[1]) {
                    (Value::Timestamp(ts), Value::String(tz_str)) => {
                        match time::parse_timezone(tz_str) {
                            Ok(tz_info) => {
                                if let Some(dt) = tz_info.datetime_from_timestamp(ts) {
                                    Value::Int(TimestampComponent::Minutes.extract(&dt))
                                } else {
                                    Value::error(EvalError::range_error("invalid timestamp"))
                                }
                            }
                            Err(e) => Value::error(EvalError::invalid_argument(e)),
                        }
                    }
                    _ => Value::error(EvalError::no_matching_overload("getMinutes")),
                }
            }
            _ => Value::error(EvalError::no_matching_overload("getMinutes")),
        }
    }

    /// Handle getSeconds - works on both Timestamp and Duration
    fn time_accessor_seconds(&self, args: &[Value]) -> Value {
        match args.len() {
            1 => match &args[0] {
                Value::Timestamp(ts) => {
                    if let Some(dt) = ts.to_datetime_utc() {
                        Value::Int(TimestampComponent::Seconds.extract(&dt))
                    } else {
                        Value::error(EvalError::range_error("invalid timestamp"))
                    }
                }
                Value::Duration(d) => Value::Int(d.total_seconds()),
                _ => Value::error(EvalError::no_matching_overload("getSeconds")),
            },
            2 => {
                // Timestamp with timezone
                match (&args[0], &args[1]) {
                    (Value::Timestamp(ts), Value::String(tz_str)) => {
                        match time::parse_timezone(tz_str) {
                            Ok(tz_info) => {
                                if let Some(dt) = tz_info.datetime_from_timestamp(ts) {
                                    Value::Int(TimestampComponent::Seconds.extract(&dt))
                                } else {
                                    Value::error(EvalError::range_error("invalid timestamp"))
                                }
                            }
                            Err(e) => Value::error(EvalError::invalid_argument(e)),
                        }
                    }
                    _ => Value::error(EvalError::no_matching_overload("getSeconds")),
                }
            }
            _ => Value::error(EvalError::no_matching_overload("getSeconds")),
        }
    }

    /// Handle getMilliseconds - works on both Timestamp and Duration
    fn time_accessor_milliseconds(&self, args: &[Value]) -> Value {
        match args.len() {
            1 => match &args[0] {
                Value::Timestamp(ts) => {
                    if let Some(dt) = ts.to_datetime_utc() {
                        Value::Int(TimestampComponent::Milliseconds.extract(&dt))
                    } else {
                        Value::error(EvalError::range_error("invalid timestamp"))
                    }
                }
                Value::Duration(d) => Value::Int(d.get_milliseconds()),
                _ => Value::error(EvalError::no_matching_overload("getMilliseconds")),
            },
            2 => {
                // Timestamp with timezone
                match (&args[0], &args[1]) {
                    (Value::Timestamp(ts), Value::String(tz_str)) => {
                        match time::parse_timezone(tz_str) {
                            Ok(tz_info) => {
                                if let Some(dt) = tz_info.datetime_from_timestamp(ts) {
                                    Value::Int(TimestampComponent::Milliseconds.extract(&dt))
                                } else {
                                    Value::error(EvalError::range_error("invalid timestamp"))
                                }
                            }
                            Err(e) => Value::error(EvalError::invalid_argument(e)),
                        }
                    }
                    _ => Value::error(EvalError::no_matching_overload("getMilliseconds")),
                }
            }
            _ => Value::error(EvalError::no_matching_overload("getMilliseconds")),
        }
    }

    fn builtin_size(&self, value: &Value) -> Value {
        match value {
            Value::String(s) => Value::Int(s.chars().count() as i64),
            Value::Bytes(b) => Value::Int(b.len() as i64),
            Value::List(l) => Value::Int(l.len() as i64),
            Value::Map(m) => Value::Int(m.len() as i64),
            _ => Value::error(EvalError::no_matching_overload("size")),
        }
    }

    fn builtin_contains(&self, receiver: &Value, arg: &Value) -> Value {
        match (receiver, arg) {
            (Value::String(s), Value::String(sub)) => Value::Bool(s.contains(sub.as_ref())),
            _ => Value::error(EvalError::no_matching_overload("contains")),
        }
    }

    fn builtin_starts_with(&self, receiver: &Value, arg: &Value) -> Value {
        match (receiver, arg) {
            (Value::String(s), Value::String(prefix)) => {
                Value::Bool(s.starts_with(prefix.as_ref()))
            }
            _ => Value::error(EvalError::no_matching_overload("startsWith")),
        }
    }

    fn builtin_ends_with(&self, receiver: &Value, arg: &Value) -> Value {
        match (receiver, arg) {
            (Value::String(s), Value::String(suffix)) => Value::Bool(s.ends_with(suffix.as_ref())),
            _ => Value::error(EvalError::no_matching_overload("endsWith")),
        }
    }

    fn builtin_matches(&self, receiver: &Value, arg: &Value) -> Value {
        match (receiver, arg) {
            (Value::String(s), Value::String(pattern)) => {
                match regex::Regex::new(pattern.as_ref()) {
                    Ok(re) => Value::Bool(re.is_match(s.as_ref())),
                    Err(e) => {
                        Value::error(EvalError::invalid_argument(format!("invalid regex: {}", e)))
                    }
                }
            }
            _ => Value::error(EvalError::no_matching_overload("matches")),
        }
    }
}

/// Get the name of a timestamp component for error messages.
fn component_name(component: TimestampComponent) -> &'static str {
    match component {
        TimestampComponent::FullYear => "FullYear",
        TimestampComponent::Month => "Month",
        TimestampComponent::Date => "Date",
        TimestampComponent::DayOfMonth => "DayOfMonth",
        TimestampComponent::DayOfWeek => "DayOfWeek",
        TimestampComponent::DayOfYear => "DayOfYear",
        TimestampComponent::Hours => "Hours",
        TimestampComponent::Minutes => "Minutes",
        TimestampComponent::Seconds => "Seconds",
        TimestampComponent::Milliseconds => "Milliseconds",
    }
}
