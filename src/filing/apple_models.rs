//! The names Apple sells its devices under, by the model code an app reads
//! on the device (`utsname.machine` on iPhone and iPad, `hw.model` on a
//! Mac), for the Devices lists.
//!
//! The table is to be filled from Apple's own published identifier lists
//! (the "Identify your iPhone / iPad / Mac model" support pages), and an
//! entry is added only from those lists: a wrong name is worse than none.
//! A code not listed shows as the platform's word with the code
//! ("iPhone (iPhone18,1)"); Android and desktop models show as the app
//! sent them.

use std::borrow::Cow;

/// (model code, the name it is sold under).
const MODELS: &[(&str, &str)] = &[
    // iPhone
    ("iPhone12,1", "iPhone 11"),
    ("iPhone12,3", "iPhone 11 Pro"),
    ("iPhone12,5", "iPhone 11 Pro Max"),
    ("iPhone12,8", "iPhone SE (2nd generation)"),
    ("iPhone13,1", "iPhone 12 mini"),
    ("iPhone13,2", "iPhone 12"),
    ("iPhone13,3", "iPhone 12 Pro"),
    ("iPhone13,4", "iPhone 12 Pro Max"),
    ("iPhone14,4", "iPhone 13 mini"),
    ("iPhone14,5", "iPhone 13"),
    ("iPhone14,2", "iPhone 13 Pro"),
    ("iPhone14,3", "iPhone 13 Pro Max"),
    ("iPhone14,6", "iPhone SE (3rd generation)"),
    ("iPhone14,7", "iPhone 14"),
    ("iPhone14,8", "iPhone 14 Plus"),
    ("iPhone15,2", "iPhone 14 Pro"),
    ("iPhone15,3", "iPhone 14 Pro Max"),
    ("iPhone15,4", "iPhone 15"),
    ("iPhone15,5", "iPhone 15 Plus"),
    ("iPhone16,1", "iPhone 15 Pro"),
    ("iPhone16,2", "iPhone 15 Pro Max"),
    ("iPhone17,3", "iPhone 16"),
    ("iPhone17,4", "iPhone 16 Plus"),
    ("iPhone17,1", "iPhone 16 Pro"),
    ("iPhone17,2", "iPhone 16 Pro Max"),
    // iPad
    ("iPad7,11", "iPad (7th generation)"),
    ("iPad7,12", "iPad (7th generation)"),
    ("iPad11,6", "iPad (8th generation)"),
    ("iPad11,7", "iPad (8th generation)"),
    ("iPad12,1", "iPad (9th generation)"),
    ("iPad12,2", "iPad (9th generation)"),
    ("iPad13,18", "iPad (10th generation)"),
    ("iPad13,19", "iPad (10th generation)"),
    ("iPad11,1", "iPad mini (5th generation)"),
    ("iPad11,2", "iPad mini (5th generation)"),
    ("iPad14,1", "iPad mini (6th generation)"),
    ("iPad14,2", "iPad mini (6th generation)"),
    ("iPad11,3", "iPad Air (3rd generation)"),
    ("iPad11,4", "iPad Air (3rd generation)"),
    ("iPad13,1", "iPad Air (4th generation)"),
    ("iPad13,2", "iPad Air (4th generation)"),
    ("iPad13,16", "iPad Air (5th generation)"),
    ("iPad13,17", "iPad Air (5th generation)"),
    ("iPad14,8", "iPad Air 11-inch (M2)"),
    ("iPad14,9", "iPad Air 11-inch (M2)"),
    ("iPad14,10", "iPad Air 13-inch (M2)"),
    ("iPad14,11", "iPad Air 13-inch (M2)"),
    ("iPad8,9", "iPad Pro 11-inch (2nd generation)"),
    ("iPad8,10", "iPad Pro 11-inch (2nd generation)"),
    ("iPad8,11", "iPad Pro 12.9-inch (4th generation)"),
    ("iPad8,12", "iPad Pro 12.9-inch (4th generation)"),
    ("iPad13,4", "iPad Pro 11-inch (3rd generation)"),
    ("iPad13,5", "iPad Pro 11-inch (3rd generation)"),
    ("iPad13,6", "iPad Pro 11-inch (3rd generation)"),
    ("iPad13,7", "iPad Pro 11-inch (3rd generation)"),
    ("iPad13,8", "iPad Pro 12.9-inch (5th generation)"),
    ("iPad13,9", "iPad Pro 12.9-inch (5th generation)"),
    ("iPad13,10", "iPad Pro 12.9-inch (5th generation)"),
    ("iPad13,11", "iPad Pro 12.9-inch (5th generation)"),
    ("iPad14,3", "iPad Pro 11-inch (4th generation)"),
    ("iPad14,4", "iPad Pro 11-inch (4th generation)"),
    ("iPad14,5", "iPad Pro 12.9-inch (6th generation)"),
    ("iPad14,6", "iPad Pro 12.9-inch (6th generation)"),
    ("iPad16,3", "iPad Pro 11-inch (M4)"),
    ("iPad16,4", "iPad Pro 11-inch (M4)"),
    ("iPad16,5", "iPad Pro 13-inch (M4)"),
    ("iPad16,6", "iPad Pro 13-inch (M4)"),
    // Mac (Apple silicon)
    ("MacBookAir10,1", "MacBook Air (M1, 2020)"),
    ("MacBookPro17,1", "MacBook Pro 13-inch (M1, 2020)"),
    ("Macmini9,1", "Mac mini (M1, 2020)"),
    ("iMac21,1", "iMac 24-inch (M1, 2021)"),
    ("iMac21,2", "iMac 24-inch (M1, 2021)"),
    ("MacBookPro18,3", "MacBook Pro 14-inch (2021)"),
    ("MacBookPro18,4", "MacBook Pro 14-inch (2021)"),
    ("MacBookPro18,1", "MacBook Pro 16-inch (2021)"),
    ("MacBookPro18,2", "MacBook Pro 16-inch (2021)"),
    ("Mac13,1", "Mac Studio (2022)"),
    ("Mac13,2", "Mac Studio (2022)"),
    ("Mac14,2", "MacBook Air 13-inch (M2, 2022)"),
    ("Mac14,7", "MacBook Pro 13-inch (M2, 2022)"),
    ("Mac14,3", "Mac mini (M2, 2023)"),
    ("Mac14,12", "Mac mini (M2 Pro, 2023)"),
    ("Mac14,5", "MacBook Pro 14-inch (2023)"),
    ("Mac14,9", "MacBook Pro 14-inch (2023)"),
    ("Mac14,6", "MacBook Pro 16-inch (2023)"),
    ("Mac14,10", "MacBook Pro 16-inch (2023)"),
    ("Mac14,15", "MacBook Air 15-inch (M2, 2023)"),
    ("Mac14,13", "Mac Studio (2023)"),
    ("Mac14,14", "Mac Studio (2023)"),
    ("Mac14,8", "Mac Pro (2023)"),
    ("Mac15,3", "MacBook Pro 14-inch (M3, 2023)"),
    ("Mac15,4", "iMac 24-inch (M3, 2023)"),
    ("Mac15,5", "iMac 24-inch (M3, 2023)"),
    ("Mac15,12", "MacBook Air 13-inch (M3, 2024)"),
    ("Mac15,13", "MacBook Air 15-inch (M3, 2024)"),
];

/// The name `model` is sold under; for an Apple code not listed, the
/// platform's word with the code ("iPad (iPad15,3)"); anything else as it
/// is.
pub fn name(model: &str) -> Cow<'_, str> {
    if let Some((_, name)) = MODELS.iter().find(|(code, _)| *code == model) {
        return Cow::Borrowed(name);
    }
    match apple_word(model) {
        Some(word) => Cow::Owned(format!("{word} ({model})")),
        None => Cow::Borrowed(model),
    }
}

/// The platform's word for an Apple model code (letters, then
/// `<major>,<minor>`): iPhone, iPad, or Mac for every other one.
fn apple_word(model: &str) -> Option<&'static str> {
    let letters = model.find(|ch: char| ch.is_ascii_digit())?;
    let (prefix, numbers) = model.split_at(letters);
    let (major, minor) = numbers.split_once(',')?;
    let numeric = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    if !(numeric(major) && numeric(minor)) {
        return None;
    }
    match prefix {
        "iPhone" => Some("iPhone"),
        "iPad" => Some("iPad"),
        _ if prefix.starts_with("Mac") || prefix == "iMac" => Some("Mac"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apple_codes_get_their_names_and_others_stay_as_sent() {
        assert_eq!(name("iPhone16,1"), "iPhone 15 Pro");
        assert_eq!(name("iPad13,18"), "iPad (10th generation)");
        assert_eq!(name("Mac14,2"), "MacBook Air 13-inch (M2, 2022)");
        // Not listed (2025 models among them): the word and the code.
        assert_eq!(name("iPhone18,1"), "iPhone (iPhone18,1)");
        assert_eq!(name("iPad15,3"), "iPad (iPad15,3)");
        assert_eq!(name("Mac16,12"), "Mac (Mac16,12)");
        assert_eq!(name("MacBookPro99,1"), "Mac (MacBookPro99,1)");
        assert_eq!(name("iMac99,1"), "Mac (iMac99,1)");
        // Not Apple's: as sent.
        assert_eq!(name("Pixel 8"), "Pixel 8");
        assert_eq!(name("SM-S921B"), "SM-S921B");
        assert_eq!(name("iPhone"), "iPhone");
    }

    #[test]
    fn every_code_is_listed_once() {
        let mut codes: Vec<&str> = MODELS.iter().map(|(code, _)| *code).collect();
        codes.sort_unstable();
        let before = codes.len();
        codes.dedup();
        assert_eq!(codes.len(), before);
    }
}
