//! Arithmetic, comparison, and unary operators for CEL evaluation.

use std::sync::Arc;

use super::{EvalError, MapKey, Value};
use crate::types::{BinaryOp, SpannedExpr, UnaryOp};

use super::evaluator::Evaluator;

impl<'a> Evaluator<'a> {
    pub(super) fn eval_unary(&self, op: UnaryOp, expr: &SpannedExpr) -> Value {
        let value = self.eval_expr(expr);
        if value.is_error() {
            return value;
        }

        match op {
            UnaryOp::Neg => self.eval_negate(value),
            UnaryOp::Not => self.eval_not(value),
        }
    }

    fn eval_negate(&self, value: Value) -> Value {
        match value {
            Value::Int(i) => i
                .checked_neg()
                .map(Value::Int)
                .unwrap_or_else(|| Value::error(EvalError::overflow("integer negation overflow"))),
            Value::Double(d) => Value::Double(-d),
            _ => Value::error(EvalError::type_mismatch(
                "int or double",
                &value.cel_type().display_name(),
            )),
        }
    }

    fn eval_not(&self, value: Value) -> Value {
        match value {
            Value::Bool(b) => Value::Bool(!b),
            _ => Value::error(EvalError::type_mismatch(
                "bool",
                &value.cel_type().display_name(),
            )),
        }
    }

    pub(super) fn eval_binary(
        &self,
        op: BinaryOp,
        left: &SpannedExpr,
        right: &SpannedExpr,
    ) -> Value {
        // Short-circuit evaluation for && and ||
        match op {
            BinaryOp::And => return self.eval_and(left, right),
            BinaryOp::Or => return self.eval_or(left, right),
            _ => {}
        }

        // Evaluate both operands
        let left_val = self.eval_expr(left);
        if left_val.is_error() {
            return left_val;
        }

        let right_val = self.eval_expr(right);
        if right_val.is_error() {
            return right_val;
        }

        match op {
            BinaryOp::Add => self.eval_add(left_val, right_val),
            BinaryOp::Sub => self.eval_sub(left_val, right_val),
            BinaryOp::Mul => self.eval_mul(left_val, right_val),
            BinaryOp::Div => self.eval_div(left_val, right_val),
            BinaryOp::Mod => self.eval_mod(left_val, right_val),
            BinaryOp::Eq => self.eval_eq(left_val, right_val),
            BinaryOp::Ne => self.eval_ne(left_val, right_val),
            BinaryOp::Lt => self.eval_lt(left_val, right_val),
            BinaryOp::Le => self.eval_le(left_val, right_val),
            BinaryOp::Gt => self.eval_gt(left_val, right_val),
            BinaryOp::Ge => self.eval_ge(left_val, right_val),
            BinaryOp::In => self.eval_in(left_val, right_val),
            BinaryOp::And | BinaryOp::Or => unreachable!("handled above"),
        }
    }

    fn eval_add(&self, left: Value, right: Value) -> Value {
        match (&left, &right) {
            (Value::Int(a), Value::Int(b)) => a
                .checked_add(*b)
                .map(Value::Int)
                .unwrap_or_else(|| Value::error(EvalError::overflow("integer addition overflow"))),
            (Value::UInt(a), Value::UInt(b)) => a
                .checked_add(*b)
                .map(Value::UInt)
                .unwrap_or_else(|| Value::error(EvalError::overflow("unsigned addition overflow"))),
            (Value::Double(a), Value::Double(b)) => Value::Double(a + b),
            (Value::String(a), Value::String(b)) => {
                let mut result = String::with_capacity(a.len() + b.len());
                result.push_str(a);
                result.push_str(b);
                Value::String(Arc::from(result))
            }
            (Value::Bytes(a), Value::Bytes(b)) => {
                let mut result = Vec::with_capacity(a.len() + b.len());
                result.extend_from_slice(a);
                result.extend_from_slice(b);
                Value::Bytes(Arc::from(result))
            }
            (Value::List(a), Value::List(b)) => {
                let mut result = Vec::with_capacity(a.len() + b.len());
                result.extend(a.iter().cloned());
                result.extend(b.iter().cloned());
                Value::List(Arc::from(result))
            }
            (Value::Map(a), Value::Map(b)) => {
                let mut result = a.as_ref().clone();
                for (key, value) in b.iter() {
                    result.insert(key.clone(), value.clone());
                }
                Value::Map(Arc::new(result))
            }
            (Value::Timestamp(t), Value::Duration(d)) => {
                // Normalize: timestamp nanos are always 0..999_999_999
                // Duration nanos can be negative for negative durations
                let nanos = t.nanos as i64 + d.nanos as i64;
                let (extra_secs, nanos) = normalize_nanos(nanos);

                match t
                    .seconds
                    .checked_add(d.seconds)
                    .and_then(|s| s.checked_add(extra_secs))
                {
                    Some(seconds) => {
                        let ts = super::Timestamp::new(seconds, nanos);
                        if ts.is_valid() {
                            Value::Timestamp(ts)
                        } else {
                            Value::error(EvalError::range_error(
                                "timestamp out of range: must be between year 0001 and 9999",
                            ))
                        }
                    }
                    None => Value::error(EvalError::overflow("timestamp addition overflow")),
                }
            }
            (Value::Duration(d), Value::Timestamp(t)) => {
                let nanos = t.nanos as i64 + d.nanos as i64;
                let (extra_secs, nanos) = normalize_nanos(nanos);

                match t
                    .seconds
                    .checked_add(d.seconds)
                    .and_then(|s| s.checked_add(extra_secs))
                {
                    Some(seconds) => {
                        let ts = super::Timestamp::new(seconds, nanos);
                        if ts.is_valid() {
                            Value::Timestamp(ts)
                        } else {
                            Value::error(EvalError::range_error(
                                "timestamp out of range: must be between year 0001 and 9999",
                            ))
                        }
                    }
                    None => Value::error(EvalError::overflow("timestamp addition overflow")),
                }
            }
            (Value::Duration(a), Value::Duration(b)) => {
                let nanos = a.nanos as i64 + b.nanos as i64;
                let (extra_secs, nanos) = normalize_nanos(nanos);

                match a
                    .seconds
                    .checked_add(b.seconds)
                    .and_then(|s| s.checked_add(extra_secs))
                {
                    Some(seconds) => {
                        let d = super::Duration::new(seconds, nanos);
                        if d.is_valid() {
                            Value::Duration(d)
                        } else {
                            Value::error(EvalError::range_error(
                                "duration out of range: must be within approximately 10000 years",
                            ))
                        }
                    }
                    None => Value::error(EvalError::overflow("duration addition overflow")),
                }
            }
            _ => Value::error(EvalError::no_matching_overload("_+_")),
        }
    }

    fn eval_sub(&self, left: Value, right: Value) -> Value {
        match (&left, &right) {
            (Value::Int(a), Value::Int(b)) => {
                a.checked_sub(*b).map(Value::Int).unwrap_or_else(|| {
                    Value::error(EvalError::overflow("integer subtraction overflow"))
                })
            }
            (Value::UInt(a), Value::UInt(b)) => {
                a.checked_sub(*b).map(Value::UInt).unwrap_or_else(|| {
                    Value::error(EvalError::overflow("unsigned subtraction overflow"))
                })
            }
            (Value::Double(a), Value::Double(b)) => Value::Double(a - b),
            (Value::Timestamp(a), Value::Timestamp(b)) => {
                let nanos = a.nanos as i64 - b.nanos as i64;
                let (extra_secs, nanos) = normalize_nanos(nanos);

                match a
                    .seconds
                    .checked_sub(b.seconds)
                    .and_then(|s| s.checked_add(extra_secs))
                {
                    Some(seconds) => {
                        let d = super::Duration::new(seconds, nanos);
                        if d.is_valid() {
                            Value::Duration(d)
                        } else {
                            Value::error(EvalError::range_error(
                                "duration out of range: must be within approximately 10000 years",
                            ))
                        }
                    }
                    None => Value::error(EvalError::overflow("timestamp subtraction overflow")),
                }
            }
            (Value::Timestamp(t), Value::Duration(d)) => {
                let nanos = t.nanos as i64 - d.nanos as i64;
                let (extra_secs, nanos) = normalize_nanos(nanos);

                match t
                    .seconds
                    .checked_sub(d.seconds)
                    .and_then(|s| s.checked_add(extra_secs))
                {
                    Some(seconds) => {
                        let ts = super::Timestamp::new(seconds, nanos);
                        if ts.is_valid() {
                            Value::Timestamp(ts)
                        } else {
                            Value::error(EvalError::range_error(
                                "timestamp out of range: must be between year 0001 and 9999",
                            ))
                        }
                    }
                    None => Value::error(EvalError::overflow("timestamp subtraction overflow")),
                }
            }
            (Value::Duration(a), Value::Duration(b)) => {
                let nanos = a.nanos as i64 - b.nanos as i64;
                let (extra_secs, nanos) = normalize_nanos(nanos);

                match a
                    .seconds
                    .checked_sub(b.seconds)
                    .and_then(|s| s.checked_add(extra_secs))
                {
                    Some(seconds) => {
                        let d = super::Duration::new(seconds, nanos);
                        if d.is_valid() {
                            Value::Duration(d)
                        } else {
                            Value::error(EvalError::range_error(
                                "duration out of range: must be within approximately 10000 years",
                            ))
                        }
                    }
                    None => Value::error(EvalError::overflow("duration subtraction overflow")),
                }
            }
            _ => Value::error(EvalError::no_matching_overload("_-_")),
        }
    }

    fn eval_mul(&self, left: Value, right: Value) -> Value {
        match (&left, &right) {
            (Value::Int(a), Value::Int(b)) => {
                a.checked_mul(*b).map(Value::Int).unwrap_or_else(|| {
                    Value::error(EvalError::overflow("integer multiplication overflow"))
                })
            }
            (Value::UInt(a), Value::UInt(b)) => {
                a.checked_mul(*b).map(Value::UInt).unwrap_or_else(|| {
                    Value::error(EvalError::overflow("unsigned multiplication overflow"))
                })
            }
            (Value::Double(a), Value::Double(b)) => Value::Double(a * b),
            _ => Value::error(EvalError::no_matching_overload("_*_")),
        }
    }

    fn eval_div(&self, left: Value, right: Value) -> Value {
        match (&left, &right) {
            (Value::Int(_), Value::Int(0)) => Value::error(EvalError::division_by_zero()),
            (Value::Int(a), Value::Int(b)) => a
                .checked_div(*b)
                .map(Value::Int)
                .unwrap_or_else(|| Value::error(EvalError::overflow("integer division overflow"))),
            (Value::UInt(_), Value::UInt(0)) => Value::error(EvalError::division_by_zero()),
            (Value::UInt(a), Value::UInt(b)) => Value::UInt(a / b),
            (Value::Double(a), Value::Double(b)) => Value::Double(a / b),
            _ => Value::error(EvalError::no_matching_overload("_/_")),
        }
    }

    fn eval_mod(&self, left: Value, right: Value) -> Value {
        match (&left, &right) {
            (Value::Int(_), Value::Int(0)) => Value::error(EvalError::modulo_by_zero()),
            (Value::Int(a), Value::Int(b)) => a
                .checked_rem(*b)
                .map(Value::Int)
                .unwrap_or_else(|| Value::error(EvalError::overflow("integer modulo overflow"))),
            (Value::UInt(_), Value::UInt(0)) => Value::error(EvalError::modulo_by_zero()),
            (Value::UInt(a), Value::UInt(b)) => Value::UInt(a % b),
            _ => Value::error(EvalError::no_matching_overload("_%_")),
        }
    }

    fn eval_eq(&self, left: Value, right: Value) -> Value {
        Value::Bool(left == right)
    }

    fn eval_ne(&self, left: Value, right: Value) -> Value {
        Value::Bool(left != right)
    }

    fn eval_lt(&self, left: Value, right: Value) -> Value {
        match left.compare(&right) {
            Some(std::cmp::Ordering::Less) => Value::Bool(true),
            Some(_) => Value::Bool(false),
            None => Value::error(EvalError::no_matching_overload("_<_")),
        }
    }

    fn eval_le(&self, left: Value, right: Value) -> Value {
        match left.compare(&right) {
            Some(std::cmp::Ordering::Less) | Some(std::cmp::Ordering::Equal) => Value::Bool(true),
            Some(_) => Value::Bool(false),
            None => Value::error(EvalError::no_matching_overload("_<=_")),
        }
    }

    fn eval_gt(&self, left: Value, right: Value) -> Value {
        match left.compare(&right) {
            Some(std::cmp::Ordering::Greater) => Value::Bool(true),
            Some(_) => Value::Bool(false),
            None => Value::error(EvalError::no_matching_overload("_>_")),
        }
    }

    fn eval_ge(&self, left: Value, right: Value) -> Value {
        match left.compare(&right) {
            Some(std::cmp::Ordering::Greater) | Some(std::cmp::Ordering::Equal) => {
                Value::Bool(true)
            }
            Some(_) => Value::Bool(false),
            None => Value::error(EvalError::no_matching_overload("_>=_")),
        }
    }

    fn eval_in(&self, left: Value, right: Value) -> Value {
        match &right {
            Value::List(list) => {
                for elem in list.iter() {
                    if left == *elem {
                        return Value::Bool(true);
                    }
                }
                Value::Bool(false)
            }
            Value::Map(map) => {
                // Handle double keys: coerce exact integers to int/uint for lookup
                if let Value::Double(d) = &left {
                    if d.is_finite() && d.fract() == 0.0 {
                        let as_int = *d as i64;
                        let int_key = MapKey::Int(as_int);
                        return Value::Bool(map.contains_key_with_numeric_coercion(&int_key));
                    } else {
                        return Value::Bool(false);
                    }
                }
                if let Some(key) = MapKey::from_value(&left) {
                    Value::Bool(map.contains_key_with_numeric_coercion(&key))
                } else {
                    Value::error(EvalError::type_mismatch(
                        "valid map key",
                        &left.cel_type().display_name(),
                    ))
                }
            }
            _ => Value::error(EvalError::no_matching_overload("_in_")),
        }
    }
}

/// Normalize nanoseconds to be in the range 0..999_999_999 for timestamps.
/// Returns (extra_seconds, normalized_nanos).
pub(super) fn normalize_nanos(nanos: i64) -> (i64, i32) {
    if (0..1_000_000_000).contains(&nanos) {
        (0, nanos as i32)
    } else if nanos >= 1_000_000_000 {
        let extra_secs = nanos / 1_000_000_000;
        let nanos = (nanos % 1_000_000_000) as i32;
        (extra_secs, nanos)
    } else {
        // Negative nanos - need to borrow from seconds
        // e.g., -999999998 nanos -> -1 second + 2 nanos
        let abs_nanos = -nanos;
        let borrow_secs = (abs_nanos + 999_999_999) / 1_000_000_000;
        let remaining = (borrow_secs * 1_000_000_000 - abs_nanos) as i32;
        (-borrow_secs, remaining)
    }
}
