//! `Library`: the resolved semantic database ([D1]) + `Outcome`.
//!
//! Cheap to clone/snapshot (`Arc`). All lookups are read-only.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::diag::Diagnostic;
use crate::schema::{
    BUILTIN_TYPES, ExtensionDef, FeatureDef, Grouping, Identity, IdentityRef, IdentityResolution,
    ModuleRecord, SchemaNode, SubmoduleRecord, TypeCandidate, TypeCandidateKind, TypeResolution,
    TypeStep, Typedef,
};

/// The result of `Repository::compile()`: diagnostics plus, when at least one
/// module compiled, a snapshot `Library`. Content problems are *never* errors
/// here ([D3]).
pub struct Outcome {
    pub library: Option<Arc<Library>>,
    pub diagnostics: Vec<Diagnostic>,
}

/// The resolved, queryable database for a workspace.
pub struct Library {
    modules: Vec<ModuleRecord>,
    /// module name -> index of the "latest revision" module (name-only lookup).
    latest: HashMap<String, usize>,
    /// (name, revision) -> index.
    by_rev: HashMap<(String, String), usize>,
    submodules: Vec<SubmoduleRecord>,
    sub_by_name: HashMap<String, Vec<usize>>,
}

impl Library {
    pub(crate) fn from_parts(
        modules: Vec<ModuleRecord>,
        submodules: Vec<SubmoduleRecord>,
    ) -> Library {
        let mut latest: HashMap<String, usize> = HashMap::new();
        let mut by_rev: HashMap<(String, String), usize> = HashMap::new();
        for (i, m) in modules.iter().enumerate() {
            let rev = m.revision.clone().unwrap_or_default();
            by_rev.insert((m.name.clone(), rev), i);
            let replace = match latest.get(&m.name) {
                None => true,
                Some(&cur) => {
                    let cur_rev = modules[cur].revision.as_deref().unwrap_or("");
                    let new_rev = m.revision.as_deref().unwrap_or("");
                    new_rev > cur_rev
                }
            };
            if replace {
                latest.insert(m.name.clone(), i);
            }
        }

        let mut sub_by_name: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, s) in submodules.iter().enumerate() {
            sub_by_name.entry(s.name.clone()).or_default().push(i);
        }

        Library {
            modules,
            latest,
            by_rev,
            submodules,
            sub_by_name,
        }
    }

    // ---- modules --------------------------------------------------------

    /// All compiled modules (submodules folded into their parents).
    pub fn modules(&self) -> &[ModuleRecord] {
        &self.modules
    }

    /// Look up a module by name, resolving to its **latest** revision.
    ///
    /// Rustdoc (name-only variant): a module with no `revision` is valid and
    /// its revision is treated as empty. When several revisions of one name are
    /// loaded the most recent is returned; use [`Library::module_rev`] to pick
    /// an exact one.
    pub fn module(&self, name: &str) -> Option<&ModuleRecord> {
        self.latest.get(name).map(|&i| &self.modules[i])
    }

    /// Look up a module by its **exact** `(name, revision)`.
    ///
    /// Rustdoc (rev-required variant): `revision` is the value of the module's
    /// `revision` statement (e.g. `"2026-01-31"`). A module written without a
    /// `revision` is registered under the empty revision. Returns `None` when
    /// that exact revision is not loaded.
    pub fn module_rev(&self, name: &str, revision: &str) -> Option<&ModuleRecord> {
        self.by_rev
            .get(&(name.to_string(), revision.to_string()))
            .map(|&i| &self.modules[i])
    }

    // ---- submodules -----------------------------------------------------

    pub fn submodules(&self) -> &[SubmoduleRecord] {
        &self.submodules
    }

    /// Look up a submodule document by name. Useful for `goto` on `include`
    /// arguments ([D6]).
    pub fn submodule(&self, name: &str) -> Option<&SubmoduleRecord> {
        self.sub_by_name
            .get(name)
            .and_then(|v| v.first())
            .map(|&i| &self.submodules[i])
    }

    // ---- symbols --------------------------------------------------------

    pub fn search_type(&self, module: &str, type_name: &str) -> Option<&Typedef> {
        self.module(module)?
            .typedefs
            .iter()
            .find(|t| t.name == type_name)
    }

    pub fn search_grouping(&self, module: &str, grouping: &str) -> Option<&Grouping> {
        self.module(module)?
            .groupings
            .iter()
            .find(|g| g.name == grouping)
    }

    pub fn search_identity(&self, module: &str, identity: &str) -> Option<&Identity> {
        self.module(module)?
            .identities
            .iter()
            .find(|i| i.name == identity)
    }

    pub fn search_extension(&self, module: &str, extension: &str) -> Option<&ExtensionDef> {
        self.module(module)?
            .extensions
            .iter()
            .find(|e| e.name == extension)
    }

    pub fn search_feature(&self, module: &str, feature: &str) -> Option<&FeatureDef> {
        self.module(module)?
            .features
            .iter()
            .find(|f| f.name == feature)
    }

    // ---- prefixes -------------------------------------------------------

    /// Resolve a prefix to a module name, in the scope of `module`.
    ///
    /// Covers the module's own prefix, its imports, and (for folded submodule
    /// content) `belongs-to` prefixes.
    pub fn prefix_to_module(&self, module: &str, prefix: &str) -> Option<&str> {
        let m = self.module(module)?;
        m.prefix_map.get(prefix).map(|s| s.as_ref())
    }

    // ---- schema-nodeid resolution --------------------------------------

    /// Resolve an absolute (or descendant) schema-nodeid to an effective node.
    ///
    /// Backs goto/hover on `augment`/`refine`/`deviation` arguments and type
    /// references (D9). `path` may be prefix-qualified (`/if:x/if:y`) or
    /// bare (`/x/y`) — the first segment resolves against `module`'s scope.
    pub fn resolve_abs_schema_node_id(&self, module: &str, path: &str) -> Option<&SchemaNode> {
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if segments.is_empty() {
            return None;
        }
        let (target_name, first_local) = match segments[0].split_once(':') {
            Some((prefix, local)) => {
                let t = self.prefix_to_module(module, prefix)?;
                (t.to_string(), local)
            }
            None => (module.to_string(), segments[0]),
        };
        let target = self.module(&target_name)?;
        let mut current = target
            .top_nodes()
            .iter()
            .find(|&&id| {
                target
                    .node(id)
                    .map(|n| n.name() == first_local)
                    .unwrap_or(false)
            })
            .copied()?;
        for seg in &segments[1..] {
            let local = seg.rsplit(':').next().unwrap_or(seg);
            let node = target.node(current)?;
            current = node
                .children()
                .iter()
                .find(|&&id| target.node(id).map(|n| n.name() == local).unwrap_or(false))
                .copied()?;
        }
        target.node(current)
    }

    // ---- type / identity resolution (existence + chain, D13) ------------

    /// Resolve `[prefix:]name` against `module`'s scope into `(module, local)`.
    /// `None` when the prefix is unmapped in `module`.
    fn qualify(&self, module: &str, text: &str) -> Option<(String, String)> {
        match text.split_once(':') {
            None => Some((module.to_string(), text.to_string())),
            Some((prefix, local)) => self
                .prefix_to_module(module, prefix)
                .map(|m| (m.to_string(), local.to_string())),
        }
    }

    /// Resolve a type reference — following the typedef chain, possibly across
    /// modules — down to a builtin type when possible.
    ///
    /// `None` when `type_name` is neither a builtin nor a typedef in scope.
    /// When the chain is resolvable but open (a base or typedef is missing),
    /// the returned [`TypeResolution`] has `complete == false`.
    ///
    /// ```text
    /// resolve_type("m", "service-port") ⇒ typedefs: [service-port, port], builtin: Some("uint16")
    /// ```
    pub fn resolve_type(&self, module: &str, type_name: &str) -> Option<TypeResolution> {
        if crate::schema::is_builtin_type(type_name) {
            return Some(TypeResolution {
                builtin: Some(type_name.to_string()),
                typedefs: Vec::new(),
                complete: true,
            });
        }
        let (m0, l0) = self.qualify(module, type_name)?;
        let first = self
            .module(&m0)?
            .typedefs
            .iter()
            .find(|t| t.name == l0)?
            .clone();

        let mut steps = Vec::new();
        let mut seen: HashSet<(String, String)> = HashSet::new();
        let mut cur_mod = m0;
        let mut cur = first;
        loop {
            let key = (cur_mod.clone(), cur.name.clone());
            if !seen.insert(key) {
                return Some(TypeResolution {
                    builtin: None,
                    typedefs: steps,
                    complete: false,
                });
            }
            steps.push(TypeStep {
                module: Arc::from(cur_mod.as_str()),
                name: cur.name.clone(),
                defining: cur.defining.clone(),
                base: cur.base.clone(),
            });
            let Some(base) = cur.base.as_deref() else {
                return Some(TypeResolution {
                    builtin: None,
                    typedefs: steps,
                    complete: false,
                });
            };
            if crate::schema::is_builtin_type(base) {
                return Some(TypeResolution {
                    builtin: Some(base.to_string()),
                    typedefs: steps,
                    complete: true,
                });
            }
            let (m1, l1) = match self.qualify(&cur_mod, base) {
                Some(x) => x,
                None => {
                    return Some(TypeResolution {
                        builtin: None,
                        typedefs: steps,
                        complete: false,
                    });
                }
            };
            let next = match self.module(&m1) {
                Some(r) => match r.typedefs.iter().find(|t| t.name == l1) {
                    Some(t) => t.clone(),
                    None => {
                        return Some(TypeResolution {
                            builtin: None,
                            typedefs: steps,
                            complete: false,
                        });
                    }
                },
                None => {
                    return Some(TypeResolution {
                        builtin: None,
                        typedefs: steps,
                        complete: false,
                    });
                }
            };
            cur_mod = m1;
            cur = next;
        }
    }

    /// Resolve an identity and the chain of its bases (its ancestry).
    pub fn resolve_identity(&self, module: &str, name: &str) -> Option<IdentityResolution> {
        let (m0, l0) = self.qualify(module, name)?;
        let rec = self.module(&m0)?;
        let root = rec.identities.iter().find(|i| i.name == l0)?.clone();
        let root_ref = IdentityRef {
            module: Arc::from(m0.as_str()),
            name: root.name.clone(),
            defining: root.defining.clone(),
            base: root.base.clone(),
        };
        let mut bases = Vec::new();
        let mut seen: HashSet<(String, String)> = HashSet::new();
        let mut cur_mod = m0;
        let mut cur = root;
        loop {
            if !seen.insert((cur_mod.clone(), cur.name.clone())) {
                break;
            }
            let Some(base) = cur.base.clone() else {
                break;
            };
            let Some((m1, l1)) = self.qualify(&cur_mod, &base) else {
                break;
            };
            let Some(next) = self
                .module(&m1)
                .and_then(|r| r.identities.iter().find(|i| i.name == l1))
                .cloned()
            else {
                break;
            };
            bases.push(IdentityRef {
                module: Arc::from(m1.as_str()),
                name: next.name.clone(),
                defining: next.defining.clone(),
                base: next.base.clone(),
            });
            cur_mod = m1;
            cur = next;
        }
        Some(IdentityResolution {
            root: root_ref,
            bases,
        })
    }

    /// Every identity that is `base` or is (transitively) derived from it —
    /// the value set an `identityref { base … }` accepts.
    pub fn derived_identities(&self, module: &str, base: &str) -> Vec<IdentityRef> {
        let Some((bm, bl)) = self.qualify(module, base) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for rec in &self.modules {
            for id in &rec.identities {
                if self.identity_reaches(&rec.name, id, &bm, &bl) {
                    out.push(IdentityRef {
                        module: Arc::from(rec.name.as_str()),
                        name: id.name.clone(),
                        defining: id.defining.clone(),
                        base: id.base.clone(),
                    });
                }
            }
        }
        out.sort_by(|a, b| (&*a.module, &a.name).cmp(&(&*b.module, &b.name)));
        out
    }

    fn identity_reaches(
        &self,
        module: &str,
        id: &Identity,
        target: &str,
        target_local: &str,
    ) -> bool {
        let mut cur_mod = module.to_string();
        let mut cur = id.clone();
        let mut seen: HashSet<(String, String)> = HashSet::new();
        loop {
            if !seen.insert((cur_mod.clone(), cur.name.clone())) {
                return false;
            }
            if cur_mod == target && cur.name == target_local {
                return true;
            }
            let Some(base) = cur.base.clone() else {
                return false;
            };
            let Some((m1, l1)) = self.qualify(&cur_mod, &base) else {
                return false;
            };
            let Some(next) = self
                .module(&m1)
                .and_then(|r| r.identities.iter().find(|i| i.name == l1))
                .cloned()
            else {
                return false;
            };
            cur_mod = m1;
            cur = next;
        }
    }

    /// Completion candidates for a `type` argument: builtins, this module's
    /// typedefs, and imported typedefs as `prefix:name`.
    pub fn type_candidates(&self, module: &str) -> Vec<TypeCandidate> {
        let mut out: Vec<TypeCandidate> = BUILTIN_TYPES
            .iter()
            .map(|b| TypeCandidate {
                name: (*b).to_string(),
                kind: TypeCandidateKind::Builtin,
                module: None,
            })
            .collect();
        let Some(rec) = self.module(module) else {
            return out;
        };
        for t in &rec.typedefs {
            out.push(TypeCandidate {
                name: t.name.clone(),
                kind: TypeCandidateKind::Typedef,
                module: Some(module.to_string()),
            });
        }
        for imp in &rec.imports {
            if let Some(trec) = self.module(&imp.module) {
                for t in &trec.typedefs {
                    out.push(TypeCandidate {
                        name: format!("{}:{}", imp.prefix, t.name),
                        kind: TypeCandidateKind::Typedef,
                        module: Some(imp.module.clone()),
                    });
                }
            }
        }
        out
    }

    /// Completion candidates for an identity `base` argument: this module's
    /// identities plus imported ones as `prefix:name`.
    pub fn identity_candidates(&self, module: &str) -> Vec<String> {
        let mut out = Vec::new();
        let Some(rec) = self.module(module) else {
            return out;
        };
        for i in &rec.identities {
            out.push(i.name.clone());
        }
        for imp in &rec.imports {
            if let Some(irec) = self.module(&imp.module) {
                for i in &irec.identities {
                    out.push(format!("{}:{}", imp.prefix, i.name));
                }
            }
        }
        out
    }
}
