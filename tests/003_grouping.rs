mod test_utils;

use test_utils::{child_names, find, library, load_file};

/// The effective tree must *expand* groupings at their `uses` sites ([D9]),
/// rather than exposing `uses` as a pseudo-node.
#[test]
fn test_groupings_expand_into_effective_tree() {
    let repo = load_file("grouping-test.yang");
    let (lib, diags) = library(&repo);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

    let m = lib.module("grouping-test").expect("module grouping-test");

    let locations = find(m, &["locations"]).expect("container locations");
    assert_eq!(child_names(m, locations), ["street", "city", "zip"]);

    let connection = find(m, &["connection"]).expect("container connection");
    assert_eq!(child_names(m, connection), ["host", "port"]);

    // the definitions are still recorded as grouping symbols
    let names: Vec<&str> = m.groupings().iter().map(|g| g.name.as_str()).collect();
    assert_eq!(names, ["address", "endpoint"]);
    assert!(lib.search_grouping("grouping-test", "address").is_some());
}
