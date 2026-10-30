//! Does a plaintext eBIRForms return name the TIN, form and period its
//! ticket names?
//!
//! The XML is a list of `<div>{form}:{field}={value}{form}:{field}=</div>`
//! lines. The checks:
//! - the field prefix is the ticket's form (`frm1701q` for `1701Qv2018`;
//!   some forms keep the version, `frm2550qv2024`);
//! - the first `…TIN1`, `…TIN2`, `…TIN3` and branch-code fields spell the
//!   ticket's TIN;
//! - the first year field's year is in the ticket's period, and when the
//!   return ticks a quarter (`…Quarter_N` / `…QuarterN`) and the period
//!   names one (`…Q3`), they agree.

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

pub fn check(xml: &[u8], ticket: &TicketClaims) -> Result<(), String> {
    let xml = std::str::from_utf8(xml).map_err(|_| "the return is not UTF-8 text".to_owned())?;
    let fields = fields(xml);
    let Some(prefix) = fields.first().map(|field| field.prefix.to_ascii_lowercase()) else {
        return Err("the return has no form fields".into());
    };
    if fields.iter().any(|field| !field.prefix.eq_ignore_ascii_case(&prefix)) {
        return Err("the return mixes fields of several forms".into());
    }
    let form = ticket.form.to_ascii_lowercase();
    let base = match form.rfind('v') {
        Some(at) if form.len() - at == 5 && form[at + 1..].bytes().all(|byte| byte.is_ascii_digit()) => &form[..at],
        _ => form.as_str(),
    };
    if prefix != format!("frm{form}") && prefix != format!("frm{base}") {
        return Err(format!("the return is a {prefix} form, not the ticket's {}", ticket.form));
    }

    let part = |suffix: &str| first(&fields, |name| name.ends_with(suffix)).unwrap_or_default();
    let tin = format!("{}{}{}", part("tin1"), part("tin2"), part("tin3"));
    let branch = first(&fields, |name| name.ends_with("branchcode")).unwrap_or_default();
    let branch = match branch.len() {
        3 => format!("00{branch}"),
        _ => branch.to_owned(),
    };
    if format!("{tin}{branch}") != ticket.tin {
        return Err("the TIN in the return is not the ticket's TIN".into());
    }

    let year = first(&fields, |name| name.contains("year") && !name.contains("birth"))
        .filter(|year| year.len() == 4 && year.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or("the return has no year")?;
    if !ticket.period.contains(year) {
        return Err(format!("the return is for {year}, not the ticket's period {}", ticket.period));
    }
    let ticked_quarter = fields.iter().find_map(|field| {
        let name = field.name.to_ascii_lowercase();
        let digit = name.strip_prefix("datequarter_").or_else(|| name.strip_prefix("optquarter"))?;
        (field.value == "true" && digit.len() == 1).then(|| digit.to_owned())
    });
    let period_quarter = ticket.period.rsplit_once('Q').map(|(_, quarter)| quarter.to_owned());
    if let (Some(ticked), Some(named)) = (ticked_quarter, period_quarter)
        && ticked != named
    {
        return Err(format!("the return is for quarter {ticked}, not the ticket's period {}", ticket.period));
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
