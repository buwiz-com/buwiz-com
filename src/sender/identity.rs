//! Does a plaintext eBIRForms return name the TIN, form and period its
//! ticket (v2) or its file name (v1) names?
//!
//! The XML is a list of `<div>{form}:{field}={value}{form}:{field}=</div>`
//! lines. The checks:
//! - the field prefix is the ticket's form (`frm1701q` for `1701Qv2018`;
//!   some forms keep the version, `frm2550qv2024`);
//! - the first `…TIN1`, `…TIN2`, `…TIN3` and branch-code fields spell the
//!   ticket's TIN;
//! - the first year field's year is in the ticket's period, and when the
//!   return ticks a quarter (`…Quarter_N` / `…QuarterN`) and the period
//!   names one (`…Q3`), they agree;
//! - none of those fields comes twice with different values, and nothing
//!   in the markup reads otherwise to an XML reader than to the split on
//!   `<div>` (comments, CDATA, processing instructions after the prolog,
//!   `<div` with attributes or in other case).
//!
//! What it still assumes: BIR's reader takes the identity fields this
//! check takes (the first whose name ends in `TIN1`, …). A decoy field
//! with another name (`xTIN1`) placed first is read by this check and may
//! not be by BIR's; telling them apart needs each form's exact field names
//! as BIR reads them, which are not public.

use super::super::submissions::IafName;
use super::super::tickets::TicketClaims;

struct Field<'a> {
    prefix: &'a str,
    name: &'a str,
    value: &'a str,
}

fn fields(xml: &str) -> Vec<Field<'_>> {
    let mut out = Vec::new();
    for chunk in xml.split("<div>").skip(1) {
        let Some(line) = chunk.split("</div>").next() else { continue };
        let Some((key, rest)) = line.split_once('=') else { continue };
        let Some((prefix, name)) = key.split_once(':') else { continue };
        // The value runs up to the repeated `{key}=` that closes the line.
        let value = rest.strip_suffix(&format!("{key}=")).unwrap_or(rest);
        out.push(Field { prefix, name, value });
    }
    out
}

fn first<'a>(fields: &'a [Field<'a>], test: impl Fn(&str) -> bool) -> Option<&'a str> {
    fields.iter().find(|field| test(&field.name.to_ascii_lowercase())).map(|field| field.value.trim())
}

/// A field this check reads to name the return: a TIN part, the branch, a
/// year or a quarter tick (`name` lowercased).
fn names_the_return(name: &str) -> bool {
    ["tin1", "tin2", "tin3", "branchcode"].iter().any(|suffix| name.ends_with(suffix))
        || (name.contains("year") && !name.contains("birth"))
        || name.starts_with("datequarter_")
        || name.starts_with("optquarter")
}

/// At most 40 characters of something taken from the return, for a
/// refusal's words (they go into the answer and into `submissions.error`).
fn quoted(text: &str) -> String {
    let mut shown: String = text.chars().take(40).map(|c| if c.is_control() { '?' } else { c }).collect();
    if text.chars().count() > 40 {
        shown.push('…');
    }
    shown
}

/// Refuses markup that an XML reader and this check's split on `<div>`
/// would read differently: a comment or CDATA section (one of them sees
/// what it hides, the other does not), a processing instruction other
/// than the leading prolog, and a `<div`/`</div` tag that is not exactly
/// `<div>`/`</div>` (attributes, spaces or other case: the split does not
/// see it, a reader does). Other markup, and `&`, are left alone.
fn plain_markup(xml: &str) -> Result<(), String> {
    let lower = xml.to_ascii_lowercase();
    if lower.contains("<!--") {
        return Err("the return has an XML comment".into());
    }
    if lower.contains("<![cdata[") {
        return Err("the return has a CDATA section".into());
    }
    let body = xml.trim_start_matches(|c: char| c == '\u{feff}' || c.is_whitespace());
    let prolog = if body.starts_with("<?xml") { body.find("?>").map(|end| end + 2).unwrap_or(0) } else { 0 };
    if body[prolog..].contains("<?") {
        return Err("the return has a processing instruction after its prolog".into());
    }
    for (at, _) in lower.match_indices('<') {
        let tag = &lower[at + 1..];
        let name = tag.strip_prefix('/').unwrap_or(tag);
        let Some(after) = name.strip_prefix("div") else { continue };
        // `<divider>` and the like are other elements, not a div.
        if after.starts_with(|c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':')) {
            continue;
        }
        let exact = if tag.starts_with('/') { "</div>" } else { "<div>" };
        if !xml[at..].starts_with(exact) {
            return Err(
                "the return has a div tag with attributes or in other case, which this check does not read".into()
            );
        }
    }
    Ok(())
}

/// Refuses a field that names the return given twice with different
/// values: a reader that keeps the last copy, or matches names in another
/// case, would read another TIN, branch or period than this check's first.
fn one_value_each(fields: &[Field<'_>]) -> Result<(), String> {
    let mut seen: Vec<(String, &str)> = Vec::new();
    for field in fields {
        let name = field.name.to_ascii_lowercase();
        if !names_the_return(&name) {
            continue;
        }
        let value = field.value.trim();
        match seen.iter().find(|(known, _)| *known == name) {
            Some((_, first)) if *first != value => {
                return Err(format!("the return names {} twice, with different values", quoted(field.name)));
            }
            Some(_) => {}
            None => seen.push((name, value)),
        }
    }
    Ok(())
}

/// What a return must name: the 14-digit TIN (root and branch), the form
/// and the period, and what named them, for the refusal's words.
pub struct Expected<'a> {
    pub tin: &'a str,
    pub form: &'a str,
    pub period: &'a str,
    /// `ticket` or `file name`.
    pub named_by: &'a str,
}

/// A sealed return against its ticket.
pub fn check(xml: &[u8], ticket: &TicketClaims) -> Result<(), String> {
    check_against(xml, &Expected { tin: &ticket.tin, form: &ticket.form, period: &ticket.period, named_by: "ticket" })
}

/// A v1 return against its file name (`{TIN}-{form}-{period}#…#.xml`).
pub fn check_named(xml: &[u8], name: &IafName) -> Result<(), String> {
    let tin = format!("{}{}", name.tin.root(), name.tin.branch());
    check_against(xml, &Expected { tin: &tin, form: &name.form_type, period: &name.period, named_by: "file name" })
}

/// The return names `expected`'s TIN, form and period. Errors say which
/// does not, in words for the person sending.
pub fn check_against(xml: &[u8], expected: &Expected<'_>) -> Result<(), String> {
    let by = expected.named_by;
    let xml = std::str::from_utf8(xml).map_err(|_| "the return is not UTF-8 text".to_owned())?;
    plain_markup(xml)?;
    let fields = fields(xml);
    let Some(prefix) = fields.first().map(|field| field.prefix.to_ascii_lowercase()) else {
        return Err("the return has no form fields".into());
    };
    if fields.iter().any(|field| !field.prefix.eq_ignore_ascii_case(&prefix)) {
        return Err("the return mixes fields of several forms".into());
    }
    let form = expected.form.to_ascii_lowercase();
    let base = match form.rfind('v') {
        Some(at) if form.len() - at == 5 && form[at + 1..].bytes().all(|byte| byte.is_ascii_digit()) => &form[..at],
        _ => form.as_str(),
    };
    if prefix != format!("frm{form}") && prefix != format!("frm{base}") {
        return Err(format!("the return is a {} form, not the {by}'s {}", quoted(&prefix), expected.form));
    }
    one_value_each(&fields)?;

    let part = |suffix: &str| first(&fields, |name| name.ends_with(suffix)).unwrap_or_default();
    let tin = format!("{}{}{}", part("tin1"), part("tin2"), part("tin3"));
    let branch = first(&fields, |name| name.ends_with("branchcode")).unwrap_or_default();
    let branch = match branch.len() {
        3 => format!("00{branch}"),
        _ => branch.to_owned(),
    };
    if format!("{tin}{branch}") != expected.tin {
        return Err(format!("the TIN in the return is not the {by}'s TIN"));
    }

    let year = first(&fields, |name| name.contains("year") && !name.contains("birth"))
        .filter(|year| year.len() == 4 && year.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or("the return has no year")?;
    if !expected.period.contains(year) {
        return Err(format!("the return is for {year}, not the {by}'s period {}", expected.period));
    }
    let ticked_quarter = fields.iter().find_map(|field| {
        let name = field.name.to_ascii_lowercase();
        let digit = name.strip_prefix("datequarter_").or_else(|| name.strip_prefix("optquarter"))?;
        (field.value == "true" && digit.len() == 1).then(|| digit.to_owned())
    });
    let period_quarter = expected.period.rsplit_once('Q').map(|(_, quarter)| quarter.to_owned());
    if let (Some(ticked), Some(named)) = (ticked_quarter, period_quarter)
        && ticked != named
    {
        return Err(format!("the return is for quarter {ticked}, not the {by}'s period {}", expected.period));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn ticket(tin: &str, form: &str, period: &str) -> TicketClaims {
        TicketClaims {
            iss: String::new(),
            aud: String::new(),
            sub: Uuid::nil(),
            account_id: Uuid::nil(),
            device_id: Uuid::nil(),
            tin: tin.into(),
            form: form.into(),
            period: period.into(),
            payload_sha256: String::new(),
            email: String::new(),
            jti: Uuid::nil(),
            iat: 0,
            exp: 0,
        }
    }

    fn line(key: &str, value: &str) -> String {
        format!("<div>{key}={value}{key}=</div>\t\r\n            ")
    }

    fn xml_1701q(tin: [&str; 4], year: &str, quarter: u8) -> Vec<u8> {
        let mut xml = String::from("<?xml version='1.0'?>\t\r\n            ");
        xml += &line("frm1701q:txtYear", year);
        for q in 1..=3 {
            xml += &line(&format!("frm1701q:DateQuarter_{q}"), if q == quarter { "true" } else { "false" });
        }
        xml += &line("frm1701q:txtTIN1", tin[0]);
        xml += &line("frm1701q:txtTIN2", tin[1]);
        xml += &line("frm1701q:txtTIN3", tin[2]);
        xml += &line("frm1701q:txtBranchCode", tin[3]);
        xml += &line("frm1701q:txtBirthYear", "1990");
        xml += &line("frm1701q:txtPg2TIN1", "999");
        xml.into_bytes()
    }

    #[test]
    fn a_return_matches_its_ticket() {
        let xml = xml_1701q(["000", "123", "453", "00000"], "2026", 1);
        assert_eq!(check(&xml, &ticket("00012345300000", "1701Qv2018", "2026Q1")), Ok(()));
    }

    #[test]
    fn a_different_tin_form_or_period_is_refused() {
        let xml = xml_1701q(["000", "123", "453", "00000"], "2026", 1);
        assert!(check(&xml, &ticket("00012345400000", "1701Qv2018", "2026Q1")).unwrap_err().contains("TIN"));
        assert!(check(&xml, &ticket("00012345300000", "1601Cv2018", "012026")).unwrap_err().contains("form"));
        assert!(check(&xml, &ticket("00012345300000", "1701Qv2018", "2025Q1")).unwrap_err().contains("2026"));
        assert!(check(&xml, &ticket("00012345300000", "1701Qv2018", "2026Q2")).unwrap_err().contains("quarter"));
        assert!(check(b"not xml", &ticket("00012345300000", "1701Qv2018", "2026Q1")).is_err());
    }

    #[test]
    fn the_ticket_refusals_read_as_before() {
        let xml = xml_1701q(["000", "123", "453", "00000"], "2026", 1);
        assert_eq!(
            check(&xml, &ticket("00012345400000", "1701Qv2018", "2026Q1")).unwrap_err(),
            "the TIN in the return is not the ticket's TIN"
        );
        assert_eq!(
            check(&xml, &ticket("00012345300000", "1601Cv2018", "012026")).unwrap_err(),
            "the return is a frm1701q form, not the ticket's 1601Cv2018"
        );
    }

    #[test]
    fn a_v1_return_matches_its_file_name() {
        let name = |filename: &str| crate::filing::submissions::parse_filename(filename).unwrap();
        let xml = xml_1701q(["000", "123", "453", "00000"], "2026", 1);
        assert_eq!(check_named(&xml, &name("00012345300000-1701Qv2018-2026Q1#a@example.test#.xml")), Ok(()));
        // A 12-digit name with a 3-digit branch is the same head office.
        assert_eq!(check_named(&xml, &name("000123453000-1701Qv2018-2026Q1#a@example.test#.xml")), Ok(()));
        for (filename, words) in [
            (
                "00012345400000-1701Qv2018-2026Q1#a@example.test#.xml",
                "the TIN in the return is not the file name's TIN",
            ),
            (
                "00012345300001-1701Qv2018-2026Q1#a@example.test#.xml",
                "the TIN in the return is not the file name's TIN",
            ),
            ("00012345300000-1601Cv2018-012026#a@example.test#.xml", "not the file name's 1601Cv2018"),
            ("00012345300000-1701Qv2018-2025Q1#a@example.test#.xml", "not the file name's period 2025Q1"),
            ("00012345300000-1701Qv2018-2026Q2#a@example.test#.xml", "quarter 1, not the file name's period 2026Q2"),
        ] {
            let refused = check_named(&xml, &name(filename)).unwrap_err();
            assert!(refused.contains(words), "{filename}: {refused}");
        }
        assert!(check_named(b"<return/>", &name("00012345300000-1701Qv2018-2026Q1#a@example.test#.xml")).is_err());
    }

    /// `xml_1701q` for the head office of 000-123-453 in 2026 Q1, with
    /// `extra` put in right after the prolog.
    fn with(extra: &str) -> Vec<u8> {
        let xml = String::from_utf8(xml_1701q(["000", "123", "453", "00000"], "2026", 1)).unwrap();
        xml.replacen("?>", &format!("?>{extra}"), 1).into_bytes()
    }

    fn refused(xml: &[u8]) -> String {
        check(xml, &ticket("00012345300000", "1701Qv2018", "2026Q1")).unwrap_err()
    }

    /// The same identity field twice with different values: a reader that
    /// keeps the last one would read another TIN, branch or period than
    /// this check's first. Equal copies, and repeated other fields, are
    /// fine.
    #[test]
    fn an_identity_field_given_twice_with_different_values_is_refused() {
        for (field, value) in [
            ("txtTIN1", "999"),
            ("txtTIN3", "454"),
            ("TXTTIN1", "999"),
            ("txtBranchCode", "00001"),
            ("txtYear", "2025"),
            ("DateQuarter_2", "true"),
        ] {
            let xml = format!("{}{}", String::from_utf8(with("")).unwrap(), line(&format!("frm1701q:{field}"), value));
            let refused = refused(xml.as_bytes());
            assert!(refused.contains("twice") && refused.contains("different values"), "{field}: {refused}");
        }
        let same = format!("{}{}", String::from_utf8(with("")).unwrap(), line("frm1701q:txtTIN1", "000"));
        assert_eq!(check(same.as_bytes(), &ticket("00012345300000", "1701Qv2018", "2026Q1")), Ok(()));
        let other = format!("{0}{1}{1}", String::from_utf8(with("")).unwrap(), line("frm1701q:txtRemarks", "x"));
        assert_eq!(check(other.as_bytes(), &ticket("00012345300000", "1701Qv2018", "2026Q1")), Ok(()));
    }

    /// Markup an XML reader treats otherwise than this check's split: a
    /// comment or CDATA hiding a field from one of them, a processing
    /// instruction after the prolog, a `<div` with attributes or in other
    /// case that the split does not see.
    #[test]
    fn markup_the_split_reads_otherwise_is_refused() {
        let decoy = "<div>frm1701q:txtTIN1=999frm1701q:txtTIN1=</div>";
        for extra in [
            format!("<!-- {decoy} -->"),
            format!("<![CDATA[{decoy}]]>"),
            format!("<![cdata[{decoy}]]>"),
            "<?php echo 1; ?>".to_owned(),
            "<div class=\"x\">frm1701q:txtTIN1=999frm1701q:txtTIN1=</div>".to_owned(),
            "<DIV>frm1701q:txtTIN1=999frm1701q:txtTIN1=</DIV>".to_owned(),
            "<div >frm1701q:txtTIN1=999frm1701q:txtTIN1=</div>".to_owned(),
            "<div>frm1701q:txtRemarks=xfrm1701q:txtRemarks=</div >".to_owned(),
        ] {
            let refused = refused(&with(&extra));
            assert!(refused.contains("the return has"), "{extra}: {refused}");
        }
        // The prolog itself, after a byte-order mark or whitespace, is fine.
        let plain = String::from_utf8(with("")).unwrap();
        for xml in [format!("\u{feff}{plain}"), format!("  \r\n{plain}")] {
            assert_eq!(check(xml.as_bytes(), &ticket("00012345300000", "1701Qv2018", "2026Q1")), Ok(()), "{xml:?}");
        }
    }

    /// What a refusal repeats from the return is cut short.
    #[test]
    fn a_refusal_repeats_little_of_the_return() {
        let long = "x".repeat(5000);
        let xml = format!("<div>frm{long}:txtYear=2026frm{long}:txtYear=</div>");
        let refused = refused(xml.as_bytes());
        assert!(refused.chars().count() < 200, "{refused}");
        let field = format!("{long}TIN1");
        let xml = format!(
            "{}{}{}",
            String::from_utf8(with("")).unwrap(),
            line(&format!("frm1701q:{field}"), "1"),
            line(&format!("frm1701q:{field}"), "2")
        );
        assert!(refused_len(&xml) < 200);
    }

    fn refused_len(xml: &str) -> usize {
        refused(xml.as_bytes()).chars().count()
    }

    /// The fixtures (`tests/fixtures/iaf/`) belong to the BIR relay files.
    #[cfg(feature = "bir")]
    #[test]
    fn the_fixture_returns_match() {
        let fixture = include_bytes!("../../tests/fixtures/iaf/1601C-062025.plain.xml");
        assert_eq!(check(fixture, &ticket("12345678800000", "1601Cv2018", "062025")), Ok(()));
        let fixture = include_bytes!("../../tests/fixtures/iaf/2550Q-122025Q1.plain.xml");
        let tin = fields(std::str::from_utf8(fixture).unwrap())
            .iter()
            .filter(|field| field.name.starts_with("txtTIN") || field.name == "branchCode")
            .map(|field| field.value)
            .take(4)
            .collect::<String>();
        assert_eq!(check(fixture, &ticket(&tin, "2550Qv2024", "122025Q1")), Ok(()));
    }
}
