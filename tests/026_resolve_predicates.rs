//! `Library::resolve_abs_schema_node_id` drops predicates SEGMENT-WISE, so a
//! leafref-style path `/a:b/c:d[k='v']/e:f` walks a:b -> c:d -> e:f and lands
//! on the final leaf, not on the predicate-bearing list.

use std::sync::Arc;

use yrepo::{Library, Repository};

#[test]
fn predicate_path_resolves_to_final_node() {
    let mut repo = Repository::new();
    repo.upsert(
        "/base.yang",
        "module base { namespace urn:base; prefix b;\n\
         container top { list item { key name; leaf name { type string; }\n\
         leaf port { type uint16; } } }\n\
         }",
    );
    repo.upsert(
        "/app.yang",
        "module app { namespace urn:app; prefix a;\n\
         import base { prefix b; }\n\
         leaf r { type leafref { path \"/b:top/b:item[b:name='x']/b:port\"; } }\n\
         }",
    );
    let out = repo.compile();
    let lib: Arc<Library> = out.library.expect("lib");
    let node = lib
        .resolve_abs_schema_node_id("app", "/b:top/b:item[b:name='x']/b:port")
        .expect("resolved");
    assert_eq!(node.name(), "port");
}
