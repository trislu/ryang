mod test_utils;

use test_utils::{find, has_code, library, load_file, load_files};
use yrepo::{DeviationOp, DiagnosticCode};

/// `deviation … deviate not-supported` removes its target from the effective
/// tree; the deviation is recorded.
#[test]
fn test_deviation_not_supported_removes_target() {
    let repo = load_file("deviation-test.yang");
    let (lib, diags) = library(&repo);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

    let m = lib.module("deviation-test").expect("module deviation-test");

    // removed from the effective tree
    assert!(find(m, &["obsolete-leaf"]).is_none());
    assert!(find(m, &["kept-leaf"]).is_some());

    // recorded
    assert_eq!(m.deviations().len(), 1);
    let dev = &m.deviations()[0];
    assert_eq!(dev.target, "/dt:obsolete-leaf");
    assert_eq!(dev.op, DeviationOp::NotSupported);
    let node = dev
        .target_node
        .map(|id| m.node(id).expect("target node"))
        .unwrap();
    assert!(node.is_removed());
}

/// A deviation may target a node in a *different* (base) module; its absolute
/// schema-nodeid must resolve against that module's effective tree (regression:
/// `apply_deviations` used to scan only the deviating module's own tree, so
/// every cross-module deviation reported `DeviationTargetNotFound`
#[test]
fn test_deviation_cross_module_target_resolves() {
    let orders: &[&[&str]] = &[
        &["dev-other.yang", "dev-base.yang"],
        &["dev-base.yang", "dev-other.yang"],
    ];
    for order in orders {
        let repo = load_files(order);
        let (lib, diags) = library(&repo);
        assert!(
            !has_code(&diags, DiagnosticCode::DeviationTargetNotFound),
            "order {order:?} reported DeviationTargetNotFound: {diags:?}"
        );

        let base = lib.module("dev-base").expect("module dev-base");
        // not-supported removed the leaf from the base module's effective tree
        assert!(find(base, &["system", "obsolete"]).is_none());
        assert!(find(base, &["system", "name"]).is_some());

        // the deviations are recorded on the deviating module
        let dev = lib.module("dev-other").expect("module dev-other");
        assert_eq!(dev.deviations().len(), 2);
        assert_eq!(dev.deviations()[0].target, "/db:system/db:obsolete");

        // goto/hover resolution of a deviation argument sees the node
        let target = lib
            .resolve_abs_schema_node_id("dev-other", "/db:system/db:name")
            .expect("resolved deviation target");
        assert_eq!(target.name(), "name");
    }
}
