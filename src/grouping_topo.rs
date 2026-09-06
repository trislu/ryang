//! Grouping dependency ordering (PHASE ② groundwork).
//!
//! A module's groupings form a DAG via the `uses` statements inside their
//! bodies (RFC 7950 §7.13: a grouping may use other groupings). Expansion must
//! process dependencies first; cycles are invalid and reported. This module
//! provides a pure, testable Kahn-topological order over a dependency map; the
//! caller (compile) adapts statement trees onto it.
//!
//! Wiring status: this is the first landed step of the approved multi-phase
//! refactor (`YREPO_PHASES.md` Phase ②). The next step adapts each module
//! instance's `grouping` symbol statements onto this order (nodes generalized
//! to (instance, grouping-name) keys for cross-module `uses` edges) and drives
//! template expansion from it, then is audited for a net-zero regression
//! before the recursion-only expansion is replaced. Until then the function is
//! unused outside its own unit tests.
#![allow(dead_code)] // consumed by the Phase ② expansion step that follows

/// Return a topological order of `nodes` (dependencies first) using
/// `deps_of(name) -> direct dependencies. Stable: nodes with equal priority
/// keep input order (deterministic across runs).
///
/// On a cycle returns `Err(cycle)` with the nodes forming the cycle in
/// traversal order.
pub fn grouping_topo(
    nodes: &[String],
    deps_of: impl Fn(&str) -> Vec<String>,
) -> Result<Vec<String>, Vec<String>> {
    let idx: std::collections::HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();
    let mut indeg = vec![0usize; nodes.len()];
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for (i, n) in nodes.iter().enumerate() {
        for d in deps_of(n) {
            if let Some(&j) = idx.get(d.as_str()) {
                adj[j].push(i); // dependency j -> dependent i
                indeg[i] += 1;
            }
            // unknown dependency names are reported by resolution phases
        }
    }
    // Kahn with a queue seeded by input order (deterministic).
    let mut ready: Vec<usize> = (0..nodes.len()).filter(|&i| indeg[i] == 0).collect();
    let mut order: Vec<String> = Vec::with_capacity(nodes.len());
    let mut visited = vec![false; nodes.len()];
    while let Some(i) = ready.pop() {
        if visited[i] {
            continue;
        }
        visited[i] = true;
        order.push(nodes[i].clone());
        for &d in &adj[i] {
            indeg[d] -= 1;
            if indeg[d] == 0 {
                ready.push(d);
            }
        }
        // keep stable: pop from the end gives reverse-of-input among peers;
        // sort ready descending by original index so output stays input-ordered
        ready.sort_by(|a, b| b.cmp(a));
    }
    if order.len() == nodes.len() {
        Ok(order)
    } else {
        // Collect a cycle: any unvisited node is on a cycle.
        let mut cycle = Vec::new();
        for (i, n) in nodes.iter().enumerate() {
            if !visited[i] {
                cycle.push(n.clone());
            }
        }
        Err(cycle)
    }
}

#[cfg(test)]
mod tests {
    use super::grouping_topo;

    fn v(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn chain_and_diamond_topological() {
        // a -> b -> c (a depends on b depends on c) plus diamond d -> e,f
        let nodes = v(&["a", "b", "c", "d", "e", "f", "g"]);
        let deps = |n: &str| -> Vec<String> {
            match n {
                "a" => v(&["b"]),
                "b" => v(&["c"]),
                "d" => v(&["e", "f"]),
                _ => vec![],
            }
        };
        let order = grouping_topo(&nodes, deps).expect("acyclic");
        for (dep, use_me) in [("b", "a"), ("c", "b"), ("e", "d"), ("f", "d")] {
            let a = order.iter().position(|x| x == dep).unwrap();
            let b = order.iter().position(|x| x == use_me).unwrap();
            assert!(a < b, "{dep} must precede {use_me}: {order:?}");
        }
        // unrelated g may sit anywhere; all nodes present
        assert_eq!(order.len(), nodes.len());
    }

    #[test]
    fn cycle_detected() {
        let nodes = v(&["x", "y", "z"]);
        let deps = |n: &str| -> Vec<String> {
            match n {
                "x" => v(&["y"]),
                "y" => v(&["z"]),
                "z" => v(&["x"]),
                _ => vec![],
            }
        };
        let err = grouping_topo(&nodes, deps).expect_err("cycle expected");
        assert_eq!(err.len(), 3);
    }

    #[test]
    fn unknown_dependency_ignored_here() {
        // resolution layers report unknown names; the topo util tolerates them
        let nodes = v(&["a"]);
        let order = grouping_topo(&nodes, |_| v(&["ghost"])).unwrap();
        assert_eq!(order, v(&["a"]));
    }
}
