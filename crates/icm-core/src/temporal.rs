//! Dates for recall: parse an instant supplied by a caller, and find the time
//! window a query talks about ("last week", "il y a trois jours", "in March
//! 2023").
//!
//! Rule-based, English and French, no LLM and no `regex`. A "day" is a civil
//! day at the UTC offset the caller passes (UTC itself by default), a "week"
//! an ISO week (Monday to Sunday). Relative expressions are resolved against
//! the `now` the caller passes, never against the wall clock, so a benchmark
//! can ask "last week" from 2023.
//!
//! A wrong window costs more than a missed one, so the rules give up when a
//! number or a word could be something other than a date: "in 2048 chunks",
//! "Mars 2020 rover", "the last week of the sprint", "this year's kernel".

use chrono::{
    DateTime, Datelike, Days, FixedOffset, Months, NaiveDate, NaiveDateTime, Offset, TimeZone, Utc,
};

use crate::error::{IcmError, IcmResult};

/// Years accepted by "in YYYY" / "<month> YYYY". A bare four-digit number
/// outside this range is far more likely a quantity than a year.
const MIN_YEAR: i32 = 1990;
const MAX_YEAR: i32 = 2100;

/// Years `parse_instant` accepts. Outside this range chrono writes a signed,
/// five-digit year (`+12345-06-01T...`) that RFC 3339 readers, the store's
/// included, cannot parse back.
const MIN_INSTANT_YEAR: i32 = 1;
const MAX_INSTANT_YEAR: i32 = 9999;

/// A half-open time range: `start` is included, `end` is excluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeWindow {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl TimeWindow {
    /// Whether `t` falls inside the window (`start <= t < end`).
    pub fn contains(&self, t: DateTime<Utc>) -> bool {
        t >= self.start && t < self.end
    }
}

/// Parse an instant supplied by a caller (HTTP `created_at` / `now`).
///
/// Accepted forms: RFC 3339 (`2023-05-30T23:40:00+02:00`, `...Z`);
/// `YYYY-MM-DDTHH:MM:SS` without an offset (a space is accepted in place of
/// the `T`, fractional seconds are allowed), taken as UTC; `YYYY-MM-DD`,
/// midnight UTC. Anything else is `IcmError::InvalidInput`, and so is an
/// instant whose year, once converted to UTC, is outside 1 to 9999: it could
/// be stored but never read back.
pub fn parse_instant(s: &str) -> IcmResult<DateTime<Utc>> {
    let s = s.trim();
    let parsed = if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        Some(dt.with_timezone(&Utc))
    } else {
        ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"]
            .iter()
            .find_map(|fmt| NaiveDateTime::parse_from_str(s, fmt).ok())
            .or_else(|| {
                NaiveDate::parse_from_str(s, "%Y-%m-%d")
                    .ok()
                    .and_then(|day| day.and_hms_opt(0, 0, 0))
            })
            .map(|naive| Utc.from_utc_datetime(&naive))
    };
    let Some(instant) = parsed else {
        return Err(IcmError::InvalidInput(format!(
            "invalid date '{s}' (expected RFC 3339, YYYY-MM-DDTHH:MM:SS or YYYY-MM-DD)"
        )));
    };
    if !(MIN_INSTANT_YEAR..=MAX_INSTANT_YEAR).contains(&instant.year()) {
        return Err(IcmError::InvalidInput(format!(
            "invalid date '{s}': year {} is out of range ({MIN_INSTANT_YEAR} to {MAX_INSTANT_YEAR})",
            instant.year()
        )));
    }
    Ok(instant)
}

/// [`parse_query_window_at`] with civil days cut in UTC. Use it when the
/// caller supplies `now` without saying where it is (HTTP, benchmarks).
pub fn parse_query_window(query: &str, now: DateTime<Utc>) -> Option<TimeWindow> {
    parse_query_window_at(query, now, Utc.fix())
}

/// Find the time window a query refers to, if it names one.
///
/// `offset` is the UTC offset of the person asking: "yesterday" at 01:30 in
/// Paris is the Parisian yesterday, not the UTC one. It is a fixed offset,
/// so a window months away across a daylight-saving change is off by that
/// hour. Pass `*chrono::Local::now().offset()` for a local user.
///
/// Matching is case-insensitive and on whole words. When several
/// expressions are present the most explicit one wins, in this order:
///
/// 1. an ISO date (`2023-04-12`): that day;
/// 2. a named month: `7 May 2023` / `May 7, 2023` (the day), `June 2023`,
///    `in March 2023`, `en mars` (the month; without a year, the most recent
///    one that has started). "may", "march", "mars" and "mai" are ordinary
///    words (and a planet): they need their preposition (`in` / `en` / `au
///    mois de`) or a day number even with a year, and without a year the
///    expression must also end the clause ("in may be fixed" is no date);
/// 3. `N days|weeks|months|years ago`, `il y a N jours|semaines|mois|ans`,
///    N in digits or spelled out up to twelve: centred on `now` minus N
///    units, with a tolerance of one day, four days, sixteen days and six
///    months respectively. Seconds, minutes and hours are not units;
/// 4. `today`, `yesterday`, `aujourd'hui`, `hier`, `avant-hier`: the day;
/// 5. `last|this week|month|year`, `la semaine dernière`, `le mois dernier`,
///    `l'an dernier`, `cette semaine`, `ce mois-ci`, `cette année`: the
///    civil period. Not when the words describe something else: "the last
///    week of the sprint", "this year's kernel", "cette année-là";
/// 6. `in YYYY` / `en YYYY`, only when the number ends the clause: "in 2048
///    chunks" and "in 2000 ms" are quantities.
///
/// A bare year after `now` is never returned: "in 2048" asked in 2026 is a
/// number, not a date. A date that names its day or month AND its year is
/// taken at its word even after `now`: the caller's `now` is when the
/// question is asked, not a bound on what the store holds (imported
/// history, or a caller anchoring `now` earlier than its newest memory, as
/// a benchmark harness does), and a window no memory falls into changes
/// nothing.
///
/// Returns `None` when nothing is recognised, which is the common case: the
/// cost is then one lowercase copy of the query and a few linear scans.
pub fn parse_query_window_at(
    query: &str,
    now: DateTime<Utc>,
    offset: FixedOffset,
) -> Option<TimeWindow> {
    let cal = Calendar {
        today: now.with_timezone(&offset).date_naive(),
        offset,
    };
    if let Some(window) = iso_date(query).and_then(|day| cal.day(day)) {
        return Some(window);
    }
    let lower = query.to_lowercase();
    let words = Words::split(&lower);
    if words.text.is_empty() {
        return None;
    }
    named_month(&words, &cal)
        .or_else(|| units_ago(&words.text, &cal))
        .or_else(|| day_word(&words.text, &cal))
        .or_else(|| civil_period(&words, &cal))
        .or_else(|| in_year(&words, &cal))
}

// --- Calendar: window builders in the caller's offset ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    Day,
    Week,
    Month,
    Year,
}

/// The asker's calendar: their current date, and the offset their civil
/// days are cut in.
struct Calendar {
    today: NaiveDate,
    offset: FixedOffset,
}

impl Calendar {
    /// `[first 00:00, end 00:00)` in the caller's offset, as UTC instants.
    fn span(&self, first: NaiveDate, end: NaiveDate) -> Option<TimeWindow> {
        let midnight = |day: NaiveDate| {
            let naive = day.and_hms_opt(0, 0, 0)?;
            let local = self.offset.from_local_datetime(&naive).single()?;
            Some(local.with_timezone(&Utc))
        };
        Some(TimeWindow {
            start: midnight(first)?,
            end: midnight(end)?,
        })
    }

    /// A day named with its year (or reached from today): never refused
    /// for being after today.
    fn day(&self, day: NaiveDate) -> Option<TimeWindow> {
        self.span(day, day.succ_opt()?)
    }

    /// A month named with its year: never refused for being after today.
    fn month(&self, year: i32, month: u32) -> Option<TimeWindow> {
        let first = NaiveDate::from_ymd_opt(year, month, 1)?;
        self.span(first, first.checked_add_months(Months::new(1))?)
    }

    /// A bare year. `None` when it has not started yet: on its own, a
    /// number after today's year is far more often a quantity than a date.
    fn year(&self, year: i32) -> Option<TimeWindow> {
        let first = NaiveDate::from_ymd_opt(year, 1, 1)?;
        if first > self.today {
            return None;
        }
        self.span(first, NaiveDate::from_ymd_opt(year.checked_add(1)?, 1, 1)?)
    }

    /// The civil period containing today, or the one just before it.
    fn period(&self, unit: Unit, previous: bool) -> Option<TimeWindow> {
        let today = self.today;
        match unit {
            Unit::Day => self.day(if previous { today.pred_opt()? } else { today }),
            Unit::Week => {
                let anchor = if previous {
                    today.checked_sub_days(Days::new(7))?
                } else {
                    today
                };
                let since_monday = u64::from(anchor.weekday().num_days_from_monday());
                let monday = anchor.checked_sub_days(Days::new(since_monday))?;
                self.span(monday, monday.checked_add_days(Days::new(7))?)
            }
            Unit::Month => {
                let first = today.with_day(1)?;
                let first = if previous {
                    first.checked_sub_months(Months::new(1))?
                } else {
                    first
                };
                self.span(first, first.checked_add_months(Months::new(1))?)
            }
            Unit::Year => self.year(if previous {
                today.year().checked_sub(1)?
            } else {
                today.year()
            }),
        }
    }

    /// "N units ago": a window centred on `today - N units`. The tolerance
    /// grows with the unit, because "two months ago" is a vaguer claim than
    /// "two days ago". Month arithmetic clamps to the end of the month (31
    /// March minus one month is the last day of February).
    fn ago(&self, n: u32, unit: Unit) -> Option<TimeWindow> {
        let today = self.today;
        let (first, last) = match unit {
            Unit::Day => {
                let centre = today.checked_sub_days(Days::new(u64::from(n)))?;
                (centre.pred_opt()?, centre.succ_opt()?)
            }
            Unit::Week => {
                let centre = today.checked_sub_days(Days::new(u64::from(n) * 7))?;
                (
                    centre.checked_sub_days(Days::new(4))?,
                    centre.checked_add_days(Days::new(4))?,
                )
            }
            Unit::Month => {
                let centre = today.checked_sub_months(Months::new(n))?;
                (
                    centre.checked_sub_days(Days::new(16))?,
                    centre.checked_add_days(Days::new(16))?,
                )
            }
            Unit::Year => {
                let centre = today.checked_sub_months(Months::new(n.checked_mul(12)?))?;
                (
                    centre.checked_sub_months(Months::new(6))?,
                    centre.checked_add_months(Months::new(6))?,
                )
            }
        };
        self.span(first, last.succ_opt()?)
    }
}

// --- Words ---

/// Words that can follow a date and start the rest of the sentence: a
/// subject, a determiner, a question word, a conjunction. After "in 2023"
/// or "in March" they confirm a date ("in 2023 we moved to Turso"); any
/// other word leaves the number or the month name possibly qualifying it
/// ("in 2048 chunks", "in may be"), and the rule gives up.
const CLAUSE_WORDS: &[&str] = &[
    "we", "i", "you", "they", "he", "she", "it", "the", "our", "my", "when", "what", "which",
    "who", "how", "why", "where", "because", "so", "then", "that", "there", "but", "with", "for",
    "about", "on", "nous", "je", "j", "tu", "il", "elle", "ils", "elles", "le", "la", "les",
    "notre", "nos", "mon", "ma", "quand", "car", "donc", "que", "qu", "qui", "c", "ce", "mais",
    "avec", "pour",
];

/// The lowercased query cut into words, each with the separator that
/// follows it: the separator is what tells a clause end ("in 2023, ...")
/// or a possessive ("this year's") from plain spacing.
struct Words<'a> {
    text: Vec<&'a str>,
    gap: Vec<&'a str>,
}

impl<'a> Words<'a> {
    fn split(lower: &'a str) -> Self {
        let mut text = Vec::new();
        let mut gap = Vec::new();
        let mut word_start = None;
        let mut gap_start = 0;
        for (i, c) in lower.char_indices() {
            if c.is_alphanumeric() {
                if word_start.is_none() {
                    if !text.is_empty() {
                        gap.push(&lower[gap_start..i]);
                    }
                    word_start = Some(i);
                }
            } else if let Some(start) = word_start.take() {
                text.push(&lower[start..i]);
                gap_start = i;
            }
        }
        match word_start {
            Some(start) => {
                text.push(&lower[start..]);
                gap.push("");
            }
            None if !text.is_empty() => gap.push(&lower[gap_start..]),
            None => {}
        }
        Words { text, gap }
    }

    /// Word `i` carries a possessive: "year's", "week’s".
    fn possessive(&self, i: usize) -> bool {
        matches!(self.gap.get(i).copied(), Some("'" | "’"))
            && self.text.get(i + 1).copied() == Some("s")
    }

    /// Word `i` is the last of its clause: last of the query, followed by
    /// punctuation then a space ("in 2023, we" but not "2000.5" nor
    /// "2023-2024"), or followed by one of [`CLAUSE_WORDS`].
    fn ends_clause(&self, i: usize) -> bool {
        let Some(gap) = self.gap.get(i).copied() else {
            return false;
        };
        let Some(next) = self.text.get(i + 1).copied() else {
            return true;
        };
        if self.possessive(i) {
            return false;
        }
        let breaks = gap.contains([',', ';', ':', '?', '!', '.', ')', '—', '–'])
            && gap.contains(char::is_whitespace);
        breaks || CLAUSE_WORDS.contains(&next)
    }
}

fn month_en(tok: &str) -> Option<u32> {
    Some(match tok {
        "january" => 1,
        "february" => 2,
        "march" => 3,
        "april" => 4,
        "may" => 5,
        "june" => 6,
        "july" => 7,
        "august" => 8,
        "september" => 9,
        "october" => 10,
        "november" => 11,
        "december" => 12,
        _ => return None,
    })
}

fn month_fr(tok: &str) -> Option<u32> {
    Some(match tok {
        "janvier" => 1,
        "février" | "fevrier" => 2,
        "mars" => 3,
        "avril" => 4,
        "mai" => 5,
        "juin" => 6,
        "juillet" => 7,
        "août" | "aout" => 8,
        "septembre" => 9,
        "octobre" => 10,
        "novembre" => 11,
        "décembre" | "decembre" => 12,
        _ => return None,
    })
}

/// Month names that are everyday words first: a modal verb, a walk, a
/// planet.
fn ambiguous_month(tok: &str) -> bool {
    matches!(tok, "may" | "march" | "mars" | "mai")
}

fn unit_en(tok: &str) -> Option<Unit> {
    Some(match tok {
        "day" | "days" => Unit::Day,
        "week" | "weeks" => Unit::Week,
        "month" | "months" => Unit::Month,
        "year" | "years" => Unit::Year,
        _ => return None,
    })
}

fn unit_fr(tok: &str) -> Option<Unit> {
    Some(match tok {
        "jour" | "jours" => Unit::Day,
        "semaine" | "semaines" => Unit::Week,
        "mois" => Unit::Month,
        "an" | "ans" | "année" | "années" | "annee" | "annees" => Unit::Year,
        _ => return None,
    })
}

/// A count written in digits (at most four, so the arithmetic cannot run
/// away).
fn digits(tok: &str) -> Option<u32> {
    if tok.len() <= 4 && tok.bytes().all(|b| b.is_ascii_digit()) {
        tok.parse().ok()
    } else {
        None
    }
}

fn number_en(tok: &str) -> Option<u32> {
    if let Some(n) = digits(tok) {
        return Some(n);
    }
    Some(match tok {
        "a" | "an" | "one" => 1,
        "two" => 2,
        "three" => 3,
        "four" => 4,
        "five" => 5,
        "six" => 6,
        "seven" => 7,
        "eight" => 8,
        "nine" => 9,
        "ten" => 10,
        "eleven" => 11,
        "twelve" => 12,
        _ => return None,
    })
}

fn number_fr(tok: &str) -> Option<u32> {
    if let Some(n) = digits(tok) {
        return Some(n);
    }
    Some(match tok {
        "un" | "une" => 1,
        "deux" => 2,
        "trois" => 3,
        "quatre" => 4,
        "cinq" => 5,
        "six" => 6,
        "sept" => 7,
        "huit" => 8,
        "neuf" => 9,
        "dix" => 10,
        "onze" => 11,
        "douze" => 12,
        _ => return None,
    })
}

/// A four-digit year in the accepted range.
fn year(tok: &str) -> Option<i32> {
    if tok.len() != 4 {
        return None;
    }
    let y = i32::try_from(digits(tok)?).ok()?;
    (MIN_YEAR..=MAX_YEAR).contains(&y).then_some(y)
}

/// A day of the month: `7`, `07`, `7th`, `1st`, `1er`.
fn day_of_month(tok: &str) -> Option<u32> {
    let number = tok.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    let suffix = &tok[number.len()..];
    if number.is_empty()
        || number.len() > 2
        || !matches!(suffix, "" | "st" | "nd" | "rd" | "th" | "er")
    {
        return None;
    }
    digits(number).filter(|d| (1..=31).contains(d))
}

// --- Rules ---

/// First `YYYY-MM-DD` in the raw query that is a real calendar date and is
/// not glued to other digits.
fn iso_date(query: &str) -> Option<NaiveDate> {
    let bytes = query.as_bytes();
    for (i, w) in bytes.windows(10).enumerate() {
        let shaped = w[4] == b'-'
            && w[7] == b'-'
            && w.iter()
                .enumerate()
                .all(|(j, b)| j == 4 || j == 7 || b.is_ascii_digit());
        if !shaped {
            continue;
        }
        let glued_before = i > 0 && bytes[i - 1].is_ascii_digit();
        let glued_after = bytes.get(i + 10).is_some_and(|b| b.is_ascii_digit());
        if glued_before || glued_after {
            continue;
        }
        // Ten ASCII bytes: always valid UTF-8.
        let date = std::str::from_utf8(w)
            .ok()
            .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
        if date.is_some() {
            return date;
        }
    }
    None
}

fn named_month(words: &Words<'_>, cal: &Calendar) -> Option<TimeWindow> {
    let toks = &words.text;
    for (i, tok) in toks.iter().enumerate() {
        let (month, french) = match (month_en(tok), month_fr(tok)) {
            (Some(m), _) => (m, false),
            (None, Some(m)) => (m, true),
            (None, None) => continue,
        };
        let before = |back: usize| i.checked_sub(back).and_then(|p| toks.get(p)).copied();
        let next = toks.get(i + 1).copied();
        // A preposition of the month's own language: "in march" yes, "life
        // in mars" no.
        let introduced = if french {
            before(1) == Some("en") || (before(1) == Some("de") && before(2) == Some("mois"))
        } else {
            before(1) == Some("in")
        };
        let day_on = |y: i32, d: u32| NaiveDate::from_ymd_opt(y, month, d).and_then(|x| cal.day(x));

        // "May 7, 2023": a day number and a year leave no doubt.
        let day_then_year = next
            .and_then(day_of_month)
            .zip(toks.get(i + 2).copied().and_then(year));
        if let Some(w) = day_then_year.and_then(|(d, y)| day_on(y, d)) {
            return Some(w);
        }
        if let Some(y) = next.and_then(year) {
            // "7 May 2023", "le 1er mars 2023".
            if let Some(w) = before(1).and_then(day_of_month).and_then(|d| day_on(y, d)) {
                return Some(w);
            }
            // The whole month. "Mars 2020 rover" and "what may 2024
            // customers expect" are why an everyday word needs its
            // preposition even with a year.
            if (introduced || !ambiguous_month(tok))
                && let Some(w) = cal.month(y, month)
            {
                return Some(w);
            }
            continue;
        }
        // Without a year: the most recent occurrence that has started.
        if introduced && (!ambiguous_month(tok) || words.ends_clause(i)) {
            let y = if month <= cal.today.month() {
                cal.today.year()
            } else {
                cal.today.year().checked_sub(1)?
            };
            if let Some(w) = cal.month(y, month) {
                return Some(w);
            }
        }
    }
    None
}

fn units_ago(toks: &[&str], cal: &Calendar) -> Option<TimeWindow> {
    // English: "<N> <unit> ago".
    for w in toks.windows(3) {
        if w[2] == "ago"
            && let Some((n, unit)) = number_en(w[0]).zip(unit_en(w[1]))
            && let Some(window) = cal.ago(n, unit)
        {
            return Some(window);
        }
    }
    // French: "il y a <N> <unité>".
    for w in toks.windows(5) {
        if w[0] == "il"
            && w[1] == "y"
            && w[2] == "a"
            && let Some((n, unit)) = number_fr(w[3]).zip(unit_fr(w[4]))
            && let Some(window) = cal.ago(n, unit)
        {
            return Some(window);
        }
    }
    None
}

fn day_word(toks: &[&str], cal: &Calendar) -> Option<TimeWindow> {
    for (i, tok) in toks.iter().enumerate() {
        let prev = i.checked_sub(1).and_then(|p| toks.get(p)).copied();
        let days_back = match *tok {
            "today" | "aujourdhui" => 0,
            // "aujourd'hui" splits on the apostrophe.
            "hui" if prev == Some("aujourd") => 0,
            "yesterday" if prev == Some("before") => 2,
            "hier" if prev == Some("avant") => 2,
            "yesterday" | "hier" => 1,
            _ => continue,
        };
        return cal
            .today
            .checked_sub_days(Days::new(days_back))
            .and_then(|day| cal.day(day));
    }
    None
}

fn civil_period(words: &Words<'_>, cal: &Calendar) -> Option<TimeWindow> {
    let toks = &words.text;
    for (i, w) in toks.windows(2).enumerate() {
        let found = match (w[0], w[1]) {
            ("last", "week") => Some((Unit::Week, true)),
            ("last", "month") => Some((Unit::Month, true)),
            ("last", "year") => Some((Unit::Year, true)),
            ("this", "week") => Some((Unit::Week, false)),
            ("this", "month") => Some((Unit::Month, false)),
            ("this", "year") => Some((Unit::Year, false)),
            ("cette", "semaine") => Some((Unit::Week, false)),
            ("ce", "mois") => Some((Unit::Month, false)),
            ("cette", "année" | "annee") => Some((Unit::Year, false)),
            // "la semaine dernière", "le mois dernier", "l'an passé".
            (
                period @ ("semaine" | "mois" | "an" | "année" | "annee"),
                "dernier" | "dernière" | "derniere" | "passé" | "passée",
            ) => unit_fr(period).map(|unit| (unit, true)),
            _ => None,
        };
        let Some((unit, previous)) = found else {
            continue;
        };
        // The same two words also describe a part of something else ("the
        // last week of the sprint", "la semaine dernière du mois"), a
        // rolling span ("over the last year"), an owner ("this year's
        // kernel") or another year altogether ("cette année-là").
        let before = i.checked_sub(1).and_then(|p| toks.get(p)).copied();
        let after = toks.get(i + 2).copied();
        let part_of = matches!(after, Some("of" | "de" | "du" | "des" | "d" | "là"));
        let rolling = w[0] == "last" && before == Some("the");
        if part_of || rolling || words.possessive(i + 1) {
            continue;
        }
        if let Some(window) = cal.period(unit, previous) {
            return Some(window);
        }
    }
    None
}

fn in_year(words: &Words<'_>, cal: &Calendar) -> Option<TimeWindow> {
    for (i, w) in words.text.windows(2).enumerate() {
        if !matches!(w[0], "in" | "en") {
            continue;
        }
        // "in 2048 chunks", "en 2048 octets", "in 2000 ms": a number that
        // does not end its clause counts something.
        let window = year(w[1])
            .filter(|_| words.ends_clause(i + 1))
            .and_then(|y| cal.year(y));
        if window.is_some() {
            return window;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    fn day(y: i32, m: u32, d: u32) -> DateTime<Utc> {
        at(y, m, d, 0, 0)
    }

    fn win(start: (i32, u32, u32), end: (i32, u32, u32)) -> Option<TimeWindow> {
        Some(TimeWindow {
            start: day(start.0, start.1, start.2),
            end: day(end.0, end.1, end.2),
        })
    }

    /// The benchmark dates its questions in 2023: Tuesday 30 May, late
    /// evening. Every expectation below is relative to this anchor, never
    /// to the wall clock.
    fn anchor() -> DateTime<Utc> {
        at(2023, 5, 30, 23, 40)
    }

    fn parse(query: &str) -> Option<TimeWindow> {
        parse_query_window(query, anchor())
    }

    /// The anchor the review ran its reproductions from: Sunday 4 October
    /// 2026, noon UTC.
    fn review_anchor() -> DateTime<Utc> {
        at(2026, 10, 4, 12, 0)
    }

    fn utc_range(start: DateTime<Utc>, end: DateTime<Utc>) -> Option<TimeWindow> {
        Some(TimeWindow { start, end })
    }

    fn east(hours: i32) -> FixedOffset {
        FixedOffset::east_opt(hours * 3600).unwrap()
    }

    // --- TimeWindow ---

    #[test]
    fn contains_includes_start_and_excludes_end() {
        let w = TimeWindow {
            start: day(2023, 5, 30),
            end: day(2023, 5, 31),
        };
        assert!(w.contains(day(2023, 5, 30)));
        assert!(w.contains(at(2023, 5, 30, 23, 59)));
        assert!(!w.contains(day(2023, 5, 31)));
        assert!(!w.contains(at(2023, 5, 29, 23, 59)));
        assert!(!w.contains(day(2024, 1, 1)));
    }

    // --- parse_instant ---

    #[test]
    fn instant_rfc3339_is_converted_to_utc() {
        assert_eq!(
            parse_instant("2023-05-30T23:40:00Z").unwrap(),
            at(2023, 5, 30, 23, 40)
        );
        assert_eq!(
            parse_instant("2023-05-30T23:40:00+02:00").unwrap(),
            at(2023, 5, 30, 21, 40)
        );
    }

    #[test]
    fn instant_without_offset_is_utc() {
        assert_eq!(
            parse_instant("2023-05-30T23:40:00").unwrap(),
            at(2023, 5, 30, 23, 40)
        );
        assert_eq!(
            parse_instant(" 2023-05-30 23:40:00.250 ").unwrap(),
            at(2023, 5, 30, 23, 40) + chrono::Duration::milliseconds(250)
        );
    }

    #[test]
    fn instant_date_only_is_midnight_utc() {
        assert_eq!(parse_instant("2023-05-30").unwrap(), day(2023, 5, 30));
        assert_eq!(parse_instant("2024-02-29").unwrap(), day(2024, 2, 29));
    }

    #[test]
    fn instant_rejects_everything_else() {
        for bad in [
            "",
            "yesterday",
            "2023/05/30",
            "2023-13-01",
            "2023-02-29",
            "2023-05-30T25:00:00",
            "2023-05",
            "30-05-2023",
            "1685490000",
        ] {
            let err = parse_instant(bad).unwrap_err();
            assert!(
                matches!(err, IcmError::InvalidInput(_)),
                "{bad:?} gave {err:?}"
            );
        }
    }

    /// chrono parses and prints years beyond four digits with a sign
    /// (`+12345-06-01T...`), which no RFC 3339 reader takes back: such an
    /// instant would be stored, then read as "now" on every read.
    #[test]
    fn instant_rejects_years_that_cannot_be_read_back() {
        for bad in [
            "+12345-06-01",
            "+99999-01-01T00:00:00",
            "0000-01-01",
            "0000-06-01T00:00:00Z",
            "-0001-01-01",
            // In range as written, out of range once in UTC.
            "9999-12-31T23:59:59-01:00",
            "0001-01-01T00:00:00+01:00",
        ] {
            match parse_instant(bad) {
                Err(IcmError::InvalidInput(msg)) => {
                    assert!(msg.contains("out of range"), "{bad:?}: {msg}");
                }
                other => panic!("{bad:?} gave {other:?}"),
            }
        }
        // With an offset the five-digit form is not RFC 3339 at all: it is
        // refused as a format error, which is as good.
        assert!(matches!(
            parse_instant("+10000-01-01T00:00:00Z"),
            Err(IcmError::InvalidInput(_))
        ));
    }

    #[test]
    fn instant_accepts_the_whole_readable_range_and_it_round_trips() {
        for ok in [
            "0001-01-01",
            "0001-01-01T00:00:00Z",
            "9999-12-31",
            "9999-12-31T23:59:59Z",
            "9999-12-31T23:59:59+01:00",
            "2023-05-30T23:40:00+02:00",
        ] {
            let t = parse_instant(ok).unwrap();
            // What the store writes is what it must be able to read.
            let reread = DateTime::parse_from_rfc3339(&t.to_rfc3339()).unwrap();
            assert_eq!(reread.with_timezone(&Utc), t, "{ok:?}");
        }
    }

    // --- parse_query_window: no expression ---

    #[test]
    fn plain_queries_have_no_window() {
        for q in [
            "",
            "   ",
            "what database do we use",
            "quelle base de données utilise-t-on",
            "sqlite-vec refuses k > 4096",
            "port 2023 is closed",
            "issue 2021",
        ] {
            assert_eq!(parse(q), None, "{q:?}");
        }
    }

    #[test]
    fn bare_month_names_do_not_trigger() {
        for q in [
            "may",
            "march",
            "mars",
            "mai",
            "I may go to the store",
            "the march of progress",
            "la planète mars",
            "life in mars",
            "en march avant",
            "may 7 people join",
            "august company",
        ] {
            assert_eq!(parse(q), None, "{q:?}");
        }
    }

    #[test]
    fn second_minute_and_hour_are_not_units() {
        for q in [
            "a second ago",
            "5 seconds ago",
            "2 hours ago",
            "ten minutes ago",
            "the second week of the trip",
            "il y a deux secondes",
            "il y a 3 heures",
        ] {
            assert_eq!(parse(q), None, "{q:?}");
        }
    }

    #[test]
    fn matching_is_on_whole_words() {
        for q in [
            "yesterdays",
            "todayish",
            "weeks agone",
            "thierry",
            "blast week",
            "in 20233",
            "in 202",
            "skin 2022",
        ] {
            assert_eq!(parse(q), None, "{q:?}");
        }
    }

    // --- parse_query_window: days ---

    #[test]
    fn today_in_both_languages() {
        let expected = win((2023, 5, 30), (2023, 5, 31));
        assert_eq!(parse("what did I decide today"), expected);
        assert_eq!(parse("qu'ai-je décidé aujourd'hui ?"), expected);
        assert_eq!(parse("décisions d’aujourd’hui"), expected);
    }

    #[test]
    fn yesterday_in_both_languages() {
        let expected = win((2023, 5, 29), (2023, 5, 30));
        assert_eq!(parse("What did we fix yesterday?"), expected);
        assert_eq!(parse("qu'a-t-on corrigé hier"), expected);
    }

    #[test]
    fn day_before_yesterday() {
        let expected = win((2023, 5, 28), (2023, 5, 29));
        assert_eq!(parse("the day before yesterday"), expected);
        assert_eq!(parse("avant-hier"), expected);
    }

    #[test]
    fn matching_ignores_case() {
        assert_eq!(parse("YESTERDAY"), win((2023, 5, 29), (2023, 5, 30)));
        assert_eq!(parse("Last Week"), win((2023, 5, 22), (2023, 5, 29)));
        assert_eq!(
            parse("Il Y A Trois Jours"),
            win((2023, 5, 26), (2023, 5, 29))
        );
        assert_eq!(parse("IN MARCH 2021"), win((2021, 3, 1), (2021, 4, 1)));
        assert_eq!(parse("EN DÉCEMBRE"), win((2022, 12, 1), (2023, 1, 1)));
    }

    // --- parse_query_window: "N units ago" ---

    #[test]
    fn days_ago_has_one_day_of_tolerance() {
        // Centre 27 May, one day each side.
        let expected = win((2023, 5, 26), (2023, 5, 29));
        assert_eq!(parse("what broke 3 days ago"), expected);
        assert_eq!(parse("what broke three days ago"), expected);
        assert_eq!(parse("qu'est-ce qui a cassé il y a 3 jours"), expected);
        assert_eq!(parse("il y a trois jours"), expected);
    }

    #[test]
    fn weeks_ago_has_four_days_of_tolerance() {
        // Centre 16 May.
        let expected = win((2023, 5, 12), (2023, 5, 21));
        assert_eq!(parse("two weeks ago"), expected);
        assert_eq!(parse("il y a deux semaines"), expected);
        // "a week ago": centre 23 May.
        assert_eq!(parse("a week ago"), win((2023, 5, 19), (2023, 5, 28)));
        assert_eq!(
            parse("il y a une semaine"),
            win((2023, 5, 19), (2023, 5, 28))
        );
    }

    #[test]
    fn months_ago_has_sixteen_days_of_tolerance() {
        // Centre 30 April.
        let expected = win((2023, 4, 14), (2023, 5, 17));
        assert_eq!(parse("what did she say a month ago"), expected);
        assert_eq!(parse("il y a un mois"), expected);
    }

    #[test]
    fn years_ago_has_six_months_of_tolerance() {
        // Centre 30 May 2021.
        let expected = win((2020, 11, 30), (2021, 12, 1));
        assert_eq!(parse("2 years ago"), expected);
        assert_eq!(parse("il y a deux ans"), expected);
        assert_eq!(parse("il y a 2 années"), expected);
    }

    #[test]
    fn spelled_numbers_stop_at_twelve() {
        assert_eq!(parse("twelve days ago"), win((2023, 5, 17), (2023, 5, 20)));
        assert_eq!(
            parse("il y a douze jours"),
            win((2023, 5, 17), (2023, 5, 20))
        );
        assert_eq!(parse("thirteen days ago"), None);
        assert_eq!(parse("il y a treize jours"), None);
        // Five digits is not a count we resolve.
        assert_eq!(parse("99999 days ago"), None);
    }

    // --- parse_query_window: civil periods ---

    #[test]
    fn last_and_this_week_are_iso_weeks() {
        // 30 May 2023 is a Tuesday: this week starts Monday 29 May.
        assert_eq!(parse("last week"), win((2023, 5, 22), (2023, 5, 29)));
        assert_eq!(
            parse("la semaine dernière"),
            win((2023, 5, 22), (2023, 5, 29))
        );
        assert_eq!(parse("this week"), win((2023, 5, 29), (2023, 6, 5)));
        assert_eq!(parse("cette semaine"), win((2023, 5, 29), (2023, 6, 5)));
    }

    #[test]
    fn last_and_this_month() {
        assert_eq!(parse("last month"), win((2023, 4, 1), (2023, 5, 1)));
        assert_eq!(parse("le mois dernier"), win((2023, 4, 1), (2023, 5, 1)));
        assert_eq!(parse("this month"), win((2023, 5, 1), (2023, 6, 1)));
        assert_eq!(parse("ce mois-ci"), win((2023, 5, 1), (2023, 6, 1)));
    }

    #[test]
    fn last_and_this_year() {
        let last = win((2022, 1, 1), (2023, 1, 1));
        assert_eq!(parse("what did we ship last year"), last);
        assert_eq!(parse("l'an dernier"), last);
        assert_eq!(parse("l'année dernière"), last);
        assert_eq!(parse("l’annee derniere"), last);
        let this = win((2023, 1, 1), (2024, 1, 1));
        assert_eq!(parse("this year"), this);
        assert_eq!(parse("cette année"), this);
    }

    // --- parse_query_window: named months, years, ISO dates ---

    #[test]
    fn month_without_year_is_the_most_recent_started_one() {
        assert_eq!(parse("in March"), win((2023, 3, 1), (2023, 4, 1)));
        assert_eq!(parse("en mars"), win((2023, 3, 1), (2023, 4, 1)));
        assert_eq!(parse("au mois de mars"), win((2023, 3, 1), (2023, 4, 1)));
        // The current month has started: it is not "the future".
        assert_eq!(parse("in May"), win((2023, 5, 1), (2023, 6, 1)));
        assert_eq!(parse("en mai"), win((2023, 5, 1), (2023, 6, 1)));
        // June has not: last year's.
        assert_eq!(parse("in June"), win((2022, 6, 1), (2022, 7, 1)));
        assert_eq!(parse("in December"), win((2022, 12, 1), (2023, 1, 1)));
        assert_eq!(parse("en août"), win((2022, 8, 1), (2022, 9, 1)));
    }

    #[test]
    fn month_with_year() {
        let expected = win((2021, 3, 1), (2021, 4, 1));
        assert_eq!(parse("what happened in March 2021"), expected);
        assert_eq!(parse("in March 2021 outage review"), expected);
        assert_eq!(parse("en mars 2021"), expected);
        assert_eq!(parse("au mois de mars 2021"), expected);
        // A month name that is nothing else needs no preposition.
        assert_eq!(parse("june 2021 outage"), win((2021, 6, 1), (2021, 7, 1)));
        assert_eq!(parse("décembre 2022"), win((2022, 12, 1), (2023, 1, 1)));
        // A year out of range leaves a bare month name: nothing.
        assert_eq!(parse("in march 1800"), None);
    }

    /// "may", "march", "mars" and "mai" are words before they are months:
    /// with a year but without their preposition or a day number, nothing.
    #[test]
    fn everyday_month_words_need_a_preposition_even_with_a_year() {
        for q in [
            "Mars 2020 rover notes",
            "notes on Mars 2020",
            "what may 2024 customers expect",
            "March 2021 outage",
            "mars 2021",
            "mai 2022",
            "may 2021",
            "life in mars 2020",
            "en march 2021",
        ] {
            assert_eq!(parse(q), None, "{q:?}");
            assert_eq!(parse_query_window(q, review_anchor()), None, "{q:?}");
        }
    }

    /// Without a year, an everyday month word must also end its clause.
    #[test]
    fn everyday_month_words_without_a_year_must_end_the_clause() {
        for q in [
            "bug in may be fixed",
            "in may be",
            "in march madness",
            "the protesters were in march formation",
            "en mai fais ce qu'il te plaît",
            "en mars attaque",
        ] {
            assert_eq!(parse(q), None, "{q:?}");
            assert_eq!(parse_query_window(q, review_anchor()), None, "{q:?}");
        }
        // The date reading survives when the clause ends there.
        let march = win((2023, 3, 1), (2023, 4, 1));
        assert_eq!(parse("what did we ship in March?"), march);
        assert_eq!(parse("in March, the deploy broke"), march);
        assert_eq!(parse("in March we moved to Turso"), march);
        assert_eq!(parse("en mars nous avons livré"), march);
        assert_eq!(parse("what broke in May."), win((2023, 5, 1), (2023, 6, 1)));
        // An unambiguous name is not held to that.
        assert_eq!(
            parse("in february deploys were manual"),
            win((2023, 2, 1), (2023, 3, 1))
        );
    }

    #[test]
    fn month_with_day_and_year_is_the_day() {
        let expected = win((2023, 5, 7), (2023, 5, 8));
        assert_eq!(parse("What did Caroline do on 7 May 2023?"), expected);
        assert_eq!(parse("on May 7, 2023"), expected);
        assert_eq!(parse("on May 7th, 2023"), expected);
        assert_eq!(parse("le 7 mai 2023"), expected);
        assert_eq!(parse("le 1er mars 2023"), win((2023, 3, 1), (2023, 3, 2)));
        // 31 June does not exist: fall back to the month.
        assert_eq!(parse("31 June 2022"), win((2022, 6, 1), (2022, 7, 1)));
    }

    #[test]
    fn year_alone_needs_a_preposition_and_a_sane_range() {
        assert_eq!(parse("in 2022"), win((2022, 1, 1), (2023, 1, 1)));
        assert_eq!(parse("en 2022"), win((2022, 1, 1), (2023, 1, 1)));
        assert_eq!(parse("in 1990"), win((1990, 1, 1), (1991, 1, 1)));
        assert_eq!(parse("in 2023"), win((2023, 1, 1), (2024, 1, 1)));
        assert_eq!(parse("in 1989"), None);
        assert_eq!(parse("in 2101"), None);
        assert_eq!(parse("2022"), None);
    }

    /// "in 2048 chunks": a number that does not end its clause is a
    /// quantity, whatever the noun.
    #[test]
    fn a_year_followed_by_what_it_counts_is_a_quantity() {
        for q in [
            "split the file in 2048 chunks",
            "fichier en 2048 octets",
            "timeout in 2000 ms",
            "retry in 2000 milliseconds",
            "summarize in 2000 words",
            "résumé en 2000 mots",
            "done in 2021 steps",
            "in 2022 requests per second",
            "in 2000.5 ms",
            "in 2022-2023",
            "in 2022's",
        ] {
            assert_eq!(parse(q), None, "{q:?}");
            assert_eq!(parse_query_window(q, review_anchor()), None, "{q:?}");
        }
    }

    /// The year reading survives when the number ends the query, is
    /// followed by punctuation, or by a word that starts the rest of the
    /// sentence.
    #[test]
    fn a_year_that_ends_its_clause_is_a_year() {
        let expected = win((2022, 1, 1), (2023, 1, 1));
        for q in [
            "in 2022",
            "what happened in 2022?",
            "in 2022, we moved to Turso",
            "in 2022 we moved to Turso",
            "(in 2022) the schema changed",
            "what did we decide in 2022 about the schema",
            "decisions in 2022 for the store",
            "en 2022",
            "en 2022 on a migré",
            "en 2022, la base a changé",
            "qu'a-t-on décidé en 2022 pour le schéma",
        ] {
            assert_eq!(parse(q), expected, "{q:?}");
        }
    }

    /// A bare year after the anchor is a number, not a date. A date that
    /// names its day or month and its year is taken at its word: the
    /// anchor is when the question is asked, not a bound on the store.
    #[test]
    fn only_a_bare_year_is_refused_for_being_after_the_anchor() {
        for q in ["in 2024", "in 2100", "en 2048"] {
            assert_eq!(parse(q), None, "{q:?}");
        }
        assert_eq!(parse("in 2023"), win((2023, 1, 1), (2024, 1, 1)));
        // The same year from a later anchor is a date.
        assert_eq!(
            parse_query_window("in 2024", review_anchor()),
            win((2024, 1, 1), (2025, 1, 1))
        );

        // Named with their year: windows, before or after the anchor.
        assert_eq!(parse("30 May 2023"), win((2023, 5, 30), (2023, 5, 31)));
        assert_eq!(parse("31 May 2023"), win((2023, 5, 31), (2023, 6, 1)));
        assert_eq!(parse("June 1, 2023"), win((2023, 6, 1), (2023, 6, 2)));
        assert_eq!(parse("in May 2023"), win((2023, 5, 1), (2023, 6, 1)));
        assert_eq!(parse("in June 2023"), win((2023, 6, 1), (2023, 7, 1)));
        assert_eq!(parse("en juin 2023"), win((2023, 6, 1), (2023, 7, 1)));
        assert_eq!(parse("december 2024"), win((2024, 12, 1), (2025, 1, 1)));
        assert_eq!(parse("2023-05-30"), win((2023, 5, 30), (2023, 5, 31)));
        assert_eq!(parse("2023-05-31"), win((2023, 5, 31), (2023, 6, 1)));
        assert_eq!(parse("2030-01-01"), win((2030, 1, 1), (2030, 1, 2)));
    }

    /// The shape of a benchmark question: the harness anchors `now` on one
    /// session of the conversation, and asks about events dated after it.
    /// Refusing those windows cost 1.5 points of recall@5 on LoCoMo.
    #[test]
    fn a_dated_question_asked_from_an_earlier_anchor_keeps_its_window() {
        let asked = at(2023, 7, 15, 12, 0);
        assert_eq!(
            parse_query_window("What setback did Melanie face in October 2023?", asked),
            win((2023, 10, 1), (2023, 11, 1))
        );
        assert_eq!(
            parse_query_window(
                "What painting did Melanie show to Caroline on October 13, 2023?",
                asked
            ),
            win((2023, 10, 13), (2023, 10, 14))
        );
    }

    /// "last week" and "this year" also describe a part of something, a
    /// rolling span, or an owner.
    #[test]
    fn period_words_that_mean_something_else() {
        for q in [
            "the last week of the sprint planning",
            "last week of the sprint",
            "the last month of the quarter",
            "this week of onboarding",
            "over the last year",
            "in the last week",
            "this year's kernel",
            "release notes of this year's kernel",
            "last year's roadmap",
            "this week’s standup",
            "la semaine dernière du sprint",
            "le mois dernier de l'année",
            "cette année-là",
            "ce mois-là",
        ] {
            assert_eq!(parse(q), None, "{q:?}");
            assert_eq!(parse_query_window(q, review_anchor()), None, "{q:?}");
        }
    }

    /// The cases the review asked to keep, from its own anchor.
    #[test]
    fn legitimate_expressions_survive_the_tightening() {
        let now = review_anchor(); // Sunday 4 October 2026, noon UTC
        let on = |q: &str| parse_query_window(q, now);
        assert_eq!(on("in 2023"), win((2023, 1, 1), (2024, 1, 1)));
        assert_eq!(on("en mars 2024"), win((2024, 3, 1), (2024, 4, 1)));
        assert_eq!(on("last week"), win((2026, 9, 21), (2026, 9, 28)));
        assert_eq!(on("il y a trois jours"), win((2026, 9, 30), (2026, 10, 3)));
        assert_eq!(
            on("what did we decide last week?"),
            win((2026, 9, 21), (2026, 9, 28))
        );
        assert_eq!(
            on("last week we chose RRF"),
            win((2026, 9, 21), (2026, 9, 28))
        );
        assert_eq!(on("this year"), win((2026, 1, 1), (2027, 1, 1)));
        assert_eq!(
            on("la semaine dernière on a choisi RRF"),
            win((2026, 9, 21), (2026, 9, 28))
        );
        assert_eq!(on("ce mois-ci"), win((2026, 10, 1), (2026, 11, 1)));
    }

    #[test]
    fn iso_date_is_the_day() {
        assert_eq!(
            parse("what happened on 2023-04-12?"),
            win((2023, 4, 12), (2023, 4, 13))
        );
        assert_eq!(
            parse("log 2023-04-12T08:15:00Z"),
            win((2023, 4, 12), (2023, 4, 13))
        );
        // Not a calendar date, or glued to other digits.
        assert_eq!(parse("2023-02-30"), None);
        assert_eq!(parse("12023-04-12"), None);
        assert_eq!(parse("2023-04-123"), None);
        // An invalid date does not hide a valid one further on.
        assert_eq!(
            parse("2023-13-01 then 2023-04-12"),
            win((2023, 4, 12), (2023, 4, 13))
        );
    }

    #[test]
    fn the_most_explicit_expression_wins() {
        assert_eq!(
            parse("last week, on 2023-04-12"),
            win((2023, 4, 12), (2023, 4, 13))
        );
        assert_eq!(
            parse("yesterday or in March 2021"),
            win((2021, 3, 1), (2021, 4, 1))
        );
        assert_eq!(
            parse("last year, three days ago"),
            win((2023, 5, 26), (2023, 5, 29))
        );
        assert_eq!(
            parse("in 2022 or yesterday"),
            win((2023, 5, 29), (2023, 5, 30))
        );
    }

    // --- parse_query_window: calendar edges ---

    #[test]
    fn month_and_year_rollover() {
        let new_year = at(2023, 1, 1, 9, 0); // a Sunday
        assert_eq!(
            parse_query_window("yesterday", new_year),
            win((2022, 12, 31), (2023, 1, 1))
        );
        assert_eq!(
            parse_query_window("last month", new_year),
            win((2022, 12, 1), (2023, 1, 1))
        );
        assert_eq!(
            parse_query_window("le mois dernier", new_year),
            win((2022, 12, 1), (2023, 1, 1))
        );
        assert_eq!(
            parse_query_window("last year", new_year),
            win((2022, 1, 1), (2023, 1, 1))
        );
        // Sunday 1 January belongs to the ISO week that began on 26 December.
        assert_eq!(
            parse_query_window("this week", new_year),
            win((2022, 12, 26), (2023, 1, 2))
        );
        assert_eq!(
            parse_query_window("last week", at(2023, 1, 3, 9, 0)),
            win((2022, 12, 26), (2023, 1, 2))
        );
        assert_eq!(
            parse_query_window("in December", new_year),
            win((2022, 12, 1), (2023, 1, 1))
        );
        assert_eq!(
            parse_query_window("il y a 3 jours", at(2023, 3, 1, 12, 0)),
            win((2023, 2, 25), (2023, 2, 28))
        );
        assert_eq!(
            parse_query_window("this month", at(2023, 12, 31, 23, 59)),
            win((2023, 12, 1), (2024, 1, 1))
        );
    }

    #[test]
    fn leap_day() {
        let leap = at(2024, 2, 29, 12, 0);
        assert_eq!(
            parse_query_window("today", leap),
            win((2024, 2, 29), (2024, 3, 1))
        );
        // One year before 29 February is 28 February.
        assert_eq!(
            parse_query_window("a year ago", leap),
            win((2022, 8, 28), (2023, 8, 29))
        );
        // One month before 31 March clamps to 29 February.
        assert_eq!(
            parse_query_window("a month ago", at(2024, 3, 31, 12, 0)),
            win((2024, 2, 13), (2024, 3, 17))
        );
        assert_eq!(
            parse_query_window("in February", at(2024, 3, 10, 12, 0)),
            win((2024, 2, 1), (2024, 3, 1))
        );
        assert_eq!(
            parse_query_window("yesterday", at(2024, 3, 1, 0, 0)),
            win((2024, 2, 29), (2024, 3, 1))
        );
        assert_eq!(
            parse_query_window("what happened on 2024-02-29", leap),
            win((2024, 2, 29), (2024, 3, 1))
        );
        assert_eq!(
            parse_query_window("29 February 2024", leap),
            win((2024, 2, 29), (2024, 3, 1))
        );
        // No 29 February in 2023: the month.
        assert_eq!(
            parse_query_window("29 February 2023", leap),
            win((2023, 2, 1), (2023, 3, 1))
        );
    }

    #[test]
    fn relative_expressions_follow_the_anchor_not_the_clock() {
        let q = "what did we decide last week";
        let from_2023 = parse_query_window(q, anchor());
        let from_2026 = parse_query_window(q, at(2026, 10, 3, 12, 0));
        assert_eq!(from_2023, win((2023, 5, 22), (2023, 5, 29)));
        assert_eq!(from_2026, win((2026, 9, 21), (2026, 9, 28)));
        // Absolute expressions do not move.
        assert_eq!(
            parse_query_window("in March 2021", at(2026, 10, 3, 12, 0)),
            win((2021, 3, 1), (2021, 4, 1))
        );
    }

    #[test]
    fn a_window_always_contains_its_own_centre() {
        // Whatever the time of day of the anchor, "today" contains it.
        for hour in [0, 12, 23] {
            let now = at(2023, 5, 30, hour, 59);
            let w = parse_query_window("today", now).unwrap();
            assert!(w.contains(now));
            assert!(w.start < w.end);
        }
    }

    #[test]
    fn long_text_without_dates_is_handled() {
        let long = "the quick brown fox jumps over the lazy dog ".repeat(500);
        assert_eq!(parse(&long), None);
        // Multi-byte text right around an ISO-shaped run must not split a char.
        assert_eq!(parse("é2023-04-12é"), win((2023, 4, 12), (2023, 4, 13)));
        assert_eq!(parse("日本語のクエリ"), None);
    }

    // --- parse_query_window_at: the caller's offset ---

    /// 01:30 in Paris on 4 October is still 3 October in UTC. "hier" must
    /// be the Parisian yesterday (3 October), not the UTC one (2 October).
    #[test]
    fn yesterday_at_0130_in_paris_is_the_local_yesterday() {
        let now = at(2026, 10, 3, 23, 30); // 2026-10-04T01:30+02:00
        let paris = east(2);
        let w = parse_query_window_at("qu'a-t-on décidé hier", now, paris);
        // 3 October, Paris time.
        assert_eq!(w, utc_range(at(2026, 10, 2, 22, 0), at(2026, 10, 3, 22, 0)));
        let w = w.unwrap();
        assert!(w.contains(at(2026, 10, 3, 13, 0))); // 15:00 on the 3rd, Paris
        assert!(w.contains(at(2026, 10, 2, 22, 0))); // 00:00 on the 3rd
        assert!(!w.contains(at(2026, 10, 3, 22, 0))); // 00:00 on the 4th
        assert!(!w.contains(at(2026, 10, 2, 13, 0))); // 15:00 on the 2nd

        // "aujourd'hui" is the 4th in Paris, and contains the instant asked.
        let today = parse_query_window_at("aujourd'hui", now, paris).unwrap();
        assert_eq!(today.start, at(2026, 10, 3, 22, 0));
        assert!(today.contains(now));

        // The UTC variant keeps its meaning: the UTC yesterday.
        assert_eq!(
            parse_query_window("qu'a-t-on décidé hier", now),
            win((2026, 10, 2), (2026, 10, 3))
        );
    }

    /// 17:30 in San Francisco on 3 October is already 4 October in UTC.
    /// "today" must be the local 3 October, which holds the local morning.
    #[test]
    fn today_at_1730_in_san_francisco_is_the_local_day() {
        let now = at(2026, 10, 4, 0, 30); // 2026-10-03T17:30-07:00
        let pdt = east(-7);
        let w = parse_query_window_at("what did we decide today", now, pdt);
        assert_eq!(w, utc_range(at(2026, 10, 3, 7, 0), at(2026, 10, 4, 7, 0)));
        let w = w.unwrap();
        assert!(w.contains(at(2026, 10, 3, 17, 0))); // 10:00 PDT, this morning
        assert!(w.contains(now));
        assert!(!w.contains(at(2026, 10, 3, 6, 59))); // 23:59 PDT the day before

        let yesterday = parse_query_window_at("yesterday", now, pdt);
        assert_eq!(
            yesterday,
            utc_range(at(2026, 10, 2, 7, 0), at(2026, 10, 3, 7, 0))
        );

        // The UTC variant: the UTC day, which misses the local morning.
        let utc = parse_query_window("what did we decide today", now).unwrap();
        assert!(!utc.contains(at(2026, 10, 3, 17, 0)));
    }

    #[test]
    fn a_zero_offset_is_the_utc_variant() {
        let zero = east(0);
        for q in [
            "today",
            "hier",
            "last week",
            "il y a trois jours",
            "in March 2021",
            "in 2022",
            "2023-04-12",
            "this year's kernel",
            "no date here",
        ] {
            for now in [anchor(), review_anchor(), at(2024, 2, 29, 23, 59)] {
                assert_eq!(
                    parse_query_window_at(q, now, zero),
                    parse_query_window(q, now),
                    "{q:?}"
                );
            }
        }
    }

    /// Weeks, months, years and absolute dates are cut at local midnight
    /// too, and a bare year is "not in the future" judged on the local date.
    #[test]
    fn periods_and_absolute_dates_follow_the_offset() {
        // Monday 5 October 2026, 00:30 in Paris: still Sunday 4 in UTC.
        let now = at(2026, 10, 4, 22, 30);
        let paris = east(2);
        let on = |q: &str| parse_query_window_at(q, now, paris);
        assert_eq!(
            on("this week"),
            utc_range(at(2026, 10, 4, 22, 0), at(2026, 10, 11, 22, 0))
        );
        assert_eq!(
            on("la semaine dernière"),
            utc_range(at(2026, 9, 27, 22, 0), at(2026, 10, 4, 22, 0))
        );
        assert_eq!(
            on("ce mois-ci"),
            utc_range(at(2026, 9, 30, 22, 0), at(2026, 10, 31, 22, 0))
        );
        assert_eq!(
            on("l'an dernier"),
            utc_range(at(2024, 12, 31, 22, 0), at(2025, 12, 31, 22, 0))
        );
        assert_eq!(
            on("il y a 3 jours"),
            utc_range(at(2026, 9, 30, 22, 0), at(2026, 10, 3, 22, 0))
        );
        assert_eq!(
            on("2026-10-05"),
            utc_range(at(2026, 10, 4, 22, 0), at(2026, 10, 5, 22, 0))
        );
        assert_eq!(
            on("en mars 2024"),
            utc_range(at(2024, 2, 29, 22, 0), at(2024, 3, 31, 22, 0))
        );
        // In UTC the same date is cut at UTC midnight (and, being a full
        // date, is a window even though the 5th is tomorrow there).
        assert_eq!(
            parse_query_window("2026-10-05", now),
            win((2026, 10, 5), (2026, 10, 6))
        );
        assert_eq!(
            parse_query_window("this week", now),
            win((2026, 9, 28), (2026, 10, 5))
        );

        // New Year's Eve, 20:00 in New York: 2027 in UTC, 2026 locally.
        let now = at(2027, 1, 1, 1, 0);
        let est = east(-5);
        assert_eq!(
            parse_query_window_at("this year", now, est),
            utc_range(at(2026, 1, 1, 5, 0), at(2027, 1, 1, 5, 0))
        );
        assert_eq!(parse_query_window_at("in 2027", now, est), None);
        assert_eq!(
            parse_query_window("in 2027", now),
            win((2027, 1, 1), (2028, 1, 1))
        );
    }

    #[test]
    fn extreme_offsets_do_not_panic() {
        let now = review_anchor();
        for seconds in [14 * 3600, -12 * 3600, 5 * 3600 + 1800, -(9 * 3600 + 1800)] {
            let offset = FixedOffset::east_opt(seconds).unwrap();
            for q in ["today", "last year", "9999 years ago", "in 1990", "hier"] {
                if let Some(w) = parse_query_window_at(q, now, offset) {
                    assert!(w.start < w.end, "{q:?} at {offset}");
                }
            }
            let today = parse_query_window_at("today", now, offset).unwrap();
            assert!(today.contains(now));
            assert_eq!(today.end - today.start, chrono::Duration::hours(24));
        }
    }
}
