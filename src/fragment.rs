//! Grouping instantiation fragments (PHASE ②/step-3 groundwork).
//!
//! The planned memoized expansion (see `YREPO_PHASES.md` ②/step-3) replays a
//! grouping's instantiated node run instead of re-walking the grouping's
//! statements at every `uses` site. A fragment is captured as a contiguous
//! arena run once; each later instantiation deep-copies it, remapping internal
//! parent/children links onto fresh ids, stamping the site's `instance_module`
//! (namespace ownership is per-site, RFC 7950 §7.13) and handing the roots
//! back so the caller can stamp `used_from` per site.
//!
//! This module implements and unit-tests that capture/instantiate pair in
//! isolation; wiring into `expand_uses` is the next slice (S3b) and is
//! audited for net-zero behavior before it replaces the recursion path.
#![allow(dead_code)] // consumed by the Phase ② step-3 driver slice that follows

use std::sync::Arc;

use crate::schema::{NodeId, SchemaNode};

/// A captured instantiation run: `nodes[start..start+nodes.len()]` shallow
/// copies (parent/children still hold their original absolute ids) plus the
/// run-root ids (nodes whose parent lay outside the run).
pub(crate) struct RunTemplate {
    start: NodeId,
    nodes: Vec<SchemaNode>,
    /// Original absolute ids of the run's root nodes (used_from already
    /// cleared: each instantiation stamps its own site).
    roots: Vec<NodeId>,
}

/// Capture `arena[start..end]` as a reusable template. Root nodes (parent
/// outside the run or absent) have their `used_from` cleared so each
/// instantiation can stamp its own site location.
pub(crate) fn snapshot_run(arena: &[SchemaNode], start: NodeId, end: NodeId) -> RunTemplate {
    let nodes: Vec<SchemaNode> = arena[start..end].to_vec();
    let roots: Vec<NodeId> = nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.parent.map(|p| p < start || p >= end).unwrap_or(true))
        .map(|(i, _)| start + i as NodeId)
        .collect();
    let mut tmpl = RunTemplate {
        start,
        nodes,
        roots,
    };
    for &r in tmpl.roots.clone().iter() {
        if let Some(n) = tmpl.nodes.get_mut(r - tmpl.start) {
            n.used_from = None;
        }
    }
    tmpl
}

/// Deep-copy `tmpl` into `arena`, remapping internal links, stamping
/// `instance_module = ns` on every copy, and linking run roots to
/// `ext_parent`. Returns the new ids of the run roots (site-stamp
/// `used_from` on them as the recursion does).
pub(crate) fn instantiate_run(
    arena: &mut Vec<SchemaNode>,
    tmpl: &RunTemplate,
    ext_parent: Option<NodeId>,
    ns: &Arc<str>,
) -> Vec<NodeId> {
    let base = arena.len() as NodeId;
    let within = |id: NodeId| id >= tmpl.start && id < tmpl.start + tmpl.nodes.len();
    let remap = |id: NodeId| id + base - tmpl.start;
    for n in &tmpl.nodes {
        let mut c = n.clone();
        c.instance_module = ns.clone();
        arena.push(c);
    }
    for (rel, n) in arena[base..].iter_mut().enumerate() {
        n.children = n
            .children
            .iter()
            .map(|&c| if within(c) { remap(c) } else { c })
            .collect();
        n.parent = match n.parent {
            Some(p) if within(p) => Some(remap(p)),
            _ => ext_parent,
        };
        let _ = tmpl.start + rel;
    }
    tmpl.roots.iter().map(|&r| remap(r)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::Location;
    use crate::schema::NodeKind;
    use crate::syntax::StatementKind;

    fn node(name: &str) -> SchemaNode {
        SchemaNode {
            kind: NodeKind::Container,
            name: name.to_string(),
            parent: None,
            children: Vec::new(),
            defining: Location {
                url: Arc::from("/t.yang"),
                range: 0..1,
            },
            used_from: None,
            origin_module: Arc::from("m"),
            instance_module: Arc::from("m"),
            config: None,
            mandatory: false,
            presence: None,
            default: None,
            status: None,
            ordered_by: None,
            min_elements: None,
            max_elements: None,
            keys: Vec::new(),
            is_key: false,
            type_name: None,
            facets: Default::default(),
            removed: false,
        }
    }

    /// Snapshot -> instantiate twice into a fresh arena: internal links are
    /// remapped, roots are distinct per instantiation, ns is stamped, and a
    /// pre-set root used_from is cleared for the site to stamp.
    #[test]
    fn instantiate_remaps_links_and_stamps_ns() {
        let mut src = Vec::new();
        let root: NodeId = 0;
        src.push(node("group"));
        let mut kid = node("kid");
        kid.parent = Some(root);
        kid.kind = NodeKind::Leaf;
        let kid_id: NodeId = 1;
        src.push(kid);
        src[root as usize].children.push(kid_id);
        // simulate a first-site stamp on the root (must be cleared).
        src[0].used_from = Some(Location {
            url: Arc::from("/u.yang"),
            range: 5..6,
        });
        src[1].used_from = Some(Location {
            url: Arc::from("/g.yang"),
            range: 9..10,
        });
        let tmpl = snapshot_run(&src, 0, 2);

        let mut dst = Vec::new();
        let ext: NodeId = 7;
        let ns1 = Arc::from("using1");
        let roots1 = instantiate_run(&mut dst, &tmpl, Some(ext), &ns1);
        assert_eq!(roots1.len(), 1);
        assert_eq!(roots1[0], 0);
        assert_eq!(dst[0].name, "group");
        assert_eq!(dst[0].parent, Some(ext));
        assert_eq!(dst[0].instance_module.as_ref(), "using1");
        assert!(dst[0].used_from.is_none(), "root used_from cleared");
        assert_eq!(dst[1].parent, Some(0));
        assert_eq!(dst[1].name, "kid");
        assert_eq!(dst[1].instance_module.as_ref(), "using1");
        // inner node keeps its captured used_from (its own inner-uses site).
        assert_eq!(dst[1].used_from.as_ref().unwrap().range, 9..10);

        // A second instantiation yields distinct ids with remapped links.
        let ns2 = Arc::from("using2");
        let roots2 = instantiate_run(&mut dst, &tmpl, Some(ext), &ns2);
        assert_eq!(roots2, vec![2usize]);
        assert_eq!(dst[2].parent, Some(ext));
        assert_eq!(dst[2].instance_module.as_ref(), "using2");
        assert_eq!(dst[3].parent, Some(2));
        assert_ne!(roots1[0], roots2[0]);
    }

    #[test]
    fn snapshot_ignores_empty() {
        let src: Vec<SchemaNode> = Vec::new();
        let tmpl = snapshot_run(&src, 0, 0);
        let mut dst = Vec::new();
        let roots = instantiate_run(&mut dst, &tmpl, None, &Arc::from("n"));
        assert!(roots.is_empty());
        assert!(dst.is_empty());
    }

    #[allow(dead_code)]
    fn _stmt_kind_probe(_: StatementKind) {}
}
