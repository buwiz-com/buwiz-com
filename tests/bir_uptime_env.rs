//! `BIR_UPTIME` read from the environment, alone in its own test binary:
//! it sets the variable, so nothing else may run beside it.

use buwiz_cloud::uptime::Level;

#[test]
fn bir_uptime_is_read_in_any_case_and_a_bad_value_records() {
    for (raw, level) in [
        (None, Level::Record),
        (Some(""), Level::Record),
        (Some("off"), Level::Off),
        (Some("record"), Level::Record),
        (Some("public"), Level::Public),
        (Some("enforce"), Level::Enforce),
        (Some("  Enforce "), Level::Enforce),
        (Some("PUBLIC"), Level::Public),
        (Some("Off\n"), Level::Off),
        (Some("on"), Level::Record),
        (Some("enforced"), Level::Record),
        (Some("1"), Level::Record),
    ] {
        // SAFETY: this binary runs this one test, on one thread, and starts
        // no other thread that reads the environment.
        unsafe {
            match raw {
                Some(raw) => std::env::set_var("BIR_UPTIME", raw),
                None => std::env::remove_var("BIR_UPTIME"),
            }
        }
        assert_eq!(Level::from_env(), level, "{raw:?}");
    }
}
