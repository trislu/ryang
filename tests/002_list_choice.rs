mod test_utils;

use test_utils::{child_names, find, library, load_file};
use yrepo::NodeKind;

#[test]
fn test_list_and_keys() {
    let repo = load_file("list-choice.yang");
    let (lib, diags) = library(&repo);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

    let m = lib.module("list-choice").expect("module list-choice");

    let server = find(m, &["server"]).expect("list server");
    assert_eq!(server.kind(), NodeKind::List);
    let keys: Vec<&str> = server.keys().iter().map(|s| s.as_str()).collect();
    assert_eq!(keys, ["name", "port"]);

    let name = find(m, &["server", "name"]).expect("key leaf name");
    assert_eq!(name.kind(), NodeKind::Leaf);
    assert!(name.is_key());
    let enabled = find(m, &["server", "enabled"]).expect("non-key leaf");
    assert!(!enabled.is_key());
}

#[test]
fn test_choice_cases() {
    let repo = load_file("list-choice.yang");
    let (lib, _diags) = library(&repo);
    let m = lib.module("list-choice").unwrap();

    let transport = find(m, &["transport"]).expect("choice transport");
    assert_eq!(transport.kind(), NodeKind::Choice);
    assert_eq!(child_names(m, transport), ["udp", "tcp"]);

    let udp = find(m, &["transport", "udp"]).expect("case udp");
    assert_eq!(udp.kind(), NodeKind::Case);

    let cfg = find(m, &["transport", "udp", "udp-config"]).expect("container under case");
    assert_eq!(cfg.kind(), NodeKind::Container);
    assert_eq!(child_names(m, cfg), ["port"]);
}
