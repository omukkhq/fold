pub mod notify;

use std::collections::BTreeSet;
use std::path::{Component, Path};

/// A lowercase name of ASCII letters, digits and single hyphens. Every other run of characters becomes one `-`; the result is never empty.
pub fn slugify(name: &str) -> String {
    let mut slug = String::new();
    let mut pending_hyphen = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_hyphen && !slug.is_empty() {
                slug.push('-');
            }
            pending_hyphen = false;
            slug.push(ch.to_ascii_lowercase());
        } else {
            pending_hyphen = true;
        }
    }
    if slug.is_empty() {
        "project".to_owned()
    } else {
        slug
    }
}

/// `base`, or `base-2`, `base-3` and so on: the first one not in `existing`.
pub fn choose_slug(base: &str, existing: &BTreeSet<String>) -> String {
    if !existing.contains(base) {
        return base.to_owned();
    }
    (2u64..)
        .map(|n| format!("{base}-{n}"))
        .find(|candidate| !existing.contains(candidate))
        .unwrap_or_else(|| base.to_owned())
}

/// The registered root that holds `cwd`: the nearest one when roots nest. Roots match whole path components, so `/src/app-2` is not inside `/src/app`. A `cwd` that is relative or holds `..` matches nothing, because `/src/app/../etc` starts with `/src/app` component by component but is not inside it.
pub fn find_root<'a, I>(cwd: &Path, roots: I) -> Option<&'a Path>
where
    I: IntoIterator<Item = &'a Path>,
{
    if !is_canonical(cwd) {
        return None;
    }
    roots
        .into_iter()
        .filter(|root| is_canonical(root) && cwd.starts_with(root))
        .max_by_key(|root| root.components().count())
}

fn is_canonical(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn slugify_table() {
        for (name, slug) in [
            ("My App", "my-app"),
            ("  spaced   out  ", "spaced-out"),
            ("a_b.c/d", "a-b-c-d"),
            ("Ünïcode Café", "n-code-caf"),
            ("日本語", "project"),
            ("", "project"),
            ("---", "project"),
            ("v2.0", "v2-0"),
            ("Already-Slug", "already-slug"),
        ] {
            assert_eq!(slugify(name), slug, "{name:?}");
        }
    }

    proptest! {
        #[test]
        fn slugify_output_uses_only_lowercase_digits_and_hyphens(name in any::<String>()) {
            let slug = slugify(&name);
            prop_assert!(!slug.is_empty());
            prop_assert!(slug.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
            prop_assert!(!slug.starts_with('-') && !slug.ends_with('-'));
            prop_assert!(!slug.contains("--"));
        }
    }

    #[test]
    fn choose_slug_appends_the_next_free_suffix() {
        let none = BTreeSet::new();
        assert_eq!(choose_slug("app", &none), "app");
        let taken: BTreeSet<String> = ["app", "app-2"].map(String::from).into();
        assert_eq!(choose_slug("app", &taken), "app-3");
        let gap: BTreeSet<String> = ["app", "app-3"].map(String::from).into();
        assert_eq!(choose_slug("app", &gap), "app-2");
        assert_eq!(choose_slug("other", &taken), "other");
    }
}
