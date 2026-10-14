use std::path::Path;

use fold_core::projects::find_root;

#[cfg(test)]
fn root<'a>(cwd: &str, roots: &[&'a str]) -> Option<&'a str> {
    find_root(Path::new(cwd), roots.iter().map(|root| Path::new(*root)))
        .map(|path| path.to_str().unwrap())
}

#[test]
fn the_root_itself_and_any_directory_below_it_resolve_to_it() {
    let roots = ["/src/app"];
    assert_eq!(root("/src/app", &roots), Some("/src/app"));
    assert_eq!(root("/src/app/ws/t-12", &roots), Some("/src/app"));
    assert_eq!(root("/src/app/ws/t-12/web/src", &roots), Some("/src/app"));
}

#[test]
fn the_nearest_registered_root_wins_whatever_the_order() {
    let roots = ["/src", "/src/app/vendor/lib", "/src/app"];
    assert_eq!(
        root("/src/app/vendor/lib/x", &roots),
        Some("/src/app/vendor/lib")
    );
    assert_eq!(root("/src/app/vendor", &roots), Some("/src/app"));
    assert_eq!(root("/src/other", &roots), Some("/src"));
}

#[test]
fn siblings_with_a_shared_prefix_are_not_inside_the_root() {
    let roots = ["/src/app"];
    assert_eq!(root("/src/app-2", &roots), None);
    assert_eq!(root("/src/app-2/ws/t-1", &roots), None);
    assert_eq!(root("/src/appendix", &roots), None);
    assert_eq!(root("/src", &roots), None);
}

#[test]
fn paths_that_are_not_canonical_match_nothing() {
    let roots = ["/src/app", "relative/app"];
    assert_eq!(root("src/app", &roots), None);
    assert_eq!(root("/src/app/../../etc", &roots), None);
    assert_eq!(root("./src/app/ws", &roots), None);
    assert_eq!(root("relative/app/ws", &roots), None);
    assert_eq!(root("/src/app/ws", &[]), None);
}
