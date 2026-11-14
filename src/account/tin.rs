//! TIN root + branch code: the key BIR files returns under.
//!
//! A TIN is valid when its root has 9 digits, not all zeros, and its branch
//! code 5 (or the old 3, or none for head office). A root of all zeros is a
//! test TIN: staff put one on a test account only, and it never sends
//! ([`crate::filing::gate`]). The `_any` readers take one; the others
//! refuse it with [`Notice::TinZero`], as people's forms always did.

use std::fmt::{self, Display, Formatter};

use crate::account::notice::Notice;

/// The root of every test TIN: `000-000-000`, at any branch.
pub const TEST_ROOT: &str = "000000000";

/// A BIR registration: 9-digit TIN root and 5-digit branch code
/// (head office `00000`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tin {
    root: String,
    branch: String,
}

impl Tin {
    /// Accepts dashes and spaces. A 3-digit branch is the old form and gets
    /// `00` in front; an empty branch means head office. A test TIN is
    /// refused ([`Notice::TinZero`]).
    pub fn parse(root: &str, branch: &str) -> Result<Self, Notice> {
        // Before the branch, as it always was: a zero root with a bad
        // branch says the root is the problem.
        if digits(root) == TEST_ROOT {
            return Err(Notice::TinZero);
        }
        Self::parse_any(root, branch)
    }

    /// [`Tin::parse`], a test TIN too: for stored rows, the gate (which
    /// refuses test TINs itself) and the staff form that adds one.
    pub fn parse_any(root: &str, branch: &str) -> Result<Self, Notice> {
        let root = digits(root);
        if root.len() != 9 {
            return Err(Notice::TinDigits);
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
        Self::parse_combined_any(raw).and_then(Self::not_test)
    }

    /// [`Tin::parse_combined`], a test TIN too.
    pub fn parse_combined_any(raw: &str) -> Result<Self, Notice> {
        let all = digits(raw);
        match all.len() {
            9 => Self::parse_any(&all, ""),
            12 | 14 => Self::parse_any(&all[..9], &all[9..]),
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

    /// [`Tin::parse_fields`], a test TIN too.
    pub fn parse_fields_any(tin: &str, branch: &str) -> Result<Self, Notice> {
        if digits(tin).len() > 9 || digits(branch).is_empty() {
            Self::parse_combined_any(tin)
        } else {
            Self::parse_any(tin, branch)
        }
    }

    fn not_test(self) -> Result<Self, Notice> {
        if self.is_test() { Err(Notice::TinZero) } else { Ok(self) }
    }

    /// A made-up TIN for testing: its root is all zeros. BIR has no such
    /// TIN, and nothing is ever sent for it.
    pub fn is_test(&self) -> bool {
        self.root == TEST_ROOT
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
    fn test_tins_are_read_only_where_asked_for() {
        for (raw, branch) in [("000-000-000", ""), ("000000000", "00001"), ("000-000-000-00000", "")] {
            let tin = Tin::parse_fields_any(raw, branch).unwrap();
            assert!(tin.is_test(), "{raw}");
            assert_eq!(Tin::parse_fields(raw, branch), Err(Notice::TinZero), "{raw}");
        }
        assert_eq!(Tin::parse_combined_any("000-000-000-00002").unwrap().to_string(), "000-000-000-00002");
        assert_eq!(Tin::parse_combined("000-000-000-00002"), Err(Notice::TinZero));
        assert_eq!(Tin::parse_any("000000000", "").unwrap().masked(), "***-***-000-00000");
        // The shape is still checked, and a real TIN is no test TIN.
        assert_eq!(Tin::parse_any("00000000", ""), Err(Notice::TinDigits));
        assert_eq!(Tin::parse_any("000000000", "12"), Err(Notice::TinBranch));
        // The strict readers name the zero root first, before a bad branch,
        // as people's forms always did.
        assert_eq!(Tin::parse("000000000", "12"), Err(Notice::TinZero));
        assert_eq!(Tin::parse_fields("000-000-000", "12"), Err(Notice::TinZero));
        assert_eq!(Tin::parse("00000000", "12"), Err(Notice::TinDigits));
        assert!(!Tin::parse_any("000123453", "").unwrap().is_test());
    }

    #[test]
    fn masks_all_but_the_last_three_root_digits() {
        assert_eq!(Tin::parse("123456789", "").unwrap().masked(), "***-***-789-00000");
    }
}
