//! The buildings a sheet or view title names (Q6292): a set drawn once for several buildings
//! says so in its titles (`WATER PIPING PLAN - EAST & WEST BLDGS`), and a plan drawn for one of
//! them names it (`POWER PLAN E. BLDG`, `BUILDING A - FLOOR PLAN`).
//!
//! The words are rule data (`wordrules.json`): `buildings.words` (one building: `BUILDING`,
//! `BLDG`), `buildings.plural_words` (several: `BUILDINGS`, `BLDGS`), `buildings.joiners` (`&`,
//! `AND`, `,`), `buildings.number_words` (`NO`, `#`) and `buildings.compass_names` (a side of the
//! site, written out or abbreviated, and the one name both read as).
//!
//! What counts as a building's name:
//! - after the building word, a short mark: one letter, a number of up to three digits, or a
//!   letter and digits (`BUILDING A`, `BLDG NO. 2`, `BLDG 1A`), or a side of the site written out
//!   (`BUILDING NORTH`); several joined (`BUILDINGS A, B AND C`, `BLDG A & B`);
//! - before it, a side of the site (`E. BLDG`, `WEST BUILDING`, `EAST & WEST BLDGS`), or
//!   several marks joined (`A & B BUILDINGS`). One letter or a number alone before the word is
//!   not a name: `A BUILDING` is an article, `(2) BUILDINGS` a count.
//!
//! A side written out or abbreviated reads as the same name (`E.` and `EAST` are `EAST`) before
//! the building word; after it one letter stays a letter (`BUILDING E` is building E of a lettered
//! set). `EXISTING BUILDING`, `BUILDING ELEVATIONS`, `BUILDING AND WALL SECTIONS` name none.

use crate::wordrules;

/// Longest title read, characters (hostile input).
const MAX_TITLE: usize = 400;
/// Most names one title gives.
const MAX_NAMES: usize = 16;

/// One token of a title: the word upper-cased, or a joining mark.
fn tokens(title: &str) -> Vec<String> {
    // A mark that may join names (`&`, `,`, `+`, `/`) or say a number follows (`#`) is a token
    // of its own; any other mark (`.`, `-`, `(`) only separates.
    let marks: Vec<char> = wordrules::words("buildings.joiners")
        .iter()
        .chain(wordrules::words("buildings.number_words"))
        .filter_map(|w| {
            let mut cs = w.chars();
            cs.next().filter(|c| !c.is_alphanumeric() && cs.next().is_none())
        })
        .collect();
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for c in title.chars().take(MAX_TITLE) {
        if c.is_alphanumeric() {
            cur.extend(c.to_uppercase());
            continue;
        }
        if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        if marks.contains(&c) {
            out.push(c.to_string());
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn is_one(t: &str) -> bool {
    wordrules::words("buildings.words").contains(&t)
}

fn is_many(t: &str) -> bool {
    wordrules::words("buildings.plural_words").contains(&t)
}

fn is_joiner(t: &str) -> bool {
    wordrules::words("buildings.joiners").contains(&t)
}

fn is_number_word(t: &str) -> bool {
    wordrules::words("buildings.number_words").contains(&t)
}

/// A side of the site, as the one name its spellings read as.
fn compass(t: &str) -> Option<&'static str> {
    wordrules::pair_value("buildings.compass_names", t)
}

/// A side written out (`NORTH`, not `N`): its own name.
fn compass_word(t: &str) -> Option<&'static str> {
    compass(t).filter(|n| *n == t)
}

/// A short mark: one letter, up to three digits, or a letter and digits (`A`, `2`, `1A`, `B2`).
fn mark(t: &str) -> bool {
    let n = t.chars().count();
    let letters = t.chars().filter(char::is_ascii_alphabetic).count();
    let digits = t.chars().filter(char::is_ascii_digit).count();
    t.chars().all(|c| c.is_ascii_alphanumeric())
        && ((letters == 1 && n == 1) || (letters == 0 && (1..=3).contains(&digits)) || (letters == 1 && digits >= 1 && n <= 3))
}

/// The name a token after the building word gives: a side written out, else a mark.
fn name_after(t: &str) -> Option<String> {
    compass_word(t).map(str::to_string).or_else(|| mark(t).then(|| t.to_string()))
}

/// The buildings `title` names, in its order, each once (empty: it names none).
pub fn named_in(title: &str) -> Vec<String> {
    let t = tokens(title);
    let mut out: Vec<String> = Vec::new();
    let push = |n: String, out: &mut Vec<String>| {
        if out.len() < MAX_NAMES && !out.contains(&n) {
            out.push(n);
        }
    };
    for (i, w) in t.iter().enumerate() {
        if !is_one(w) && !is_many(w) {
            continue;
        }
        // Before the word: the names joined right up to it.
        let is_name = |x: &str| compass(x).is_some() || mark(x);
        let mut before: Vec<&str> = Vec::new();
        let mut j = i;
        while let Some(prev) = j.checked_sub(1).and_then(|p| t.get(p)).filter(|x| is_name(x)) {
            before.push(prev);
            j -= 1;
            // Another name only across a joining word.
            let joins = j.checked_sub(1).and_then(|p| t.get(p)).is_some_and(|x| is_joiner(x));
            let more = j.checked_sub(2).and_then(|p| t.get(p)).is_some_and(|x| is_name(x));
            if joins && more {
                j -= 1;
            } else {
                break;
            }
        }
        before.reverse();
        let joined = before.len() > 1;
        let sides = before.iter().all(|b| compass(b).is_some());
        if !before.is_empty() && (sides || joined) {
            for b in before {
                push(compass(b).map_or_else(|| b.to_string(), str::to_string), &mut out);
            }
            continue;
        }
        // After the word: a name, more across joining words.
        let mut k = i + 1;
        while t.get(k).is_some_and(|x| is_number_word(x)) {
            k += 1;
        }
        let mut after: Vec<String> = Vec::new();
        while let Some(n) = t.get(k).and_then(|x| name_after(x)) {
            after.push(n);
            k += 1;
            if t.get(k).is_some_and(|x| is_joiner(x)) && t.get(k + 1).and_then(|x| name_after(x)).is_some() {
                k += 1;
            } else {
                break;
            }
        }
        for n in after {
            push(n, &mut out);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(t: &str) -> Vec<String> {
        named_in(t)
    }

    #[test]
    fn several_buildings_named_before_the_plural_word() {
        assert_eq!(names("SANITARY PLAN - NORTHEAST & SOUTHWEST BLDGS."), ["NORTHEAST", "SOUTHWEST"]);
        assert_eq!(names("Mechanical HVAC Plan - East and West Buildings"), ["EAST", "WEST"]);
        assert_eq!(names("FLOOR PLAN, A & B BUILDINGS"), ["A", "B"]);
    }

    #[test]
    fn one_building_named_by_its_side_abbreviated_or_written_out() {
        assert_eq!(names("POWER PLAN E. BLDG"), ["EAST"]);
        assert_eq!(names("LTG PLAN SW.BLDG"), ["SOUTHWEST"]);
        assert_eq!(names("NORTH BUILDING LIGHTING PLAN"), ["NORTH"]);
    }

    #[test]
    fn names_after_the_building_word() {
        assert_eq!(names("BUILDING A - FLOOR PLAN"), ["A"]);
        assert_eq!(names("BLDG NO. 2 POWER PLAN"), ["2"]);
        assert_eq!(names("BLDG #3 PLAN"), ["3"]);
        assert_eq!(names("BUILDINGS A, B AND C - SITE UTILITIES"), ["A", "B", "C"]);
        assert_eq!(names("BUILDING 1A FLOOR PLAN"), ["1A"]);
        // One letter after the word stays a letter of a lettered set.
        assert_eq!(names("BUILDING E PLAN"), ["E"]);
    }

    #[test]
    fn titles_that_name_no_building() {
        for t in [
            "BUILDING ELEVATIONS",
            "BUILDING AND WALL SECTIONS",
            "EXISTING BUILDING",
            "REMOVE EXISTING BUILDING",
            "A BUILDING PERMIT IS REQUIRED",
            "(2) BUILDINGS",
            "FEED TO THE NEW BUILDING",
            "FLOOR PLAN, ROOF PLAN AND REFLECTED CEILING PLAN",
            "NORTH",
            "",
        ] {
            assert!(names(t).is_empty(), "{t}: {:?}", names(t));
        }
    }

    #[test]
    fn each_name_once_in_the_titles_order() {
        assert_eq!(names("WEST BLDG PLAN (SEE ALSO EAST BLDG; W. BLDG)"), ["WEST", "EAST"]);
    }

    #[test]
    fn hostile_titles_are_capped() {
        let long = "N & ".repeat(5000) + "BLDGS";
        assert!(names(&long).len() <= MAX_NAMES);
        let many: String = (0..200).map(|i| format!("BUILDING {i} ")).collect();
        assert_eq!(names(&many).len(), MAX_NAMES);
    }
}
