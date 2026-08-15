// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

//! What the lint should and should not say. Each case below is here because it is a distinct
//! claim about the matcher, not because it is a distinct piece of code.

struct Row {
    weight: u32,
    count: u32,
    enabled: bool,
}

// ---------------------------------------------------------------------------
// A renamed copy. Every local is spelled differently and one literal changed;
// nothing structural did. This is the case the alpha-renaming exists for, and
// the diagnostic should name the correspondence rather than just flag it.
// ---------------------------------------------------------------------------

fn total(rows: &[Row]) -> u32 {
    let mut sum = 0;
    for row in rows {
        if row.enabled {
            sum += row.weight * row.count;
        } else {
            sum += row.weight;
        }
    }
    if sum > 100 {
        sum = 100;
    }
    sum
}

fn tally(items: &[Row]) -> u32 {
    let mut acc = 0;
    for item in items {
        if item.enabled {
            acc += item.weight * item.count;
        } else {
            acc += item.weight;
        }
    }
    if acc > 200 {
        acc = 200;
    }
    acc
}

// ---------------------------------------------------------------------------
// A duplicated *stretch* rather than a duplicated function. Both of these do
// their own unrelated work either side of the same six statements, so only a
// matcher that considers runs of statements can see it.
// ---------------------------------------------------------------------------

fn report_widths(rows: &[Row]) -> (u32, String) {
    let mut widest = 0;
    for row in rows {
        if row.weight > widest {
            widest = row.weight;
        }
    }
    let mut label = String::new();
    label.push_str("width=");
    (widest, label)
}

fn report_counts(rows: &[Row]) -> (u32, u32) {
    let mut widest = 0;
    for row in rows {
        if row.weight > widest {
            widest = row.weight;
        }
    }
    let mut seen = 0;
    seen += rows.len() as u32;
    (widest, seen)
}

// ---------------------------------------------------------------------------
// Not a clone. Same size, same rough shape, different logic — this one must
// stay silent, or the lint is just a size detector.
// ---------------------------------------------------------------------------

fn describe(kind: u32) -> String {
    match kind {
        0 => String::from("zero"),
        1 => String::from("one"),
        2 => String::from("two"),
        3 => String::from("three"),
        _ => String::new(),
    }
}

fn main() {
    let rows = [Row {
        weight: 1,
        count: 2,
        enabled: true,
    }];
    println!(
        "{:?} {:?} {:?} {:?} {:?}",
        total(&rows),
        tally(&rows),
        report_widths(&rows),
        report_counts(&rows),
        describe(1)
    );
}
