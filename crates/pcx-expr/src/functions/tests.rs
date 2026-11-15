// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use chrono::{TimeZone, Utc};

use crate::expression::Expression;
use crate::inputs::Inputs;
use crate::types::Type;
use crate::value::{Value, Values};

fn eval(source: &str) -> Result<Value, String> {
    let inputs = Inputs::new([("t".to_string(), Type::Timestamp)]).unwrap();
    let expr = Expression::check(source, &inputs).map_err(|e| e.to_string())?;
    let at = Utc.with_ymd_and_hms(2026, 3, 29, 0, 30, 0).unwrap();
    let values: Values = [("t".to_string(), Value::Timestamp(at))].into();
    expr.value(&values).map_err(|e| e.to_string())
}

fn ints(items: &[i64]) -> Value {
    Value::List(items.iter().map(|i| Value::Int(*i)).collect())
}

#[test]
fn lists_flatten_slice_range_sum_and_distinct() {
    assert_eq!(
        eval("lists.flatten([[1, 2], [], [3]])").unwrap(),
        ints(&[1, 2, 3])
    );
    assert_eq!(
        eval("lists.slice([1, 2, 3, 4], 1, 3)").unwrap(),
        ints(&[2, 3])
    );
    assert_eq!(eval("lists.range(4)").unwrap(), ints(&[0, 1, 2, 3]));
    assert_eq!(eval("lists.sum([1, 2, 3])").unwrap(), Value::Int(6));
    assert_eq!(eval("lists.sum([0.5, 0.25])").unwrap(), Value::Double(0.75));
    assert_eq!(
        eval("lists.sum([duration('1s'), duration('1.5s')])").unwrap(),
        Value::Duration(chrono::TimeDelta::milliseconds(2_500))
    );
    assert_eq!(
        eval("lists.distinct(['a', 'b', 'a', 'c', 'b'])").unwrap(),
        Value::list(["a", "b", "c"].map(|s| Value::String(s.into())).to_vec())
    );
}

#[test]
fn a_list_function_out_of_its_range_fails_rather_than_guesses() {
    assert!(
        eval("lists.slice([1, 2], 1, 5)")
            .unwrap_err()
            .contains("0 <= from")
    );
    assert!(eval("lists.range(-1)").unwrap_err().contains("lists.range"));
    assert!(
        eval("lists.range(2000000)")
            .unwrap_err()
            .contains("1000000")
    );
    assert!(
        eval("lists.sum([9223372036854775807, 1])")
            .unwrap_err()
            .contains("overflow")
    );
    assert!(
        eval("lists.distinct([1.5])")
            .unwrap_err()
            .contains("not a double")
    );
    // Types are checked when the expression is.
    assert!(eval("lists.flatten([1, 2])").is_err());
    assert!(eval("lists.sum(['a'])").is_err());
}

#[test]
fn times_parse_and_format_by_layout_in_utc_or_a_zone() {
    let at = |y, mo, d, h, mi| Value::Timestamp(Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap());
    assert_eq!(
        eval("time.parse('2026-11-01 10:30', '%Y-%m-%d %H:%M')").unwrap(),
        at(2026, 11, 1, 10, 30)
    );
    assert_eq!(
        eval("time.parse('01.11.2026', '%d.%m.%Y')").unwrap(),
        at(2026, 11, 1, 0, 0)
    );
    assert_eq!(
        eval("time.parse('2026-11-01 12:30', '%Y-%m-%d %H:%M', 'Europe/Sofia')").unwrap(),
        at(2026, 11, 1, 10, 30)
    );
    assert_eq!(
        eval("time.parse('2026-11-01T10:30:00+02:00', '%Y-%m-%dT%H:%M:%S%:z')").unwrap(),
        at(2026, 11, 1, 8, 30)
    );
    assert_eq!(
        eval("time.format(t, '%Y-%m-%d %H:%M')").unwrap(),
        Value::String("2026-03-29 00:30".into())
    );
    // The clock moved on that night in Sofia (EET to EEST at 03:00).
    assert_eq!(
        eval("time.format(t, '%H:%M %Z', 'Europe/Sofia')").unwrap(),
        Value::String("02:30 EET".into())
    );
}

#[test]
fn a_time_that_does_not_read_or_a_zone_that_is_none_fails() {
    assert!(
        eval("time.parse('soon', '%Y')")
            .unwrap_err()
            .contains("not a time")
    );
    assert!(
        eval("time.parse('2026', '%Y', 'Mars/Base')")
            .unwrap_err()
            .contains("IANA")
    );
    assert!(
        eval("time.format(t, '%Y', 'Mars/Base')")
            .unwrap_err()
            .contains("IANA")
    );
    assert!(
        eval("time.format(t, '%Q')")
            .unwrap_err()
            .contains("strftime")
    );
    // 03:30 does not exist in Sofia on 2026-03-29.
    assert!(
        eval("time.parse('2026-03-29 03:30', '%Y-%m-%d %H:%M', 'Europe/Sofia')")
            .unwrap_err()
            .contains("gap")
    );
}
