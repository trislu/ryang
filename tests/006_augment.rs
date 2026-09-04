mod test_utils;

use test_utils::{child, child_names, find, has_code, library, load_file, load_files};
use yrepo::NodeKind;

/// Module-level `augment` with an absolute schema-nodeid is applied onto the
/// effective tree ([D9]).
#[test]
fn test_augment_applied() {
    let repo = load_file("augment-test.yang");
    let (lib, diags) = library(&repo);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

    let m = lib.module("augment-test").expect("module augment-test");
    let target = find(m, &["target"]).expect("container target");
    assert_eq!(child_names(m, target), ["original", "added"]);

    let added = find(m, &["target", "added"]).expect("augment-added leaf");
    assert_eq!(added.kind(), NodeKind::Leaf);

    // recorded on the source module
    assert_eq!(m.augments().len(), 1);
    let aug = &m.augments()[0];
    assert_eq!(aug.target, "/at:target");
    assert_eq!(aug.source_module.as_ref(), "augment-test");
}

/// `Library::resolve_abs_schema_node_id` backs goto/hover on augment /
/// refine / deviation arguments (D9): it walks the effective tree.
#[test]
fn test_resolve_abs_schema_node_id() {
    let repo = load_file("augment-test.yang");
    let (lib, _diags) = library(&repo);

    let node = lib
        .resolve_abs_schema_node_id("augment-test", "/at:target")
        .expect("resolved /at:target");
    assert_eq!(node.name(), "target");
    assert_eq!(node.kind(), NodeKind::Container);

    // resolving a nested child
    let added = lib
        .resolve_abs_schema_node_id("augment-test", "/at:target/at:added")
        .expect("resolved nested");
    assert_eq!(added.name(), "added");

    // unknown paths yield None
    assert!(
        lib.resolve_abs_schema_node_id("augment-test", "/at:nope")
            .is_none()
    );
}

/// Augments may target nodes that *another* augment installs. Resolution must
/// not depend on the order documents were upserted (regression: applying
/// augments in document order spuriously reported `AugmentTargetNotFound` for
/// `aug-chain-c`, whose target `/b:root/a:extra` is created by `aug-chain-a`'s
/// augment — the real-world `ietf-ip-mounted`/`ietf-interfaces-mounted` case).
#[test]
fn test_augment_order_independent() {
    let orders: &[&[&str]] = &[
        // c (the dependent augment) first.
        &["aug-chain-c.yang", "aug-chain-b.yang", "aug-chain-a.yang"],
        // a (the augment that installs the intermediate node) first.
        &["aug-chain-a.yang", "aug-chain-b.yang", "aug-chain-c.yang"],
    ];
    for order in orders {
        let repo = load_files(order);
        let (lib, diags) = library(&repo);
        assert!(
            !has_code(&diags, yrepo::DiagnosticCode::AugmentTargetNotFound),
            "order {order:?} reported AugmentTargetNotFound: {diags:?}"
        );

        // The effective tree of module b has root -> extra (from a) and the
        // extra gains `deep` (from c), in both orders.
        let mb = lib.module("aug-chain-b").expect("module aug-chain-b");
        let extra = lib
            .resolve_abs_schema_node_id("aug-chain-c", "/b:root/a:extra")
            .expect("resolved chained node");
        assert!(child(mb, extra, "deep").is_some(), "c's deep child missing");
        assert!(child(mb, extra, "e").is_some(), "a's leaf e missing");
    }
}

/// An `rpc`/`action` always has an `input` and an `output` schema node, even
/// when the module omits one or both (RFC 7950 §7.14/§7.15). Augments that
/// target an (implicit) `input` must resolve and apply (regression: the
/// `ietf-ipv4/ipv6-unicast-routing` augments of ietf-routing's `active-route`
/// action reported `AugmentTargetNotFound` because `input`/`output` nodes had
/// empty names and missing blocks were never synthesized).
#[test]
fn test_augment_rpc_action_input_output() {
    let orders: &[&[&str]] = &[
        &["rpc-io-aug.yang", "rpc-io.yang"],
        &["rpc-io.yang", "rpc-io-aug.yang"],
    ];
    for order in orders {
        let repo = load_files(order);
        let (lib, diags) = library(&repo);
        assert!(
            !has_code(&diags, yrepo::DiagnosticCode::AugmentTargetNotFound),
            "order {order:?} reported AugmentTargetNotFound: {diags:?}"
        );
        let m = lib.module("rpc-io").expect("module rpc-io");

        // The `reset` action declares only `output`; its `input` is implicit
        // yet still a named, augmentable schema node.
        let input = lib
            .resolve_abs_schema_node_id("rpc-io-aug", "/r:sys/r:item/r:reset/input")
            .expect("resolved action input");
        assert_eq!(input.name(), "input");
        assert_eq!(input.kind(), NodeKind::Input);
        assert!(
            child(m, input, "reason").is_some(),
            "augment into input missing"
        );

        let output = lib
            .resolve_abs_schema_node_id("rpc-io-aug", "/r:sys/r:item/r:reset/output")
            .expect("resolved action output");
        assert_eq!(output.kind(), NodeKind::Output);
        assert!(
            child(m, output, "ok").is_some(),
            "declared output leaf ok missing"
        );
        assert!(
            child(m, output, "finished-at").is_some(),
            "augment into output missing"
        );

        // An RPC declaring neither input nor output still exposes both.
        let op_input = lib
            .resolve_abs_schema_node_id("rpc-io-aug", "/r:op/input")
            .expect("resolved rpc input");
        assert_eq!(op_input.kind(), NodeKind::Input);
        assert!(
            child(m, op_input, "arg1").is_some(),
            "augment into rpc input missing"
        );
    }
}
