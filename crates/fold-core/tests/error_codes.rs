use std::collections::HashSet;

use fold_core::codes;

const GOLDEN: &str = include_str!("error_codes.golden");

#[test]
fn golden_codes_are_still_shipped() {
    for line in GOLDEN.lines().filter(|line| !line.is_empty()) {
        assert!(
            codes::ALL.contains(&line),
            "shipped code {line} was removed or renamed"
        );
    }
    let golden: HashSet<&str> = GOLDEN.lines().filter(|line| !line.is_empty()).collect();
    for code in codes::ALL {
        assert!(
            golden.contains(code),
            "code {code} is missing from error_codes.golden"
        );
    }
}

#[test]
fn codes_are_unique_and_snake_case() {
    let unique: HashSet<_> = codes::ALL.iter().collect();
    assert_eq!(unique.len(), codes::ALL.len(), "duplicate code");
    for code in codes::ALL {
        assert!(!code.is_empty());
        assert!(
            code.split('_').all(|part| !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())),
            "{code} is not snake_case"
        );
    }
}
