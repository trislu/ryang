//! Semantic model: the **effective/expanded schema tree** ([D9]) and the
//! per-module records that the `Library` is built from.

use std::collections::HashMap;
use std::sync::Arc;

use crate::diag::Location;

/// Arena index of a schema node inside its module.
pub type NodeId = usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Container,
    Leaf,
    LeafList,
    List,
    Choice,
    Case,
    Anyxml,
    Anydata,
    Rpc,
    Action,
    Notification,
    Input,
    Output,
}

impl NodeKind {
    pub fn as_str(self) -> &'static str {
        use NodeKind::*;
        match self {
            Container => "container",
            Leaf => "leaf",
            LeafList => "leaf-list",
            List => "list",
            Choice => "choice",
            Case => "case",
            Anyxml => "anyxml",
            Anydata => "anydata",
            Rpc => "rpc",
            Action => "action",
            Notification => "notification",
            Input => "input",
            Output => "output",
        }
    }
}

/// One node of the effective schema tree.
pub struct SchemaNode {
    pub(crate) kind: NodeKind,
    pub(crate) name: String,
    pub(crate) parent: Option<NodeId>,
    pub(crate) children: Vec<NodeId>,
    /// Where the node's definition is written in the source.
    pub(crate) defining: Location,
    /// The `uses` site that instantiated this node (grouping-born nodes).
    pub(crate) used_from: Option<Location>,
    /// Module that owns this node's definition (a grouping defined in module X
    /// yields nodes whose `origin_module` is X even when used in module Y).
    pub(crate) origin_module: Arc<str>,

    pub(crate) config: Option<bool>,
    pub(crate) mandatory: bool,
    pub(crate) presence: Option<String>,
    pub(crate) default: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) ordered_by: Option<String>,
    pub(crate) min_elements: Option<String>,
    pub(crate) max_elements: Option<String>,
    pub(crate) keys: Vec<String>,
    pub(crate) is_key: bool,
    pub(crate) type_name: Option<String>,
    /// Removed by a `deviation … deviate not-supported`.
    pub(crate) removed: bool,
}

impl SchemaNode {
    pub fn kind(&self) -> NodeKind {
        self.kind
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn parent(&self) -> Option<NodeId> {
        self.parent
    }
    pub fn children(&self) -> &[NodeId] {
        &self.children
    }
    pub fn defining(&self) -> &Location {
        &self.defining
    }
    pub fn used_from(&self) -> Option<&Location> {
        self.used_from.as_ref()
    }
    pub fn origin_module(&self) -> &str {
        &self.origin_module
    }
    pub fn is_removed(&self) -> bool {
        self.removed
    }
    pub fn config(&self) -> Option<bool> {
        self.config
    }
    pub fn is_mandatory(&self) -> bool {
        self.mandatory
    }
    pub fn presence(&self) -> Option<&str> {
        self.presence.as_deref()
    }
    pub fn default(&self) -> Option<&str> {
        self.default.as_deref()
    }
    pub fn keys(&self) -> &[String] {
        &self.keys
    }
    pub fn is_key(&self) -> bool {
        self.is_key
    }
    pub fn type_name(&self) -> Option<&str> {
        self.type_name.as_deref()
    }
    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }
    pub fn ordered_by(&self) -> Option<&str> {
        self.ordered_by.as_deref()
    }
    pub fn min_elements(&self) -> Option<&str> {
        self.min_elements.as_deref()
    }
    pub fn max_elements(&self) -> Option<&str> {
        self.max_elements.as_deref()
    }
}

/// A `typedef` symbol (not a schema node).
#[derive(Debug, Clone)]
pub struct Typedef {
    pub name: String,
    pub defining: Location,
    /// The raw argument of the typedef's `type` statement (`uint16`, `my-int`,
    /// `inet:ip-address`, …), when present. A typedef without a `type` is
    /// malformed.
    pub base: Option<String>,
    /// Where the `type` argument is written (for diagnostics).
    pub(crate) base_loc: Option<Location>,
}

impl Typedef {
    pub fn base(&self) -> Option<&str> {
        self.base.as_deref()
    }
}

/// A `grouping` symbol (not a schema node).
#[derive(Debug, Clone)]
pub struct Grouping {
    pub name: String,
    pub defining: Location,
}

/// An `identity` symbol (not a schema node).
#[derive(Debug, Clone)]
pub struct Identity {
    pub name: String,
    pub defining: Location,
    /// The raw argument of the identity's `base` statement (possibly
    /// prefix-qualified), when the identity is derived from another one.
    pub base: Option<String>,
    /// Where the `base` argument is written (for diagnostics).
    pub(crate) base_loc: Option<Location>,
}

impl Identity {
    pub fn base(&self) -> Option<&str> {
        self.base.as_deref()
    }
}

/// An `extension` symbol (not a schema node).
#[derive(Debug, Clone)]
pub struct ExtensionDef {
    pub name: String,
    pub defining: Location,
    /// The `argument` statement's name, when the extension declares one.
    pub argument: Option<String>,
}

/// A `feature` symbol (not a schema node).
#[derive(Debug, Clone)]
pub struct FeatureDef {
    pub name: String,
    pub defining: Location,
}

// ---- shared helpers -----------------------------------------------------

/// The RFC 7950 builtin type names (single source of truth).
pub(crate) const BUILTIN_TYPES: &[&str] = &[
    "binary",
    "bits",
    "boolean",
    "decimal64",
    "empty",
    "enumeration",
    "identityref",
    "instance-identifier",
    "int8",
    "int16",
    "int32",
    "int64",
    "leafref",
    "string",
    "uint8",
    "uint16",
    "uint32",
    "uint64",
    "union",
];

pub(crate) fn is_builtin_type(name: &str) -> bool {
    BUILTIN_TYPES.contains(&name)
}

// ---- symbol resolution result types -------------------------------------

/// One typedef step on the way from a type reference to a builtin.
#[derive(Debug, Clone)]
pub struct TypeStep {
    /// Module that defines the typedef.
    pub module: Arc<str>,
    pub name: String,
    pub defining: Location,
    /// The raw `type` argument this typedef is based on.
    pub base: Option<String>,
}

/// The resolution of a `type` reference: a typedef chain ending in a builtin.
#[derive(Debug, Clone)]
pub struct TypeResolution {
    /// The builtin type the chain bottoms out at (e.g. `"uint16"`), when
    /// reached; `None` when the chain is incomplete or acyclic-but-open.
    pub builtin: Option<String>,
    /// Typedefs traversed, outermost first. Empty when `name` was a builtin.
    pub typedefs: Vec<TypeStep>,
    /// `false` when some step in the chain could not be resolved (an
    /// existence diagnostic is reported separately).
    pub complete: bool,
}

/// A resolved identity, with its module.
#[derive(Debug, Clone)]
pub struct IdentityRef {
    pub module: Arc<str>,
    pub name: String,
    pub defining: Location,
    /// Raw `base` argument, when derived.
    pub base: Option<String>,
}

impl IdentityRef {
    /// `name` or `module:name` when the module has a prefix-able identity.
    pub fn qualified(&self) -> String {
        format!("{}:{}", self.module, self.name)
    }
}

/// Resolution of an identity: the identity plus the chain of its bases.
#[derive(Debug, Clone)]
pub struct IdentityResolution {
    pub root: IdentityRef,
    /// The base identities above `root`, innermost first (each is `base`-d by
    /// the previous). Empty when `root` is not derived.
    pub bases: Vec<IdentityRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeCandidateKind {
    Builtin,
    Typedef,
}

/// A completion candidate for a `type` argument.
#[derive(Debug, Clone)]
pub struct TypeCandidate {
    /// The text to insert (`uint16`, `my-int`, `inet:ip-address`, …).
    pub name: String,
    pub kind: TypeCandidateKind,
    /// Module the candidate is defined in (`None` for builtins).
    pub module: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviationOp {
    NotSupported,
    Add,
    Delete,
    Replace,
}

/// A module-level `augment` that has been resolved and applied.
#[derive(Debug, Clone)]
pub struct AppliedAugment {
    /// The written target (absolute schema-nodeid), as normalized.
    pub target: String,
    /// Resolved target node in the (possibly foreign) module's arena.
    pub target_node: NodeId,
    /// Module in which the `augment` is written.
    pub source_module: Arc<str>,
    /// Location of the `augment` statement.
    pub defining: Location,
}

/// A module-level `deviation` (recorded; `NotSupported` is applied).
#[derive(Debug, Clone)]
pub struct AppliedDeviation {
    pub target: String,
    pub target_node: Option<NodeId>,
    pub op: DeviationOp,
    pub defining: Location,
}

#[derive(Debug, Clone)]
pub struct ImportInfo {
    pub module: String,
    pub prefix: String,
}

/// The compiled semantic record for one module (submodules folded in, [D6]).
pub struct ModuleRecord {
    pub(crate) name: String,
    pub(crate) revision: Option<String>,
    pub(crate) namespace: Option<String>,
    pub(crate) prefix: Option<String>,
    /// Physical documents folded into this module (module file first).
    pub(crate) source_urls: Vec<Arc<str>>,
    pub(crate) imports: Vec<ImportInfo>,
    pub(crate) includes: Vec<String>,
    /// prefix -> module name, covering own prefix, imports, and submodule
    /// belongs-to prefixes.
    pub(crate) prefix_map: HashMap<String, String>,
    pub(crate) nodes: Vec<SchemaNode>,
    pub(crate) top: Vec<NodeId>,
    pub(crate) groupings: Vec<Grouping>,
    pub(crate) typedefs: Vec<Typedef>,
    pub(crate) identities: Vec<Identity>,
    pub(crate) extensions: Vec<ExtensionDef>,
    pub(crate) features: Vec<FeatureDef>,
    pub(crate) augments: Vec<AppliedAugment>,
    pub(crate) deviations: Vec<AppliedDeviation>,
}

impl ModuleRecord {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn revision(&self) -> Option<&str> {
        self.revision.as_deref()
    }
    pub fn namespace(&self) -> Option<&str> {
        self.namespace.as_deref()
    }
    pub fn prefix(&self) -> Option<&str> {
        self.prefix.as_deref()
    }
    pub fn source_urls(&self) -> &[Arc<str>] {
        &self.source_urls
    }
    pub fn imports(&self) -> &[ImportInfo] {
        &self.imports
    }
    pub fn includes(&self) -> &[String] {
        &self.includes
    }
    pub fn nodes(&self) -> &[SchemaNode] {
        &self.nodes
    }
    pub fn node(&self, id: NodeId) -> Option<&SchemaNode> {
        self.nodes.get(id)
    }
    pub fn top_nodes(&self) -> &[NodeId] {
        &self.top
    }
    pub fn groupings(&self) -> &[Grouping] {
        &self.groupings
    }
    pub fn typedefs(&self) -> &[Typedef] {
        &self.typedefs
    }
    pub fn identities(&self) -> &[Identity] {
        &self.identities
    }
    pub fn extensions(&self) -> &[ExtensionDef] {
        &self.extensions
    }
    pub fn features(&self) -> &[FeatureDef] {
        &self.features
    }
    pub fn augments(&self) -> &[AppliedAugment] {
        &self.augments
    }
    pub fn deviations(&self) -> &[AppliedDeviation] {
        &self.deviations
    }
}

/// A submodule document known to the `Library` ([D6]).
///
/// A submodule is always a separate editor document (its own `url`); when its
/// parent module is present **and** includes it, its content is folded into the
/// parent `ModuleRecord` (`parent_module` is set) and every folded node keeps a
/// `Location` pointing back into this submodule file.
#[derive(Debug, Clone)]
pub struct SubmoduleRecord {
    pub name: String,
    pub revision: Option<String>,
    /// `(parent module name, prefix used to refer to the parent)`.
    pub belongs_to: Option<(String, String)>,
    pub url: Arc<str>,
    /// Parent module into which this submodule was folded, if any.
    pub parent_module: Option<String>,
}

impl SubmoduleRecord {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn revision(&self) -> Option<&str> {
        self.revision.as_deref()
    }
    pub fn url(&self) -> &Arc<str> {
        &self.url
    }
    pub fn parent_module(&self) -> Option<&str> {
        self.parent_module.as_deref()
    }
}
