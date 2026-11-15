// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The functions every expression gains beside CEL's standard library and
//! `cel-core`'s extensions (docs/modules/workflow.md, data blocks): lists
//! (`lists.flatten`, `lists.slice`, `lists.range`, `lists.sum`,
//! `lists.distinct`) and times (`time.parse`, `time.format`, in UTC or a
//! zone as the IANA names it).

use std::collections::BTreeSet;
use std::fmt::Write;
use std::sync::Arc;

use cel_core::{CelType, Duration, EvalError, FunctionDecl, OverloadDecl, Timestamp, Value as Cel};
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;

/// Items `lists.range` gives at most: a block's items at most.
pub const MAX_RANGE: i64 = 1_000_000;

/// The functions, declared with their implementations.
pub fn all() -> Vec<FunctionDecl> {
    vec![
        flatten(),
        slice(),
        range(),
        sum(),
        distinct(),
        parse(),
        format(),
    ]
}

/// A list of `T`.
fn list_of(t: CelType) -> CelType {
    CelType::list(t)
}

/// A failed call: `why`.
fn failed(why: impl Into<String>) -> Cel {
    Cel::error(EvalError::invalid_argument(why.into()))
}

fn flatten() -> FunctionDecl {
    let t = || CelType::type_param("T");
    FunctionDecl::new("lists.flatten").with_overload(
        OverloadDecl::function(
            "lists_flatten_list_list",
            vec![list_of(list_of(t()))],
            list_of(t()),
        )
        .with_type_params(vec!["T".into()])
        .with_impl(|args| match &args[0] {
            Cel::List(lists) => {
                let mut out = Vec::new();
                for list in lists.iter() {
                    match list {
                        Cel::List(items) => out.extend(items.iter().cloned()),
                        _ => return failed("lists.flatten takes a list of lists"),
                    }
                }
                Cel::from(out)
            }
            _ => failed("lists.flatten takes a list of lists"),
        }),
    )
}

fn slice() -> FunctionDecl {
    let t = || CelType::type_param("T");
    FunctionDecl::new("lists.slice").with_overload(
        OverloadDecl::function(
            "lists_slice_list_int_int",
            vec![list_of(t()), CelType::Int, CelType::Int],
            list_of(t()),
        )
        .with_type_params(vec!["T".into()])
        .with_impl(|args| match (&args[0], &args[1], &args[2]) {
            (Cel::List(items), Cel::Int(from), Cel::Int(to)) => {
                let len = items.len();
                let at = |i: i64| usize::try_from(i).ok().filter(|i| *i <= len);
                match (at(*from), at(*to)) {
                    (Some(from), Some(to)) if from <= to => Cel::from(items[from..to].to_vec()),
                    _ => failed(format!(
                        "lists.slice({from}, {to}) of {len} items: 0 <= from <= to <= {len}"
                    )),
                }
            }
            _ => failed("lists.slice takes a list and two ints"),
        }),
    )
}

fn range() -> FunctionDecl {
    FunctionDecl::new("lists.range").with_overload(
        OverloadDecl::function("lists_range_int", vec![CelType::Int], list_of(CelType::Int))
            .with_impl(|args| match &args[0] {
                Cel::Int(n) if (0..=MAX_RANGE).contains(n) => {
                    Cel::from((0..*n).map(Cel::Int).collect::<Vec<_>>())
                }
                Cel::Int(n) => failed(format!("lists.range({n}): 0 to {MAX_RANGE}")),
                _ => failed("lists.range takes an int"),
            }),
    )
}

fn sum() -> FunctionDecl {
    FunctionDecl::new("lists.sum")
        .with_overload(
            OverloadDecl::function("lists_sum_int", vec![list_of(CelType::Int)], CelType::Int)
                .with_impl(|args| {
                    fold(&args[0], Cel::Int(0), |acc, v| match (acc, v) {
                        (Cel::Int(a), Cel::Int(b)) => a.checked_add(*b).map(Cel::Int),
                        _ => None,
                    })
                }),
        )
        .with_overload(
            OverloadDecl::function(
                "lists_sum_uint",
                vec![list_of(CelType::UInt)],
                CelType::UInt,
            )
            .with_impl(|args| {
                fold(&args[0], Cel::UInt(0), |acc, v| match (acc, v) {
                    (Cel::UInt(a), Cel::UInt(b)) => a.checked_add(*b).map(Cel::UInt),
                    _ => None,
                })
            }),
        )
        .with_overload(
            OverloadDecl::function(
                "lists_sum_double",
                vec![list_of(CelType::Double)],
                CelType::Double,
            )
            .with_impl(|args| {
                fold(&args[0], Cel::Double(0.0), |acc, v| match (acc, v) {
                    (Cel::Double(a), Cel::Double(b)) => Some(Cel::Double(a + b)),
                    _ => None,
                })
            }),
        )
        .with_overload(
            OverloadDecl::function(
                "lists_sum_duration",
                vec![list_of(CelType::Duration)],
                CelType::Duration,
            )
            .with_impl(|args| {
                fold(
                    &args[0],
                    Cel::Duration(Duration::new(0, 0)),
                    |acc, v| match (acc, v) {
                        (Cel::Duration(a), Cel::Duration(b)) => a
                            .to_nanos()
                            .checked_add(b.to_nanos())
                            .map(|n| Cel::Duration(nanos(n))),
                        _ => None,
                    },
                )
            }),
        )
}

/// `list`'s items added by `add` from `zero`; an overflow fails.
fn fold(list: &Cel, zero: Cel, add: impl Fn(&Cel, &Cel) -> Option<Cel>) -> Cel {
    let Cel::List(items) = list else {
        return failed("lists.sum takes a list");
    };
    let mut acc = zero;
    for item in items.iter() {
        match add(&acc, item) {
            Some(next) => acc = next,
            None => return failed("lists.sum overflowed, or an item is of another type"),
        }
    }
    acc
}

/// `n` nanoseconds as CEL's duration.
fn nanos(n: i64) -> Duration {
    // The remainder of a division by 10^9 is within an i32.
    Duration::new(n / 1_000_000_000, (n % 1_000_000_000) as i32)
}

/// A key of `lists.distinct`: a value two items share when they are one.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Bool(bool),
    Int(i64),
    Uint(u64),
    Text(Arc<str>),
}

fn distinct() -> FunctionDecl {
    let t = || CelType::type_param("T");
    FunctionDecl::new("lists.distinct").with_overload(
        OverloadDecl::function("lists_distinct_list", vec![list_of(t())], list_of(t()))
            .with_type_params(vec!["T".into()])
            .with_impl(|args| {
                let Cel::List(items) = &args[0] else {
                    return failed("lists.distinct takes a list");
                };
                let mut seen = BTreeSet::new();
                let mut out = Vec::new();
                for item in items.iter() {
                    let key = match item {
                        Cel::Bool(b) => Key::Bool(*b),
                        Cel::Int(i) => Key::Int(*i),
                        Cel::UInt(u) => Key::Uint(*u),
                        Cel::String(s) => Key::Text(s.clone()),
                        other => {
                            return failed(format!(
                                "lists.distinct takes strings, ints, uints or bools, not a {}",
                                other.cel_type().display_name()
                            ));
                        }
                    };
                    if seen.insert(key) {
                        out.push(item.clone());
                    }
                }
                Cel::from(out)
            }),
    )
}

fn parse() -> FunctionDecl {
    let s = || CelType::String;
    FunctionDecl::new("time.parse")
        .with_overload(
            OverloadDecl::function(
                "time_parse_string_string",
                vec![s(), s()],
                CelType::Timestamp,
            )
            .with_impl(|args| match (&args[0], &args[1]) {
                (Cel::String(text), Cel::String(layout)) => parsed(text, layout, None),
                _ => failed("time.parse takes a text and a layout"),
            }),
        )
        .with_overload(
            OverloadDecl::function(
                "time_parse_string_string_string",
                vec![s(), s(), s()],
                CelType::Timestamp,
            )
            .with_impl(|args| match (&args[0], &args[1], &args[2]) {
                (Cel::String(text), Cel::String(layout), Cel::String(zone)) => {
                    match zone.parse::<Tz>() {
                        Ok(zone) => parsed(text, layout, Some(zone)),
                        Err(_) => failed(format!("\"{zone}\" is no IANA time zone")),
                    }
                }
                _ => failed("time.parse takes a text, a layout and a zone"),
            }),
        )
}

/// `text` read by `layout` (`strftime`): with its offset when the layout
/// has one, else a time (or a date, at midnight) in `zone` (none: UTC).
fn parsed(text: &str, layout: &str, zone: Option<Tz>) -> Cel {
    if let Ok(t) = DateTime::parse_from_str(text, layout) {
        return Cel::Timestamp(Timestamp::from_datetime(&t.with_timezone(&Utc)));
    }
    let naive = NaiveDateTime::parse_from_str(text, layout).or_else(|_| {
        NaiveDate::parse_from_str(text, layout).map(|d| d.and_time(chrono::NaiveTime::MIN))
    });
    let Ok(naive) = naive else {
        return failed(format!(
            "\"{text}\" is not a time of the layout \"{layout}\""
        ));
    };
    let utc = match zone {
        None => Some(naive.and_utc()),
        Some(zone) => zone
            .from_local_datetime(&naive)
            .earliest()
            .map(|t| t.with_timezone(&Utc)),
    };
    match utc {
        Some(t) => Cel::Timestamp(Timestamp::from_datetime(&t)),
        None => failed(format!(
            "\"{text}\" is no time in its zone (a gap of the clock)"
        )),
    }
}

fn format() -> FunctionDecl {
    let s = || CelType::String;
    FunctionDecl::new("time.format")
        .with_overload(
            OverloadDecl::function(
                "time_format_timestamp_string",
                vec![CelType::Timestamp, s()],
                s(),
            )
            .with_impl(|args| match (&args[0], &args[1]) {
                (Cel::Timestamp(t), Cel::String(layout)) => match t.to_datetime_utc() {
                    Some(t) => formatted(&t, layout),
                    None => failed("a time out of range"),
                },
                _ => failed("time.format takes a time and a layout"),
            }),
        )
        .with_overload(
            OverloadDecl::function(
                "time_format_timestamp_string_string",
                vec![CelType::Timestamp, s(), s()],
                s(),
            )
            .with_impl(|args| match (&args[0], &args[1], &args[2]) {
                (Cel::Timestamp(t), Cel::String(layout), Cel::String(zone)) => {
                    match (t.to_datetime_utc(), zone.parse::<Tz>()) {
                        (Some(t), Ok(zone)) => formatted(&t.with_timezone(&zone), layout),
                        (None, _) => failed("a time out of range"),
                        (_, Err(_)) => failed(format!("\"{zone}\" is no IANA time zone")),
                    }
                }
                _ => failed("time.format takes a time, a layout and a zone"),
            }),
        )
}

/// `t` written by `layout` (`strftime`); a layout chrono does not read
/// fails rather than panics.
fn formatted<Z: TimeZone>(t: &DateTime<Z>, layout: &str) -> Cel
where
    Z::Offset: std::fmt::Display,
{
    let mut out = String::new();
    match write!(out, "{}", t.format(layout)) {
        Ok(()) => Cel::String(Arc::from(out.as_str())),
        Err(_) => failed(format!("\"{layout}\" is no strftime layout")),
    }
}

#[cfg(test)]
mod tests;
