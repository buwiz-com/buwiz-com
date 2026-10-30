//! The fields staff compare, picked out of the text read from a TIN ID or a
//! Form 2303: the TIN, the name (and trade name), the birth date or RDO
//! code, and which document it looks like. Then each field beside what the
//! claimant typed. Pure functions over lines of text, so the OCR engine and
//! a PDF's text layer feed the same code.
//!
//! OCR text is noisy: `O` for `0`, `l` for `1`, labels and values split
//! across lines, a two-line name. The rules below are tolerant of that, and
//! they only help staff look; nothing is decided from them.

use serde::{Deserialize, Serialize};

use crate::tin::Tin;

/// One line of text, top to bottom, with how sure the reader was (0 to 1).
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub text: String,
    pub confidence: f32,
}

impl Line {
    pub fn new(text: impl Into<String>, confidence: f32) -> Self {
        Self { text: text.into(), confidence }
    }
}

/// What the document looks like.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    TinId,
    Form2303,
    Other,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::TinId => "TIN ID",
            Self::Form2303 => "Form 2303",
            Self::Other => "Something else",
        }
    }
}

/// The fields read from one document. Only these are stored, never the
/// whole text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fields {
    pub kind: Kind,
    /// `123-456-789` or with the branch as printed, `123-456-789-000`.
    pub tin: Option<String>,
    /// The TIN ID's name, or the 2303's taxpayer (registered) name.
    pub name: Option<String>,
    pub trade_name: Option<String>,
    /// ISO date, from a TIN ID.
    pub birth_date: Option<String>,
    /// From a 2303: three digits, and a letter for split offices (`053A`).
    pub rdo: Option<String>,
}

/// How sure the reader was of each field: the confidence of the line or
/// lines it came from.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Confidence {
    pub tin: Option<f32>,
    pub name: Option<f32>,
    pub trade_name: Option<f32>,
    pub birth_date: Option<f32>,
    pub rdo: Option<f32>,
}

/// Picks the fields out of the lines read from a document.
pub fn extract(lines: &[Line]) -> (Fields, Confidence) {
    let lines: Vec<Line> = lines
        .iter()
        .map(|line| Line::new(line.text.split_whitespace().collect::<Vec<_>>().join(" "), line.confidence))
        .filter(|line| !line.text.is_empty())
        .collect();
    let kind = guess_kind(&lines);
    let mut confidence = Confidence::default();
    let tin = find_tin(&lines).map(|(tin, sure)| {
        confidence.tin = Some(sure);
        tin
    });
    let name_labels = match kind {
        Kind::TinId => TIN_ID_NAME_LABELS,
        Kind::Form2303 => FORM_2303_NAME_LABELS,
        Kind::Other => ANY_NAME_LABELS,
    };
    let mut name = labelled(&lines, name_labels, clean_name);
    if name.is_none() && kind != Kind::Form2303 {
        name = unlabelled_name(&lines);
    }
    let name = name.map(|(name, sure)| {
        confidence.name = Some(sure);
        name
    });
    let trade_name =
        if kind == Kind::TinId { None } else { labelled(&lines, TRADE_NAME_LABELS, clean_name) }.map(|(name, sure)| {
            confidence.trade_name = Some(sure);
            name
        });
    let birth_date = if kind == Kind::Form2303 { None } else { labelled(&lines, BIRTH_DATE_LABELS, parse_date) }.map(
        |(date, sure)| {
            confidence.birth_date = Some(sure);
            date
        },
    );
    let rdo = if kind == Kind::TinId { None } else { labelled(&lines, RDO_LABELS, parse_rdo) }.map(|(rdo, sure)| {
        confidence.rdo = Some(sure);
        rdo
    });
    (Fields { kind, tin, name, trade_name, birth_date, rdo }, confidence)
}

const TIN_ID_NAME_LABELS: &[&str] = &["NAME", "FULL NAME", "TAXPAYER NAME", "TAXPAYERS NAME"];
const FORM_2303_NAME_LABELS: &[&str] =
    &["TAXPAYER NAME", "TAXPAYERS NAME", "REGISTERED NAME", "NAME OF TAXPAYER", "TAXPAYER REGISTERED NAME"];
const ANY_NAME_LABELS: &[&str] =
    &["TAXPAYER NAME", "TAXPAYERS NAME", "REGISTERED NAME", "NAME OF TAXPAYER", "FULL NAME", "NAME"];
const TRADE_NAME_LABELS: &[&str] = &["TRADE NAME", "TRADE BUSINESS NAME", "BUSINESS NAME", "TRADENAME"];
const BIRTH_DATE_LABELS: &[&str] = &["BIRTH DATE", "DATE OF BIRTH", "BIRTHDAY", "DOB"];
const RDO_LABELS: &[&str] = &["RDO CODE", "RDO NO", "RDO"];

/// Every label above, and others printed on the two documents: a line that
/// is one of these alone is not a value.
const OTHER_LABELS: &[&str] = &[
    "TIN",
    "ADDRESS",
    "REGISTERED ADDRESS",
    "BUSINESS ADDRESS",
    "ISSUE DATE",
    "DATE ISSUED",
    "DATE OF ISSUE",
    "SIGNATURE",
    "TAX TYPE",
    "TAX TYPES",
    "LINE OF BUSINESS",
    "REGISTRATION DATE",
    "DATE OF REGISTRATION",
    "TAXPAYER TYPE",
    "FORM TYPE",
];

/// Words of the documents' printed headings, never part of a name.
const HEADING_WORDS: &[&str] = &[
    "REPUBLIC",
    "PHILIPPINES",
    "PILIPINAS",
    "DEPARTMENT",
    "FINANCE",
    "BUREAU",
    "INTERNAL",
    "REVENUE",
    "TAXPAYER",
    "IDENTIFICATION",
    "CARD",
    "CERTIFICATE",
    "REGISTRATION",
    "SIGNATURE",
    "ADDRESS",
    "BIRTH",
    "ISSUE",
    "DIGITAL",
    "BIR",
    "TIN",
    "FORM",
];

/// A character as labels are compared: upper case, with the digits OCR
/// reads for letters turned back into letters.
fn fold_letter(ch: char) -> char {
    match ch.to_ascii_uppercase() {
        '0' => 'O',
        '1' | '|' => 'I',
        '5' => 'S',
        '8' => 'B',
        other => other,
    }
}

/// The text after `label` at the start of `text`, ignoring case, spaces,
/// punctuation and OCR's digits-for-letters. `None` if `text` does not start
/// with the label as a whole word.
fn after_label<'a>(text: &'a str, label: &str) -> Option<&'a str> {
    let mut wanted = label.chars().filter(char::is_ascii_alphanumeric).peekable();
    let mut end = None;
    for (index, ch) in text.char_indices() {
        let Some(&next) = wanted.peek() else {
            end = Some(index);
            break;
        };
        if !ch.is_alphanumeric() {
            continue;
        }
        if fold_letter(ch) != next {
            return None;
        }
        wanted.next();
    }
    let end = match end {
        Some(end) => end,
        None if wanted.peek().is_none() => text.len(),
        None => return None,
    };
    let rest = &text[end..];
    if rest.starts_with(|ch: char| ch.is_alphabetic()) {
        return None;
    }
    Some(
        rest.trim_start_matches(|ch: char| ch.is_whitespace() || matches!(ch, ':' | '.' | '-' | '–' | '—' | ';' | '/')),
    )
}

/// A line that is only a label, with nothing after it.
fn is_bare_label(text: &str) -> bool {
    [TIN_ID_NAME_LABELS, FORM_2303_NAME_LABELS, TRADE_NAME_LABELS, BIRTH_DATE_LABELS, RDO_LABELS, OTHER_LABELS]
        .iter()
        .flat_map(|labels| labels.iter())
        .any(|label| after_label(text, label).is_some_and(|rest| rest.trim().is_empty()))
}

/// A line that starts with any known label.
fn starts_with_label(text: &str) -> bool {
    [TIN_ID_NAME_LABELS, FORM_2303_NAME_LABELS, TRADE_NAME_LABELS, BIRTH_DATE_LABELS, RDO_LABELS, OTHER_LABELS]
        .iter()
        .flat_map(|labels| labels.iter())
        .any(|label| after_label(text, label).is_some())
}

/// The value of the first line that starts with one of `labels`: the rest
/// of that line, or else the next line that is not a label. `parse` cleans
/// and checks it.
fn labelled(lines: &[Line], labels: &[&str], parse: fn(&str) -> Option<String>) -> Option<(String, f32)> {
    for (index, line) in lines.iter().enumerate() {
        let Some(rest) = labels.iter().find_map(|label| after_label(&line.text, label)) else {
            continue;
        };
        if let Some(value) = parse(rest) {
            // A name cut after "LAST," goes on to the next line.
            if rest.trim_end().ends_with(',')
                && let Some(next) = lines.get(index + 1).filter(|next| !starts_with_label(&next.text))
                && let Some(more) = parse(&format!("{} {}", rest.trim(), next.text))
            {
                return Some((more, line.confidence.min(next.confidence)));
            }
            return Some((value, line.confidence));
        }
        if !rest.trim().is_empty() {
            continue;
        }
        let Some(next) = lines.get(index + 1).filter(|next| !is_bare_label(&next.text)) else {
            continue;
        };
        if let Some(value) = parse(&next.text) {
            if next.text.trim_end().ends_with(',')
                && let Some(after) = lines.get(index + 2).filter(|after| !starts_with_label(&after.text))
                && let Some(more) = parse(&format!("{} {}", next.text, after.text))
            {
                return Some((more, next.confidence.min(after.confidence)));
            }
            return Some((value, next.confidence));
        }
    }
    None
}

/// A TIN ID name printed without a label: the first `LAST, FIRST MIDDLE`
/// line that is not a heading or an address.
fn unlabelled_name(lines: &[Line]) -> Option<(String, f32)> {
    lines.iter().find_map(|line| {
        let (last, first) = line.text.split_once(',')?;
        if line.text.chars().any(|ch| ch.is_ascii_digit()) || last.trim().is_empty() || first.trim().is_empty() {
            return None;
        }
        let words = name_words(&line.text);
        if words.iter().any(|word| HEADING_WORDS.contains(&word.as_str())) {
            return None;
        }
        clean_name(&line.text).map(|name| (name, line.confidence))
    })
}

/// A name as printed, without stray marks: letters, spaces and `,.'-&`.
/// `None` if it has fewer than two letters or looks like a heading.
fn clean_name(raw: &str) -> Option<String> {
    let kept: String = raw
        .chars()
        .map(|ch| match ch {
            '0' => 'O',
            '1' | '|' => 'I',
            '5' => 'S',
            other => other,
        })
        .filter(|ch| ch.is_alphabetic() || matches!(ch, ' ' | ',' | '.' | '\'' | '-' | '&'))
        .collect();
    let name = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    let name = name.trim_matches(|ch: char| matches!(ch, ',' | '.' | '-' | ' ' | '\'')).to_owned();
    let words = name_words(&name);
    if name.chars().filter(|ch| ch.is_alphabetic()).count() < 2
        || words.iter().all(|word| HEADING_WORDS.contains(&word.as_str()))
    {
        return None;
    }
    Some(name)
}

/// Upper-case words of a name, accents dropped (`Ñ` is `N`), punctuation
/// gone, and company suffixes spelled out.
fn name_words(name: &str) -> Vec<String> {
    let plain: String = name
        .chars()
        .map(|ch| match ch {
            'á' | 'à' | 'â' | 'ä' | 'Á' | 'À' | 'Â' | 'Ä' => 'A',
            'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' | 'Ë' => 'E',
            'í' | 'ì' | 'î' | 'ï' | 'Í' | 'Ì' | 'Î' | 'Ï' => 'I',
            'ó' | 'ò' | 'ô' | 'ö' | 'Ó' | 'Ò' | 'Ô' | 'Ö' => 'O',
            'ú' | 'ù' | 'û' | 'ü' | 'Ú' | 'Ù' | 'Û' | 'Ü' => 'U',
            'ñ' | 'Ñ' => 'N',
            '0' => 'O',
            '1' => 'I',
            '5' => 'S',
            '\'' | '’' => '\0',
            other if other.is_ascii_alphanumeric() => other.to_ascii_uppercase(),
            _ => ' ',
        })
        .filter(|ch| *ch != '\0')
        .collect();
    plain
        .split_whitespace()
        .map(|word| {
            match word {
                "CORP" => "CORPORATION",
                "INC" => "INCORPORATED",
                "CO" => "COMPANY",
                "LTD" => "LIMITED",
                "ENT" | "ENTS" => "ENTERPRISES",
                "INTL" => "INTERNATIONAL",
                "STA" => "SANTA",
                "STO" => "SANTO",
                other => other,
            }
            .to_owned()
        })
        .collect()
}

/// Digits OCR reads as letters, inside a run of digits.
fn digit_of(ch: char) -> Option<char> {
    match ch {
        '0'..='9' => Some(ch),
        'O' | 'o' | 'Q' | 'D' => Some('0'),
        'I' | 'l' | 'i' | '|' | '!' => Some('1'),
        'Z' | 'z' => Some('2'),
        'S' | 's' => Some('5'),
        'G' | 'b' => Some('6'),
        'B' => Some('8'),
        'g' | 'q' => Some('9'),
        _ => None,
    }
}

/// TINs in one line of text: `123-456-789`, `123 456 789 000`,
/// `123456789-00000`, with OCR's letters-for-digits fixed. A group made only
/// of letters (`NO.`) does not start a number.
fn tins_in(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut groups: Vec<String> = Vec::new();
    let mut group = String::new();
    let mut real_digits = 0;
    let mut gap = 0;
    let finish_group = |groups: &mut Vec<String>, group: &mut String, real_digits: &mut usize| -> bool {
        // Letters alone start no number, but `OOO` after `123-456-789` is a
        // branch read badly.
        let kept = !group.is_empty() && (*real_digits > 0 || !groups.is_empty());
        if kept {
            groups.push(std::mem::take(group));
        }
        group.clear();
        *real_digits = 0;
        kept
    };
    for ch in text.chars().chain(std::iter::once('\n')) {
        if let Some(digit) = digit_of(ch) {
            if gap > 3 {
                found.extend(tin_of_groups(&groups));
                groups.clear();
            }
            gap = 0;
            group.push(digit);
            if ch.is_ascii_digit() {
                real_digits += 1;
            }
        } else if matches!(ch, ' ' | '-' | '–' | '—' | '.' | '_') && !(group.is_empty() && groups.is_empty()) {
            if !group.is_empty() && !finish_group(&mut groups, &mut group, &mut real_digits) {
                found.extend(tin_of_groups(&groups));
                groups.clear();
            }
            gap += 1;
        } else {
            finish_group(&mut groups, &mut group, &mut real_digits);
            found.extend(tin_of_groups(&groups));
            groups.clear();
            gap = 0;
        }
    }
    found
}

/// A TIN from the digit groups of one number, or `None` if they do not make
/// one: three groups of three and an optional 3–5 digit branch, or 9, 12, 13
/// or 14 digits run together.
fn tin_of_groups(groups: &[String]) -> Option<String> {
    if groups.is_empty() {
        return None;
    }
    let lengths: Vec<usize> = groups.iter().map(String::len).collect();
    let all: String = groups.concat();
    let (root, branch) = if lengths.len() >= 3 && lengths[..3] == [3, 3, 3] {
        let branch = groups.get(3).filter(|branch| (3..=5).contains(&branch.len())).cloned();
        (all[..9].to_owned(), branch)
    } else if lengths.len() <= 2 && lengths[0] == 9 && matches!(all.len(), 9 | 12..=14) {
        (all[..9].to_owned(), Some(all[9..].to_owned()).filter(|branch| !branch.is_empty()))
    } else if lengths == [12] || lengths == [13] || lengths == [14] {
        (all[..9].to_owned(), Some(all[9..].to_owned()))
    } else {
        return None;
    };
    if root.bytes().all(|digit| digit == b'0') {
        return None;
    }
    let mut tin = format!("{}-{}-{}", &root[..3], &root[3..6], &root[6..]);
    if let Some(branch) = branch {
        tin = format!("{tin}-{branch}");
    }
    Some(tin)
}

/// Words of a line as labels are compared.
fn label_words(text: &str) -> Vec<String> {
    text.chars()
        .map(|ch| if ch.is_alphanumeric() { fold_letter(ch) } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

/// The TIN: one on a line labelled TIN (or just after that label) first,
/// else the first one anywhere. Lines split by the reader are tried joined
/// when no single line holds one.
fn find_tin(lines: &[Line]) -> Option<(String, f32)> {
    let labelled_tin = |text: &str| label_words(text).iter().any(|word| word == "TIN" || word == "TINNO");
    let mut first = None;
    for (index, line) in lines.iter().enumerate() {
        let Some(tin) = tins_in(&line.text).into_iter().next() else { continue };
        let near_label = labelled_tin(&line.text) || index > 0 && labelled_tin(&lines[index - 1].text);
        if near_label {
            return Some((tin, line.confidence));
        }
        first.get_or_insert((tin, line.confidence));
    }
    if first.is_some() {
        return first;
    }
    lines.windows(2).find_map(|pair| {
        let joined = format!("{} {}", pair[0].text, pair[1].text);
        tins_in(&joined).into_iter().next().map(|tin| (tin, pair[0].confidence.min(pair[1].confidence)))
    })
}

/// TIN ID, Form 2303 or something else, from the words printed on it.
fn guess_kind(lines: &[Line]) -> Kind {
    let raw: String = lines.iter().map(|line| line.text.as_str()).collect::<Vec<_>>().join(" ");
    let words = format!(" {} ", label_words(&raw).join(" "));
    let has = |phrase: &str| words.contains(&format!(" {phrase} "));
    let mut form_2303 = 0;
    if raw.split(|ch: char| !ch.is_ascii_digit()).any(|number| number == "2303") {
        form_2303 += 2;
    }
    for (phrase, weight) in [
        ("CERTIFICATE OF REGISTRATION", 2),
        ("TAX TYPE", 1),
        ("TAX TYPES", 1),
        ("LINE OF BUSINESS", 1),
        ("REGISTRATION DATE", 1),
        ("DATE OF REGISTRATION", 1),
        ("RDO CODE", 1),
        ("TRADE NAME", 1),
    ] {
        if has(phrase) {
            form_2303 += weight;
        }
    }
    let mut tin_id = 0;
    for (phrase, weight) in [
        ("TAXPAYER IDENTIFICATION", 2),
        ("TIN ID", 2),
        ("DIGITAL TIN", 2),
        ("IDENTIFICATION CARD", 1),
        ("BIRTH DATE", 1),
        ("DATE OF BIRTH", 1),
        ("BIRTHDATE", 1),
    ] {
        if has(phrase) {
            tin_id += weight;
        }
    }
    match (tin_id, form_2303) {
        (id, form) if form >= 2 && form > id => Kind::Form2303,
        (id, form) if id >= 2 && id > form => Kind::TinId,
        _ => Kind::Other,
    }
}

const MONTHS: [&str; 12] = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"];

/// The first date in `text`, as `YYYY-MM-DD`: `1990/01/15`, `01/15/1990`
/// (month first, as BIR prints it), `January 15, 1990` or `15 JAN 1990`.
pub fn parse_date(text: &str) -> Option<String> {
    let tokens: Vec<&str> = text.split(|ch: char| !ch.is_alphanumeric()).filter(|token| !token.is_empty()).collect();
    let number = |token: &str| -> Option<u32> {
        let digits: Option<String> = token.chars().map(digit_of).collect();
        let digits = digits?;
        if token.chars().filter(char::is_ascii_digit).count() * 2 < token.len() {
            return None;
        }
        digits.parse().ok()
    };
    let month = |token: &str| -> Option<u32> {
        let upper = token.to_ascii_uppercase();
        if upper.len() < 3 {
            return None;
        }
        MONTHS.iter().position(|month| upper.starts_with(month)).map(|index| index as u32 + 1)
    };
    let date = |year: u32, month: u32, day: u32| -> Option<String> {
        let month = time::Month::try_from(u8::try_from(month).ok()?).ok()?;
        let date = time::Date::from_calendar_date(i32::try_from(year).ok()?, month, u8::try_from(day).ok()?).ok()?;
        (1900..=2100).contains(&year).then(|| date.to_string())
    };
    for window in tokens.windows(3) {
        let [a, b, c] = window else { continue };
        let found = match (number(a), number(b), number(c)) {
            (Some(year), Some(month), Some(day)) if a.len() == 4 => date(year, month, day),
            (Some(month), Some(day), Some(year)) if c.len() == 4 => date(year, month, day),
            _ => match (month(a), number(b), number(c), number(a), month(b)) {
                (Some(month), Some(day), Some(year), _, _) if c.len() == 4 => date(year, month, day),
                (_, _, Some(year), Some(day), Some(month)) if c.len() == 4 => date(year, month, day),
                _ => None,
            },
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// An RDO code: two or three digits, and `A` or `B` for a split office.
fn parse_rdo(text: &str) -> Option<String> {
    let token = text.split(|ch: char| !ch.is_alphanumeric()).find(|token| !token.is_empty())?;
    let (number, letter) = match token.char_indices().last() {
        Some((index, letter @ ('A' | 'B' | 'a' | 'b'))) if index >= 2 => (&token[..index], Some(letter)),
        _ => (token, None),
    };
    let digits: String = number.chars().map(digit_of).collect::<Option<String>>()?;
    if !(2..=3).contains(&digits.len()) || !number.chars().any(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let letter = letter.map(|letter| letter.to_ascii_uppercase().to_string()).unwrap_or_default();
    Some(format!("{digits:0>3}{letter}"))
}

/// How a field read from the document compares with what was typed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Match,
    /// Close: the TIN's root matches but not its branch; a name with a
    /// missing or misread word.
    Partial,
    Mismatch,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Match => "match",
            Self::Partial => "partial",
            Self::Mismatch => "mismatch",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [Self::Match, Self::Partial, Self::Mismatch].into_iter().find(|verdict| verdict.as_str() == raw)
    }
}

/// The TIN read beside the one typed: the 9-digit roots must be equal. A
/// branch printed on the document must match too (`000` is `00000`);
/// without one, the root decides.
pub fn compare_tin(typed: &Tin, read: &str) -> Verdict {
    let digits: String = read.chars().filter(char::is_ascii_digit).collect();
    if digits.len() < 9 || digits[..9] != *typed.root() {
        return Verdict::Mismatch;
    }
    let branch = &digits[9..];
    let number = |raw: &str| raw.parse::<u32>().ok();
    if branch.is_empty() || number(branch) == number(typed.branch()) { Verdict::Match } else { Verdict::Partial }
}

/// Two names, ignoring case, accents, punctuation and the order of
/// `LAST, FIRST MIDDLE` against `First Middle Last`. Every word equal is a
/// match; most words equal, or an initial for a middle name, or one letter
/// off, is partial.
pub fn compare_names(typed: &str, read: &str) -> Verdict {
    let typed_words = name_words(typed);
    let read_words = name_words(read);
    if typed_words.is_empty() || read_words.is_empty() {
        return Verdict::Mismatch;
    }
    let mut sorted_typed = typed_words.clone();
    let mut sorted_read = read_words.clone();
    sorted_typed.sort();
    sorted_read.sort();
    if sorted_typed == sorted_read || run_together(typed) == run_together(read) {
        return Verdict::Match;
    }
    // Pair each word of the shorter name with one of the longer.
    let (short, long) =
        if typed_words.len() <= read_words.len() { (&typed_words, &read_words) } else { (&read_words, &typed_words) };
    let mut unused: Vec<&String> = long.iter().collect();
    let mut paired = 0;
    for word in short {
        let found = unused
            .iter()
            .position(|other| *other == word)
            .or_else(|| unused.iter().position(|other| is_initial(word, other) || is_initial(other, word)))
            .or_else(|| unused.iter().position(|other| word.len() >= 4 && other.len() >= 4 && edits(word, other) <= 1));
        if let Some(index) = found {
            unused.remove(index);
            paired += 1;
        }
    }
    if paired == short.len() || paired >= 2 && paired * 2 >= long.len() { Verdict::Partial } else { Verdict::Mismatch }
}

/// A name in `First Middle Last` order with no spaces, so `DELA CRUZ` and
/// `DELACRUZ` meet.
fn run_together(name: &str) -> String {
    let ordered = match name.split_once(',') {
        Some((last, first)) => format!("{first} {last}"),
        None => name.to_owned(),
    };
    name_words(&ordered).concat()
}

fn is_initial(short: &str, long: &str) -> bool {
    short.len() == 1 && long.len() > 1 && long.starts_with(short)
}

/// Levenshtein distance, for words a letter apart.
fn edits(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let current = row[j + 1];
            row[j + 1] = if ca == *cb { previous } else { 1 + previous.min(row[j]).min(row[j + 1]) };
            previous = current;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<Line> {
        text.lines().map(|line| Line::new(line, 0.9)).collect()
    }

    fn tin(root: &str, branch: &str) -> Tin {
        Tin::parse(root, branch).unwrap()
    }

    #[test]
    fn finds_tins_with_spaces_dashes_and_ocr_noise() {
        for (text, expected) in [
            ("TIN: 123-456-789-000", "123-456-789-000"),
            ("TIN 123 456 789 00000", "123-456-789-00000"),
            ("123456789", "123-456-789"),
            ("123456789-0000", "123-456-789-0000"),
            ("TIN NO. 123-456-789", "123-456-789"),
            ("TIN: l23-4S6-7B9-OOO", "123-456-789-000"),
            ("TlN 1Z3 456 789", "123-456-789"),
            ("TIN : 123 - 456 - 789 - 000", "123-456-789-000"),
            ("123456789000", "123-456-789-000"),
            ("TIN 123.456.789.000", "123-456-789-000"),
            ("TIN—123–456–789", "123-456-789"),
        ] {
            assert_eq!(tins_in(text).first().map(String::as_str), Some(expected), "{text}");
        }
    }

    #[test]
    fn other_numbers_are_not_tins() {
        for text in [
            "09171234567",
            "BIRTH DATE 1990/01/15",
            "1234 RIZAL ST BRGY 123",
            "RDO 050",
            "SOLIS SAMSON",
            "000-000-000",
            "12-345-678",
            "123-456-7890",
        ] {
            assert_eq!(tins_in(text), Vec::<String>::new(), "{text}");
        }
    }

    #[test]
    fn prefers_the_tin_by_its_label() {
        let (fields, confidence) = extract(&lines(
            "Control No. 987654321\nREPUBLIC OF THE PHILIPPINES\nTAXPAYER IDENTIFICATION\nTIN\n123-456-789-000",
        ));
        assert_eq!(fields.tin.as_deref(), Some("123-456-789-000"));
        assert_eq!(confidence.tin, Some(0.9));
    }

    #[test]
    fn joins_a_tin_split_across_two_lines() {
        let (fields, _) = extract(&lines("TAXPAYER IDENTIFICATION CARD\n123-456\n-789-000\nJUAN"));
        assert_eq!(fields.tin.as_deref(), Some("123-456-789-000"));
    }

    const TIN_CARD: &str = "REPUBLIC OF THE PHILIPPINES
DEPARTMENT OF FINANCE
BUREAU OF INTERNAL REVENUE
TAXPAYER IDENTIFICATION
TIN 123-456-789-000
NAME
DELA CRUZ, JUAN SANTOS
ADDRESS
12 RIZAL ST, QUEZON CITY
BIRTH DATE 1990/01/15
ISSUE DATE 2020/02/03";

    #[test]
    fn reads_a_tin_id_card() {
        let (fields, confidence) = extract(&lines(TIN_CARD));
        assert_eq!(fields.kind, Kind::TinId);
        assert_eq!(fields.tin.as_deref(), Some("123-456-789-000"));
        assert_eq!(fields.name.as_deref(), Some("DELA CRUZ, JUAN SANTOS"));
        assert_eq!(fields.birth_date.as_deref(), Some("1990-01-15"));
        assert_eq!(fields.trade_name, None);
        assert_eq!(fields.rdo, None);
        assert_eq!(confidence.name, Some(0.9));
    }

    #[test]
    fn reads_a_noisy_tin_id_card() {
        let text = "REPUBL1C OF THE PH1LIPPINES
BUREAU 0F INTERNAL REVENUE
TAXPAYER IDENTIFICATI0N CARD
TIN: l23-456-7B9-0OO
NAME: REYES, MARIA C1ARA
BIRTHDATE: 03/2l/1985";
        let (fields, _) = extract(&lines(text));
        assert_eq!(fields.kind, Kind::TinId);
        assert_eq!(fields.tin.as_deref(), Some("123-456-789-000"));
        assert_eq!(fields.name.as_deref(), Some("REYES, MARIA CIARA"));
        assert_eq!(fields.birth_date.as_deref(), Some("1985-03-21"));
    }

    #[test]
    fn reads_a_name_without_a_label_and_one_split_over_two_lines() {
        let (fields, _) = extract(&lines(
            "BUREAU OF INTERNAL REVENUE\nTAXPAYER IDENTIFICATION\nTIN 123-456-789\nSANTOS, ANA MAE\n5 MABINI ST, CEBU",
        ));
        assert_eq!(fields.name.as_deref(), Some("SANTOS, ANA MAE"));
        let (fields, _) =
            extract(&lines("TAXPAYER IDENTIFICATION\nTIN 123-456-789\nNAME: DE LOS SANTOS,\nMARIA LUISA\nADDRESS"));
        assert_eq!(fields.name.as_deref(), Some("DE LOS SANTOS, MARIA LUISA"));
    }

    #[test]
    fn reads_the_digital_tin_id() {
        let text = "Digital TIN ID
Name
Juan Santos Dela Cruz
TIN
123456789-00000
Date of Birth
January 15, 1990
Address
Quezon City";
        let (fields, _) = extract(&lines(text));
        assert_eq!(fields.kind, Kind::TinId);
        assert_eq!(fields.tin.as_deref(), Some("123-456-789-00000"));
        assert_eq!(fields.name.as_deref(), Some("Juan Santos Dela Cruz"));
        assert_eq!(fields.birth_date.as_deref(), Some("1990-01-15"));
    }

    const FORM_2303: &str = "BIR Form No. 2303
Certificate of Registration
Republic of the Philippines
Department of Finance
Bureau of Internal Revenue
TIN 123 456 789 00000
RDO Code: 039 - South Quezon City
Taxpayer Name
ABC TRADING CORP.
Trade Name: ABC STORE
Registered Address
1 Main St, Quezon City
Registration Date: 01/05/2015
Tax Types
Income Tax";

    #[test]
    fn reads_a_form_2303() {
        let (fields, confidence) = extract(&lines(FORM_2303));
        assert_eq!(fields.kind, Kind::Form2303);
        assert_eq!(fields.tin.as_deref(), Some("123-456-789-00000"));
        assert_eq!(fields.name.as_deref(), Some("ABC TRADING CORP"));
        assert_eq!(fields.trade_name.as_deref(), Some("ABC STORE"));
        assert_eq!(fields.rdo.as_deref(), Some("039"));
        assert_eq!(fields.birth_date, None);
        assert_eq!(confidence.rdo, Some(0.9));
    }

    #[test]
    fn reads_older_2303_labels_and_split_rdos() {
        let text = "CERTIFICATE OF REGISTRATI0N
BIR FORM 2303
TIN: 987-654-321-000
RDO: 53A
REGISTERED NAME: PEÑAFLOR, JOSÉ R.
BUSINESS NAME
PEÑAFLOR'S BAKERY";
        let (fields, _) = extract(&lines(text));
        assert_eq!(fields.kind, Kind::Form2303);
        assert_eq!(fields.rdo.as_deref(), Some("053A"));
        assert_eq!(fields.name.as_deref(), Some("PEÑAFLOR, JOSÉ R"));
        assert_eq!(fields.trade_name.as_deref(), Some("PEÑAFLOR'S BAKERY"));
    }

    #[test]
    fn a_2303_without_a_trade_name_has_none() {
        let text = "Certificate of Registration\nTaxpayer Name: JUAN DELA CRUZ\nTrade Name\nTax Types\nRDO Code 050";
        let (fields, _) = extract(&lines(text));
        assert_eq!(fields.trade_name, None);
        assert_eq!(fields.name.as_deref(), Some("JUAN DELA CRUZ"));
        assert_eq!(fields.rdo.as_deref(), Some("050"));
    }

    #[test]
    fn guesses_other_documents() {
        for text in [
            "REPUBLIC OF THE PHILIPPINES\nDRIVER'S LICENSE\nDELA CRUZ, JUAN",
            "PHILIPPINE IDENTIFICATION CARD\nPambansang Pagkakakilanlan",
            "",
            "Official Receipt\nTotal 1,234.00",
        ] {
            assert_eq!(extract(&lines(text)).0.kind, Kind::Other, "{text}");
        }
    }

    #[test]
    fn labels_survive_ocr_noise_but_not_other_words() {
        assert_eq!(after_label("NAME: JUAN", "NAME"), Some("JUAN"));
        assert_eq!(after_label("Taxpayer's Name: JUAN", "TAXPAYERS NAME"), Some("JUAN"));
        assert_eq!(after_label("8IRTH DATE 1990/01/15", "BIRTH DATE"), Some("1990/01/15"));
        assert_eq!(after_label("BIRTHDATE", "BIRTH DATE"), Some(""));
        assert_eq!(after_label("NAMES OF HEIRS", "NAME"), None);
        assert_eq!(after_label("TRADE NAME", "NAME"), None);
        assert_eq!(after_label("RDO", "RDO CODE"), None);
    }

    #[test]
    fn parses_dates_as_bir_prints_them() {
        for (text, expected) in [
            ("1990/01/15", Some("1990-01-15")),
            ("1990-01-15", Some("1990-01-15")),
            ("01/15/1990", Some("1990-01-15")),
            ("Jan 15, 1990", Some("1990-01-15")),
            ("JANUARY 15 1990", Some("1990-01-15")),
            ("15 JAN 1990", Some("1990-01-15")),
            ("O1/l5/199O", Some("1990-01-15")),
            ("1990/02/30", None),
            ("13/13/1990", None),
            ("no date", None),
            ("1990/01/15 ISSUE DATE 2020/02/03", Some("1990-01-15")),
        ] {
            assert_eq!(parse_date(text).as_deref(), expected, "{text}");
        }
    }

    #[test]
    fn parses_rdo_codes() {
        assert_eq!(parse_rdo("039 - South Quezon City").as_deref(), Some("039"));
        assert_eq!(parse_rdo("53B").as_deref(), Some("053B"));
        assert_eq!(parse_rdo("O5O").as_deref(), Some("050"));
        assert_eq!(parse_rdo("South").as_deref(), None);
        assert_eq!(parse_rdo("1234").as_deref(), None);
    }

    #[test]
    fn tins_match_after_normalising() {
        let typed = tin("123456789", "");
        assert_eq!(compare_tin(&typed, "123-456-789"), Verdict::Match);
        assert_eq!(compare_tin(&typed, "123-456-789-000"), Verdict::Match);
        assert_eq!(compare_tin(&typed, "123-456-789-00000"), Verdict::Match);
        assert_eq!(compare_tin(&typed, "123-456-789-00001"), Verdict::Partial);
        assert_eq!(compare_tin(&typed, "123-456-788-000"), Verdict::Mismatch);
        assert_eq!(compare_tin(&typed, "12345"), Verdict::Mismatch);
        let branch = tin("123456789", "001");
        assert_eq!(compare_tin(&branch, "123-456-789-001"), Verdict::Match);
        assert_eq!(compare_tin(&branch, "123-456-789-000"), Verdict::Partial);
    }

    #[test]
    fn names_match_in_either_order_and_case() {
        for (typed, read) in [
            ("Juan Santos Dela Cruz", "DELA CRUZ, JUAN SANTOS"),
            ("juan santos dela cruz", "Juan Santos Dela Cruz"),
            ("Juan Santos dela Cruz", "DELACRUZ, JUAN SANTOS"),
            ("José Peñaflor", "PENAFLOR, JOSE"),
            ("ABC Trading Corp.", "ABC TRADING CORPORATION"),
            ("O'Brien, Mary", "MARY OBRIEN"),
            ("Ma. Clara Reyes", "REYES, MA CLARA"),
        ] {
            assert_eq!(compare_names(typed, read), Verdict::Match, "{typed} / {read}");
        }
    }

    #[test]
    fn names_that_nearly_match_are_partial() {
        for (typed, read) in [
            ("Juan Santos Dela Cruz", "DELA CRUZ, JUAN S."),
            ("Juan Dela Cruz", "DELA CRUZ, JUAN SANTOS"),
            ("Maria Clara Reyes", "REYES, MARIA CIARA"),
            ("Maria Clara Reyes", "REYES, MARlA CLARA"),
            ("ABC Trading", "ABC TRADING CORP"),
        ] {
            assert_eq!(compare_names(typed, read), Verdict::Partial, "{typed} / {read}");
        }
    }

    #[test]
    fn different_names_do_not_match() {
        for (typed, read) in [
            ("Juan Dela Cruz", "SANTOS, ANA MAE"),
            ("Juan Dela Cruz", "JUAN REYES"),
            ("ABC Trading Corp", "XYZ HOLDINGS INC"),
            ("Juan", ""),
        ] {
            assert_eq!(compare_names(typed, read), Verdict::Mismatch, "{typed} / {read}");
        }
    }

    #[test]
    fn whole_documents_compare_with_what_was_typed() {
        let (fields, _) = extract(&lines(TIN_CARD));
        assert_eq!(compare_tin(&tin("123-456-789", "000"), fields.tin.as_deref().unwrap()), Verdict::Match);
        assert_eq!(compare_names("Juan Santos Dela Cruz", fields.name.as_deref().unwrap()), Verdict::Match);
        let (fields, _) = extract(&lines(FORM_2303));
        assert_eq!(compare_tin(&tin("123456789", "00000"), fields.tin.as_deref().unwrap()), Verdict::Match);
        assert_eq!(compare_names("ABC Trading Corporation", fields.name.as_deref().unwrap()), Verdict::Match);
        assert_eq!(compare_names("ABC Store", fields.trade_name.as_deref().unwrap()), Verdict::Match);
        assert_eq!(compare_tin(&tin("987654321", ""), fields.tin.as_deref().unwrap()), Verdict::Mismatch);
    }

    #[test]
    fn fields_round_trip_through_json() {
        let (fields, confidence) = extract(&lines(TIN_CARD));
        let json = serde_json::to_string(&fields).unwrap();
        assert!(json.contains("\"kind\":\"tin_id\""), "{json}");
        assert_eq!(serde_json::from_str::<Fields>(&json).unwrap(), fields);
        let json = serde_json::to_string(&confidence).unwrap();
        assert_eq!(serde_json::from_str::<Confidence>(&json).unwrap(), confidence);
    }
}
