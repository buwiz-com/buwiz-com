//! TIN root + branch code: the key BIR files returns under.

use std::fmt::{self, Display, Formatter};

use crate::notice::Notice;

/// A BIR registration: 9-digit TIN root and 5-digit branch code
/// (head office `00000`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tin {
    root: String,
    branch: String,
}

impl Tin {
    /// Accepts dashes and spaces. A 3-digit branch is the old form and gets
    /// `00` in front; an empty branch means head office.
    pub fn parse(root: &str, branch: &str) -> Result<Self, Notice> {
        let root = digits(root);
        if root.len() != 9 {
            return Err(Notice::TinDigits);
        }
        if root.bytes().all(|digit| digit == b'0') {
            return Err(Notice::TinZero);
        }
        let branch = match digits(branch) {
            branch if branch.is_empty() => "00000".to_owned(),
            branch if branch.len() == 5 => branch,
            branch if branch.len() == 3 => format!("00{branch}"),
            _ => return Err(Notice::TinBranch),
        };
        Ok(Self { root, branch })
    }

    /// Both parts in one field, as printed on Form 2303: `123-456-789-00000`
    /// or just the 9-digit root.
    pub fn parse_combined(raw: &str) -> Result<Self, Notice> {
        let all = digits(raw);
        match all.len() {
            9 => Self::parse(&all, ""),
            12 | 14 => Self::parse(&all[..9], &all[9..]),
            _ => Err(Notice::TinDigits),
        }
    }

    /// The TIN and branch code fields of a form. The TIN field may carry the
    /// branch too, as printed on Form 2303; then the branch field is ignored.
    pub fn parse_fields(tin: &str, branch: &str) -> Result<Self, Notice> {
        if digits(tin).len() > 9 || digits(branch).is_empty() {
            Self::parse_combined(tin)
        } else {
            Self::parse(tin, branch)
        }
    }

    pub fn root(&self) -> &str {
        &self.root
    }

    pub fn branch(&self) -> &str {
        &self.branch
    }

    /// Shows only the last three digits of the root: `***-***-789-00000`.
    pub fn masked(&self) -> String {
        format!("***-***-{}-{}", &self.root[6..], self.branch)
    }
}

impl Display for Tin {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let root = &self.root;
        write!(f, "{}-{}-{}-{}", &root[..3], &root[3..6], &root[6..], self.branch)
    }
}

fn digits(raw: &str) -> String {
    raw.chars().filter(char::is_ascii_digit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_head_office_and_old_three_digit_branches() {
        let tin = Tin::parse("123-456-789", "").unwrap();
        assert_eq!(tin.to_string(), "123-456-789-00000");
        assert_eq!(Tin::parse("123456789", "001").unwrap().branch(), "00001");
        assert_eq!(Tin::parse("123456789", "00002").unwrap().branch(), "00002");
    }

    #[test]
    fn parses_the_combined_form() {
        assert_eq!(Tin::parse_combined("123-456-789-00003").unwrap().branch(), "00003");
        assert_eq!(Tin::parse_combined("123-456-789-000").unwrap().branch(), "00000");
        assert_eq!(Tin::parse_combined("123 456 789").unwrap().branch(), "00000");
        assert!(Tin::parse_combined("123-456-78").is_err());
    }

    #[test]
    fn parses_separate_tin_and_branch_fields() {
        assert_eq!(Tin::parse_fields("987-654-321", "00002").unwrap().to_string(), "987-654-321-00002");
        assert_eq!(Tin::parse_fields("987654321", "").unwrap().branch(), "00000");
        assert_eq!(Tin::parse_fields("987-654-321-00004", "00000").unwrap().branch(), "00004");
        assert!(Tin::parse_fields("987-654-321", "12").is_err());
    }

    #[test]
    fn rejects_bad_lengths_and_all_zeros() {
        assert!(Tin::parse("12345678", "").is_err());
        assert!(Tin::parse("000000000", "").is_err());
        assert!(Tin::parse("123456789", "12").is_err());
    }

    #[test]
    fn masks_all_but_the_last_three_root_digits() {
        assert_eq!(Tin::parse("123456789", "").unwrap().masked(), "***-***-789-00000");
    }
}
