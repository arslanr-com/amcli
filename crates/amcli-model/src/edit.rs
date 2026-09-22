//! Mutations.
//!
//! Everything that changes a model goes through here, and everything here is
//! in-memory. Persisting is a separate, explicit step, which is what lets a
//! batch of edits either all land or leave the file byte-identical.
//!
//! Attribute order for newly created nodes matches what Archi writes — verified
//! against its own test fixtures. Getting it wrong would not corrupt anything,
//! but it would make every diff noisier than it needs to be, and a noisy diff is
//! how a review stops catching real changes.

use amcli_xml::{NodeBuilder, NodeId};

use crate::model::{Concept, ConceptId, ConceptKind, FolderId, ViewId};
use crate::{ElementType, FolderType, Model, ModelError, RelType, matrix};

/// Why an edit was refused.
#[derive(Debug, thiserror::Error)]
pub enum EditError {
    #[error("no concept with id `{0}`")]
    NoSuchConcept(String),
    #[error("no folder at `{0}`")]
    NoSuchFolder(String),
    #[error(
        "`{0}` is not under the views folder; a diagram filed elsewhere does not load in Archi"
    )]
    NotAViewsFolder(String),
    #[error("`{0}` still holds {1} item(s); only an empty folder can be deleted")]
    FolderNotEmpty(String, usize),
    #[error("`{0}` is one of the folders Archi expects at the top; it cannot be deleted")]
    TopFolder(String),
    #[error("`{0}` is one of the folders Archi expects at the top; it cannot be renamed or moved")]
    TopFolderFixed(String),
    #[error(
        "`{0}` cannot move under `{1}`: a folder stays inside the top-level folder of its type, \
         because Archi files each kind of thing in its own tree"
    )]
    FolderTree(String, String),
    #[error("`{0}` cannot move under `{1}`, which is inside it")]
    FolderCycle(String, String),
    #[error("no object `{0}` on view `{1}`")]
    NoSuchObject(String, String),
    #[error("`{0}` cannot be nested inside `{1}`, which is inside it")]
    ObjectCycle(String, String),
    #[error("`{0}` is not something that can hold other objects")]
    NotAContainer(String),
    #[error(
        "ArchiMate does not permit {rel} from {source_type} to {target_type}{}",
        permitted_hint(.permitted)
    )]
    InvalidRelationship {
        rel: &'static str,
        source_type: String,
        target_type: String,
        permitted: Vec<&'static str>,
    },
    // Note: not named `source`, which thiserror reserves for the error cause.
    #[error("a {rel} relationship from `{from}` to `{to}` already exists (id `{existing}`)")]
    DuplicateRelationship { rel: &'static str, from: String, to: String, existing: String },
    #[error("every relationship at a junction must be the same type; `{0}` already has {1}")]
    MixedJunction(String, &'static str),
    #[error("accessType must be 0 (write), 1 (read), 2 (unspecified) or 3 (read/write), not {0}")]
    BadAccessType(i64),
    #[error("`{0}` is a relationship; only an element can be {1}")]
    NotAnElement(String, &'static str),
    #[error("`{0}` is a junction; a junction joins relationships of one type and cannot be {1}")]
    JunctionFixed(String, &'static str),
    #[error("`{0}` cannot be merged into itself")]
    SameElement(String),
    #[error(
        "{} relationship(s) would break the ArchiMate matrix: {}",
        .0.len(),
        illegal_hint(.0)
    )]
    IllegalRelationships(Vec<IllegalRelationship>),
    #[error(transparent)]
    Model(#[from] ModelError),
    #[error(transparent)]
    Xml(#[from] amcli_xml::MixedContent),
}

fn permitted_hint(p: &[&'static str]) -> String {
    if p.is_empty() {
        " — no relationship type is permitted between these two".to_string()
    } else {
        format!(" — permitted here: {}", p.join(", "))
    }
}

fn illegal_hint(list: &[IllegalRelationship]) -> String {
    list.iter()
        .map(|r| {
            format!(
                "{} `{}` {} {} `{}`{}",
                r.rel,
                r.id,
                if r.direction == "out" { "to" } else { "from" },
                r.other_type,
                if r.other_name.is_empty() { &r.other } else { &r.other_name },
                permitted_hint(&r.permitted)
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// A relationship the matrix would refuse once one of its ends changed:
/// an element retyped, or folded into another. What it is, which end would
/// change, and what the matrix does permit for the pair it would then join.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IllegalRelationship {
    pub id: String,
    /// The relationship's type, bare.
    pub rel: &'static str,
    /// `out` when the changed element is the source, `in` when it is the
    /// target.
    pub direction: &'static str,
    /// The end that is not changing: its id, name and type.
    pub other: String,
    pub other_name: String,
    pub other_type: String,
    /// Bare relationship types permitted between the pair as it would be.
    pub permitted: Vec<&'static str>,
}

/// What a merge did, by id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Merged {
    /// Relationships repointed to the survivor.
    pub relationships: Vec<String>,
    /// Relationships deleted: a twin of one the survivor already had, or a
    /// relationship between the two elements, which would have looped.
    pub dropped: Vec<String>,
    /// Diagram objects now showing the survivor.
    pub objects: Vec<String>,
    /// Names of the views that now show the survivor in more than one box.
    pub duplicated_on: Vec<String>,
}

/// What a delete would take with it. Returned before anything is touched so a
/// caller can look before it leaps, and returned again afterwards as a record.
#[derive(Clone, Debug, Default)]
pub struct Cascade {
    /// Concepts removed, the requested one first.
    pub concepts: Vec<String>,
    /// Relationships removed because an endpoint went.
    pub relationships: Vec<String>,
    /// Diagram objects removed because the concept they showed went.
    pub diagram_objects: Vec<String>,
    /// Connections removed because their relationship or an endpoint went.
    pub connections: Vec<String>,
    /// Views whose contents changed.
    pub views: Vec<String>,
    /// Junctions left with fewer than two connections. Flagged, never
    /// auto-deleted: a junction is a modelling decision, not debris.
    pub degenerate_junctions: Vec<String>,
}

impl Cascade {
    pub fn is_empty(&self) -> bool {
        self.relationships.is_empty()
            && self.diagram_objects.is_empty()
            && self.connections.is_empty()
    }

    pub fn total(&self) -> usize {
        self.concepts.len()
            + self.relationships.len()
            + self.diagram_objects.len()
            + self.connections.len()
    }
}

impl Model {
    // ---- creating -------------------------------------------------------

    /// Add an element. With no folder given it lands in the one Archi would
    /// have chosen for its type, which keeps the folder taxonomy from rotting
    /// the way it does when everything is dumped in one place.
    pub fn add_element(
        &mut self,
        ty: ElementType,
        name: &str,
        folder: Option<FolderId>,
        documentation: Option<&str>,
    ) -> Result<ConceptId, EditError> {
        let folder = match folder {
            Some(f) => f,
            None => self
                .top_folder(ty.info().home)
                .ok_or_else(|| EditError::NoSuchFolder(ty.info().home.as_str().to_string()))?,
        };
        let id = self.fresh_id(&["element", ty.info().xsi, name]);
        let folder_node = self.folder(folder).node;
        // Attribute order matches what Archi writes: xsi:type, name, id. Not a
        // correctness issue, but the wrong order makes every diff noisier, and
        // a noisy diff is how review stops catching real changes.
        let node = self.doc.append_child(
            folder_node,
            NodeBuilder::new("element")
                .attr("xsi:type", ty.info().xsi)
                .attr("name", name)
                .attr("id", &*id),
        )?;
        if let Some(doc) = documentation.filter(|d| !d.is_empty()) {
            self.set_documentation_node(node, doc)?;
        }
        self.reindex();
        Ok(self.concept_by_id(&id).expect("just added"))
    }

    /// Add a relationship, refusing anything ArchiMate does not permit.
    ///
    /// Three checks, matching what Archi enforces: the matrix, no duplicate
    /// direct relationship of the same type between the same ordered pair, and
    /// the rule that every relationship touching a junction shares its type.
    ///
    /// `name` is the label Archi shows on the line — an Association reads
    /// "related to" without one and "owns" with it. Most relationships have
    /// none, and EMF omits the attribute rather than writing it empty.
    pub fn add_relation(
        &mut self,
        ty: RelType,
        source: ConceptId,
        target: ConceptId,
        access_type: Option<i64>,
        documentation: Option<&str>,
        name: Option<&str>,
    ) -> Result<ConceptId, EditError> {
        self.check_relationship(ty, source, target)?;
        if let Some(a) = access_type
            && !(0..=3).contains(&a)
        {
            return Err(EditError::BadAccessType(a));
        }

        let folder = self
            .top_folder(FolderType::Relations)
            .ok_or_else(|| EditError::NoSuchFolder("relations".to_string()))?;
        let (src_id, tgt_id) = (self.concept(source).id.clone(), self.concept(target).id.clone());
        let id = self.fresh_id(&["relation", ty.info().xsi, &src_id, &tgt_id]);

        // Archi's order: xsi:type, name, id, source, target — the name, when
        // there is one, sits where it does on an element.
        let mut b = NodeBuilder::new("element").attr("xsi:type", ty.info().xsi);
        if let Some(n) = name.filter(|n| !n.is_empty()) {
            b = b.attr("name", n);
        }
        b = b.attr("id", &*id).attr("source", &*src_id).attr("target", &*tgt_id);
        // Archi omits accessType when it equals the schema default of 0
        // (write); writing it explicitly would break byte identity against a
        // file Archi produced.
        if let Some(a) = access_type.filter(|a| *a != 0) {
            b = b.attr("accessType", a.to_string());
        }

        let folder_node = self.folder(folder).node;
        let node = self.doc.append_child(folder_node, b)?;
        if let Some(doc) = documentation.filter(|d| !d.is_empty()) {
            self.set_documentation_node(node, doc)?;
        }
        self.reindex();
        Ok(self.concept_by_id(&id).expect("just added"))
    }

    /// The full legality check, without performing the edit.
    pub fn check_relationship(
        &self,
        ty: RelType,
        source: ConceptId,
        target: ConceptId,
    ) -> Result<(), EditError> {
        let (s, t) = (self.concept(source), self.concept(target));

        // An unknown type has no matrix row, so the table cannot judge it. That
        // is a reason to allow the edit and let validation report it, not to
        // block work on a model this build does not fully understand.
        if let (Some(si), Some(ti)) = (s.kind.matrix_idx(), t.kind.matrix_idx())
            && !matrix::allows(si, ti, ty)
        {
            return Err(EditError::InvalidRelationship {
                rel: ty.info().short,
                source_type: s.kind.name().to_string(),
                target_type: t.kind.name().to_string(),
                permitted: matrix::permitted(si, ti).iter().map(|r| r.info().short).collect(),
            });
        }

        if let Some(existing) = self.concepts().find(|c| {
            c.kind == ConceptKind::Relationship(ty)
                && c.source.as_deref() == Some(s.id.as_str())
                && c.target.as_deref() == Some(t.id.as_str())
        }) {
            return Err(EditError::DuplicateRelationship {
                rel: ty.info().short,
                from: display_name(s),
                to: display_name(t),
                existing: existing.id.clone(),
            });
        }

        for end in [s, t] {
            if end.kind == ConceptKind::Element(ElementType::Junction)
                && let Some(other) = self.junction_rel_type(&end.id)
                && other != ty
            {
                return Err(EditError::MixedJunction(display_name(end), other.info().short));
            }
        }
        Ok(())
    }

    /// The relationship type already in use at a junction, if any.
    fn junction_rel_type(&self, junction_id: &str) -> Option<RelType> {
        self.concepts().find_map(|c| {
            let touches = c.source.as_deref() == Some(junction_id)
                || c.target.as_deref() == Some(junction_id);
            match (&c.kind, touches) {
                (ConceptKind::Relationship(r), true) => Some(*r),
                _ => None,
            }
        })
    }

    /// Create a folder under `parent`, or return the one already there.
    ///
    /// Two folders with the same path make `folder_by_path` a coin toss and
    /// show up in Archi as duplicates, so a repeat is the existing folder
    /// rather than a second one — which is also what makes a script that
    /// declares the folders it needs safe to run twice.
    pub fn add_folder(&mut self, parent: FolderId, name: &str) -> Result<FolderId, EditError> {
        if let Some(existing) = self
            .folders_with_ids()
            .find(|(_, f)| f.parent == Some(parent) && f.name == name)
            .map(|(i, _)| i)
        {
            return Ok(existing);
        }
        let parent_path = self.folder(parent).path.clone();
        let id = self.fresh_id(&["folder", &parent_path, name]);
        let parent_node = self.folder(parent).node;
        // Folders come before elements in a folder's children, as Archi writes
        // them; inserting at the end would still load, but the diff would move
        // things around on the next Archi save.
        let at = self
            .doc
            .children(parent_node)
            .take_while(|c| self.doc.local_name(*c) == "folder")
            .count();
        self.doc.insert_child(
            parent_node,
            at,
            NodeBuilder::new("folder").attr("name", name).attr("id", &*id),
        )?;
        self.reindex();
        Ok(self.folder_id_by_id(&id).expect("just added"))
    }

    /// Rename a folder. The top-level folders are Archi's and keep their names.
    ///
    /// Renaming used to take three commands — make the new folder, move
    /// everything into it, delete the old one — which also gave the folder a
    /// new id and moved every view's bytes in the file: a four-thousand-line
    /// diff for one word. This changes one attribute.
    pub fn rename_folder(&mut self, folder: FolderId, name: &str) -> Result<(), EditError> {
        if self.folder(folder).parent.is_none() {
            return Err(EditError::TopFolderFixed(self.folder(folder).path.clone()));
        }
        let node = self.folder(folder).node;
        self.doc.set_attr(node, "name", name);
        self.reindex();
        Ok(())
    }

    /// Re-file a folder, with everything in it, under another folder of the
    /// same tree. The node keeps its bytes, so the views and concepts inside
    /// move as one block and nothing in them is rewritten.
    pub fn move_folder(&mut self, folder: FolderId, parent: FolderId) -> Result<(), EditError> {
        let path = self.folder(folder).path.clone();
        if self.folder(folder).parent.is_none() {
            return Err(EditError::TopFolderFixed(path));
        }
        let dest = self.folder(parent).path.clone();
        if self.top_of(folder) != self.top_of(parent) {
            return Err(EditError::FolderTree(path, dest));
        }
        let mut at = Some(parent);
        while let Some(f) = at {
            if f == folder {
                return Err(EditError::FolderCycle(path, dest));
            }
            at = self.folder(f).parent;
        }
        let node = self.folder(folder).node;
        let target = self.folder(parent).node;
        if self.doc.parent(node) == Some(target) {
            return Ok(());
        }
        // Among the folders, before the elements, as `add_folder` places one.
        let slot = self
            .doc
            .children(target)
            .filter(|c| *c != node)
            .take_while(|c| self.doc.local_name(*c) == "folder")
            .count();
        self.doc.move_child(node, target, slot);
        self.reindex();
        Ok(())
    }

    /// The top-level folder a folder sits under.
    fn top_of(&self, folder: FolderId) -> FolderId {
        let mut f = folder;
        while let Some(p) = self.folder(f).parent {
            f = p;
        }
        f
    }

    // ---- changing -------------------------------------------------------

    pub fn rename(&mut self, c: ConceptId, name: &str) {
        let node = self.concept(c).node;
        self.doc.set_attr(node, "name", name);
        self.reindex();
    }

    pub fn set_documentation(&mut self, c: ConceptId, text: &str) -> Result<(), EditError> {
        let node = self.concept(c).node;
        self.set_documentation_node(node, text)
    }

    fn set_documentation_node(&mut self, node: NodeId, text: &str) -> Result<(), EditError> {
        match self.doc.child_named(node, "documentation") {
            Some(d) if text.is_empty() => self.doc.remove_subtree(d),
            Some(d) => self.doc.set_text(d, text)?,
            None if text.is_empty() => {}
            None => {
                let at = self.slot_for(node, "documentation");
                self.doc.insert_child(node, at, NodeBuilder::new("documentation").text(text))?;
            }
        }
        Ok(())
    }

    /// Where a new child of this kind goes among a node's children, in the
    /// order Archi writes them.
    ///
    /// EMF serialises containment in metamodel order, and the metamodel lists
    /// a view's supertypes as `DiagramModelContainer`, then `Documentable`,
    /// then `Properties` — so a view's `<documentation>` comes *after* its
    /// `<child>` objects and before its `<property>` lines, while on a concept
    /// it comes first, after any `<feature>`. One rank per kind, and a new
    /// node lands after the last sibling that ranks no higher than it, so
    /// the file reads the same whether the documentation was written before
    /// the objects or after them — a batch may do either, and Archi's next
    /// save would otherwise move the line.
    fn slot_for(&self, parent: NodeId, kind: &str) -> usize {
        fn rank(name: &str) -> u8 {
            match name {
                "feature" => 0,
                "documentation" => 2,
                "property" => 3,
                // `child`, and anything this build does not know: content.
                _ => 1,
            }
        }
        let want = rank(kind);
        self.doc
            .children(parent)
            .enumerate()
            .filter(|(_, c)| rank(self.doc.local_name(*c)) <= want)
            .map(|(i, _)| i + 1)
            .last()
            .unwrap_or(0)
    }

    pub fn set_property(&mut self, c: ConceptId, key: &str, value: &str) -> Result<(), EditError> {
        let node = self.concept(c).node;
        self.set_property_node(node, key, value)
    }

    /// Set a property on a view. A view carries properties exactly as a
    /// concept does; this is what lets `--replace` keep them.
    pub fn set_view_property(
        &mut self,
        view: ViewId,
        key: &str,
        value: &str,
    ) -> Result<(), EditError> {
        let node = self.view(view).node;
        self.set_property_node(node, key, value)
    }

    fn set_property_node(&mut self, node: NodeId, key: &str, value: &str) -> Result<(), EditError> {
        let existing = self
            .doc
            .children(node)
            .filter(|n| self.doc.local_name(*n) == "property")
            .find(|n| self.doc.attr(*n, "key").as_deref() == Some(key));
        match existing {
            Some(p) => self.doc.set_attr(p, "value", value),
            None => {
                let at = self.doc.children(node).count();
                self.doc.insert_child(
                    node,
                    at,
                    NodeBuilder::new("property").attr("key", key).attr("value", value),
                )?;
            }
        }
        Ok(())
    }

    pub fn remove_property(&mut self, c: ConceptId, key: &str) {
        let node = self.concept(c).node;
        let found: Vec<NodeId> = self
            .doc
            .children(node)
            .filter(|n| {
                self.doc.local_name(*n) == "property"
                    && self.doc.attr(*n, "key").as_deref() == Some(key)
            })
            .collect();
        for p in found {
            self.doc.remove_subtree(p);
        }
        self.reindex();
    }

    /// Re-file a concept. The node itself moves, keeping its own bytes, so
    /// unknown attributes and unknown children survive the trip — rebuilding it
    /// from the fields we understand would quietly drop them.
    pub fn move_to_folder(&mut self, c: ConceptId, folder: FolderId) -> Result<(), EditError> {
        let node = self.concept(c).node;
        let target = self.folder(folder).node;
        if self.doc.parent(node) == Some(target) {
            return Ok(());
        }
        let at = self.doc.children(target).count();
        self.doc.move_child(node, target, at);
        self.reindex();
        Ok(())
    }

    // ---- deleting -------------------------------------------------------

    /// Remove an empty folder.
    ///
    /// Empty is the whole contract: deleting a folder that holds concepts or
    /// views would be a cascading delete wearing a filing operation's clothes,
    /// and there is already a command for deleting things. Refusing keeps this
    /// usable for the one job it is for — tidying folders that should not have
    /// been made.
    pub fn delete_folder(&mut self, folder: FolderId) -> Result<(), EditError> {
        let node = self.folder(folder).node;
        let held = self.doc.children(node).count();
        if held > 0 {
            return Err(EditError::FolderNotEmpty(self.folder(folder).path.clone(), held));
        }
        if self.folder(folder).parent.is_none() {
            return Err(EditError::TopFolder(self.folder(folder).path.clone()));
        }
        self.doc.remove_subtree(node);
        self.reindex();
        Ok(())
    }

    /// Everything a delete would remove, computed without changing anything.
    ///
    /// The visual half is what the old Python tool skipped, and skipping it is
    /// what left models Archi refused to open: a diagram object whose
    /// `archimateElement` no longer resolves is a load error, not a cosmetic
    /// problem.
    pub fn delete_plan(&self, c: ConceptId) -> Cascade {
        let mut plan = Cascade::default();
        let root = self.concept(c);
        plan.concepts.push(root.id.clone());

        // Relationships fall transitively: a relationship may itself be the
        // endpoint of an association.
        let mut doomed_concepts: Vec<String> = vec![root.id.clone()];
        let mut i = 0;
        while i < doomed_concepts.len() {
            let victim = doomed_concepts[i].clone();
            i += 1;
            for rel in self.concepts().filter(|r| r.kind.is_relationship()) {
                if plan.relationships.contains(&rel.id) {
                    continue;
                }
                if rel.source.as_deref() == Some(victim.as_str())
                    || rel.target.as_deref() == Some(victim.as_str())
                {
                    plan.relationships.push(rel.id.clone());
                    doomed_concepts.push(rel.id.clone());
                }
            }
        }

        let gone: std::collections::HashSet<&str> =
            doomed_concepts.iter().map(String::as_str).collect();

        // Now the visuals, view by view.
        for view in self.views() {
            let mut dead_visuals: std::collections::HashSet<String> = Default::default();

            for n in self.doc.descendants(view.node) {
                let local = self.doc.local_name(n);
                let id = self.doc.attr(n, "id").unwrap_or_default();
                let refers =
                    |attr: &str| self.doc.attr(n, attr).is_some_and(|v| gone.contains(v.as_str()));
                let dies = match local {
                    "child" => refers("archimateElement"),
                    "sourceConnection" => refers("archimateRelationship"),
                    _ => false,
                };
                if dies {
                    dead_visuals.insert(id);
                }
            }

            self.expand_dead_visuals(view.node, &mut dead_visuals);
            let (objects, connections) = self.classify_visuals(view.node, &dead_visuals);
            if !objects.is_empty() || !connections.is_empty() {
                plan.views.push(view.id.clone());
            }
            plan.diagram_objects.extend(objects);
            plan.connections.extend(connections);
        }

        // A junction left with fewer than two connections no longer joins
        // anything, but removing it is a modelling decision.
        for concept in self.concepts() {
            if concept.kind != ConceptKind::Element(ElementType::Junction)
                || gone.contains(concept.id.as_str())
            {
                continue;
            }
            let left = self
                .concepts()
                .filter(|r| {
                    r.kind.is_relationship()
                        && !plan.relationships.contains(&r.id)
                        && (r.source.as_deref() == Some(concept.id.as_str())
                            || r.target.as_deref() == Some(concept.id.as_str()))
                })
                .count();
            if left < 2 {
                plan.degenerate_junctions.push(display_name(concept));
            }
        }

        plan
    }

    /// Grow a set of doomed visuals to its fixpoint inside one view.
    ///
    /// Iterating rather than sweeping once is the point: a removed diagram
    /// object takes its connections, and one of those may have been the last
    /// thing holding another object's connection.
    fn expand_dead_visuals(&self, view_node: NodeId, dead: &mut std::collections::HashSet<String>) {
        loop {
            let before = dead.len();
            for n in self.doc.descendants(view_node) {
                let id = self.doc.attr(n, "id").unwrap_or_default();
                if id.is_empty() || dead.contains(&id) {
                    continue;
                }
                let doomed_parent = self
                    .doc
                    .parent(n)
                    .and_then(|p| self.doc.attr(p, "id"))
                    .is_some_and(|p| dead.contains(&p));
                let endpoint_gone = self.doc.local_name(n) == "sourceConnection"
                    && ["source", "target"]
                        .iter()
                        .any(|a| self.doc.attr(n, a).is_some_and(|v| dead.contains(&v)));
                if doomed_parent || endpoint_gone {
                    dead.insert(id);
                }
            }
            if dead.len() == before {
                return;
            }
        }
    }

    /// Split a set of doomed visual ids into (objects, connections), in
    /// document order.
    fn classify_visuals(
        &self,
        view_node: NodeId,
        dead: &std::collections::HashSet<String>,
    ) -> (Vec<String>, Vec<String>) {
        let (mut objects, mut connections) = (Vec::new(), Vec::new());
        for n in self.doc.descendants(view_node) {
            let Some(id) = self.doc.attr(n, "id") else { continue };
            if !dead.contains(&id) {
                continue;
            }
            match self.doc.local_name(n) {
                "child" => objects.push(id),
                "sourceConnection" => connections.push(id),
                _ => {}
            }
        }
        (objects, connections)
    }

    /// Delete a concept and everything the plan says goes with it.
    pub fn delete_concept(&mut self, c: ConceptId) -> Result<Cascade, EditError> {
        let plan = self.delete_plan(c);

        let mut nodes: Vec<NodeId> = Vec::new();
        for id in plan.concepts.iter().chain(plan.relationships.iter()) {
            if let Some(cid) = self.concept_by_id(id) {
                nodes.push(self.concept(cid).node);
            }
        }
        let visual_ids: std::collections::HashSet<&str> = plan
            .diagram_objects
            .iter()
            .chain(plan.connections.iter())
            .map(String::as_str)
            .collect();
        for view in self.views() {
            for n in self.doc.descendants(view.node) {
                if self.doc.attr(n, "id").is_some_and(|id| visual_ids.contains(id.as_str())) {
                    nodes.push(n);
                }
            }
        }
        for n in nodes {
            self.doc.remove_subtree(n);
        }

        // `targetConnections` is a derived mirror of the connections that point
        // at an object. Recomputing it, rather than patching it, removes an
        // entire class of corruption that Archi tolerates in memory but chokes
        // on at load.
        for view_id in &plan.views {
            if let Some(v) = self.view_by_id(view_id) {
                self.recompute_target_connections(v);
            }
        }

        self.reindex();
        Ok(plan)
    }

    /// Rebuild every `targetConnections` in a view from the connections that
    /// actually exist.
    pub fn recompute_target_connections(&mut self, view: ViewId) {
        let node = self.view(view).node;
        let mut incoming: std::collections::HashMap<String, Vec<String>> = Default::default();
        for n in self.doc.descendants(node) {
            if self.doc.local_name(n) != "sourceConnection" {
                continue;
            }
            let (Some(id), Some(target)) = (self.doc.attr(n, "id"), self.doc.attr(n, "target"))
            else {
                continue;
            };
            incoming.entry(target).or_default().push(id);
        }

        let objects: Vec<NodeId> = self
            .doc
            .descendants(node)
            .into_iter()
            .filter(|n| self.doc.local_name(*n) == "child")
            .collect();
        for obj in objects {
            let Some(id) = self.doc.attr(obj, "id") else { continue };
            match incoming.get(&id) {
                Some(list) => self.doc.set_attr(obj, "targetConnections", &list.join(" ")),
                // EMF omits an empty IDREFS attribute entirely.
                None => self.doc.remove_attr(obj, "targetConnections"),
            }
        }
    }
}

fn display_name(c: &Concept) -> String {
    if c.name.is_empty() { c.id.clone() } else { c.name.clone() }
}

// ---- retyping and merging --------------------------------------------------

impl Model {
    /// The relationships that the matrix would refuse if `c` were of type
    /// `to`. Empty means the retype is legal; nothing is changed either way.
    ///
    /// A relationship, a junction and a junction-to-be are refused outright:
    /// the first is not an element, and a junction's one rule — every
    /// relationship through it shares a type — is about the relationships,
    /// not the element.
    pub fn retype_check(
        &self,
        c: ConceptId,
        to: ElementType,
    ) -> Result<Vec<IllegalRelationship>, EditError> {
        let e = self.concept(c);
        if e.kind.is_relationship() {
            return Err(EditError::NotAnElement(display_name(e), "retyped"));
        }
        if e.kind == ConceptKind::Element(ElementType::Junction) || to == ElementType::Junction {
            return Err(EditError::JunctionFixed(display_name(e), "retyped"));
        }
        Ok(self.illegal_relationships(&e.id, to.info().matrix_idx, &|_| false))
    }

    /// Change an element's type in place: same id, name, documentation,
    /// properties, relationships and diagram objects — only `xsi:type`
    /// changes, and the element moves to the top-level folder Archi keeps
    /// that type in when it is not already somewhere under it. Returns
    /// whether it moved.
    ///
    /// Refused, with every relationship that would become illegal listed,
    /// before anything is touched: the caller fixes those first. There is
    /// deliberately no way to force it, because a relationship the standard
    /// forbids is exactly what `validate` exists to catch.
    pub fn retype(&mut self, c: ConceptId, to: ElementType) -> Result<bool, EditError> {
        let illegal = self.retype_check(c, to)?;
        if !illegal.is_empty() {
            return Err(EditError::IllegalRelationships(illegal));
        }
        let home = self
            .top_folder(to.info().home)
            .ok_or_else(|| EditError::NoSuchFolder(to.info().home.as_str().to_string()))?;
        let node = self.concept(c).node;
        self.doc.set_attr(node, "xsi:type", to.info().xsi);
        // A user subfolder under the right top folder is where the author
        // filed it; only a folder of another tree is wrong for the new type.
        let moved = self.top_of(self.concept(c).folder) != home;
        if moved {
            self.move_to_folder(c, home)?;
        }
        self.reindex();
        Ok(moved)
    }

    /// The relationships that the matrix would refuse once every one of
    /// `merged`'s stood at `into` instead. Empty means the merge is legal;
    /// nothing is changed either way.
    pub fn merge_check(
        &self,
        merged: ConceptId,
        into: ConceptId,
    ) -> Result<Vec<IllegalRelationship>, EditError> {
        let (a, b) = (self.concept(merged), self.concept(into));
        for e in [a, b] {
            if e.kind.is_relationship() {
                return Err(EditError::NotAnElement(display_name(e), "merged"));
            }
            if e.kind == ConceptKind::Element(ElementType::Junction) {
                return Err(EditError::JunctionFixed(display_name(e), "merged"));
            }
        }
        if merged == into {
            return Err(EditError::SameElement(display_name(a)));
        }
        // An unknown survivor has no matrix row; as in `check_relationship`,
        // that is a reason to let validation report it, not to block.
        let Some(idx) = b.kind.matrix_idx() else { return Ok(Vec::new()) };
        let (aid, bid) = (a.id.as_str(), b.id.as_str());
        // A relationship between the two is deleted, not repointed, so it is
        // not judged.
        let between = |r: &Concept| {
            let (s, t) = (r.source.as_deref(), r.target.as_deref());
            (s == Some(aid) && t == Some(bid)) || (s == Some(bid) && t == Some(aid))
        };
        Ok(self.illegal_relationships(aid, idx, &between))
    }

    /// Every relationship touching `id` — except those `skip` says so of —
    /// that the matrix would refuse once that end stood for a concept with
    /// matrix index `idx`: the type it is being retyped to, or the survivor
    /// it is being merged into. A self-loop has both ends replaced.
    fn illegal_relationships(
        &self,
        id: &str,
        idx: u8,
        skip: &dyn Fn(&Concept) -> bool,
    ) -> Vec<IllegalRelationship> {
        let mut out = Vec::new();
        for r in self.concepts() {
            let ConceptKind::Relationship(ty) = &r.kind else { continue };
            let (Some(s), Some(t)) = (r.source.as_deref(), r.target.as_deref()) else { continue };
            if (s != id && t != id) || skip(r) {
                continue;
            }
            let end_idx = |end: &str| {
                if end == id {
                    Some(idx)
                } else {
                    self.concept_by_id(end).and_then(|c| self.concept(c).kind.matrix_idx())
                }
            };
            // An end this build does not know has no row to judge by.
            let (Some(si), Some(ti)) = (end_idx(s), end_idx(t)) else { continue };
            if matrix::allows(si, ti, *ty) {
                continue;
            }
            let (direction, other) = if s == id { ("out", t) } else { ("in", s) };
            let other_c = self.concept_by_id(other).map(|c| self.concept(c));
            out.push(IllegalRelationship {
                id: r.id.clone(),
                rel: ty.info().short,
                direction,
                other: other.to_string(),
                other_name: other_c.map(|c| c.name.clone()).unwrap_or_default(),
                other_type: other_c.map(|c| c.kind.name().to_string()).unwrap_or_default(),
                permitted: matrix::permitted(si, ti).iter().map(|r| r.info().short).collect(),
            });
        }
        out
    }

    /// Fold `merged` into `into`, which means the same thing, and delete it.
    ///
    /// Every relationship at the merged element is repointed to the survivor.
    /// One that would then be the twin of a relationship the survivor already
    /// has — same type, same other end, same direction — is deleted instead,
    /// and the lines that drew it are handed to the twin, so no drawing loses
    /// a line over a duplicate. A relationship between the two elements
    /// would become a self-loop and is deleted with its lines. Every box that
    /// showed the merged element shows the survivor, exactly where it was and
    /// styled as it was; a view that now shows the survivor twice is allowed,
    /// as Archi allows it, and named in the result.
    ///
    /// The survivor keeps its documentation; the merged element's, when
    /// `keep_doc` and it says something different, is appended as a new
    /// paragraph. Properties the survivor lacks are copied and ones it has
    /// keep its value — except `src` and `source`, whose differing values
    /// are gathered into `supporting-source`, `;`-separated.
    ///
    /// Refused before anything changes when a repointed relationship would
    /// break the matrix; the error lists each one.
    pub fn merge_elements(
        &mut self,
        merged: ConceptId,
        into: ConceptId,
        keep_doc: bool,
    ) -> Result<Merged, EditError> {
        let illegal = self.merge_check(merged, into)?;
        if !illegal.is_empty() {
            return Err(EditError::IllegalRelationships(illegal));
        }
        let (aid, bid) = (self.concept(merged).id.clone(), self.concept(into).id.clone());
        let (a_node, b_node) = (self.concept(merged).node, self.concept(into).node);
        let mut done = Merged::default();

        // Relationships. `held` is what the survivor has, including what this
        // loop hands it, so two twins arriving together collapse to one.
        let mut held: Vec<(RelType, String, String, String)> = self
            .concepts()
            .filter_map(|r| {
                let ConceptKind::Relationship(ty) = &r.kind else { return None };
                let (s, t) = (r.source.clone()?, r.target.clone()?);
                (s == bid || t == bid).then(|| (*ty, s, t, r.id.clone()))
            })
            .collect();
        let touching: Vec<ConceptId> = self
            .concepts_with_ids()
            .filter(|(_, r)| {
                r.kind.is_relationship()
                    && (r.source.as_deref() == Some(&aid) || r.target.as_deref() == Some(&aid))
            })
            .map(|(i, _)| i)
            .collect();
        // Deleted once everything else is in place: (relationship, the twin
        // its lines go to — none for a would-be self-loop).
        let mut doomed: Vec<(ConceptId, Option<String>)> = Vec::new();
        for r in touching {
            let rc = self.concept(r);
            let (Some(s), Some(t)) = (rc.source.clone(), rc.target.clone()) else { continue };
            if (s == aid && t == bid) || (s == bid && t == aid) {
                doomed.push((r, None));
                continue;
            }
            let ns = if s == aid { bid.clone() } else { s };
            let nt = if t == aid { bid.clone() } else { t };
            let ty = match &rc.kind {
                ConceptKind::Relationship(ty) => Some(*ty),
                _ => None,
            };
            if let Some(ty) = ty
                && let Some((_, _, _, twin)) =
                    held.iter().find(|(k, hs, ht, _)| *k == ty && *hs == ns && *ht == nt)
            {
                doomed.push((r, Some(twin.clone())));
                continue;
            }
            let (node, id) = (rc.node, rc.id.clone());
            self.doc.set_attr(node, "source", &ns);
            self.doc.set_attr(node, "target", &nt);
            if let Some(ty) = ty {
                held.push((ty, ns, nt, id.clone()));
            }
            done.relationships.push(id);
        }

        // Boxes: the object stays, with its bounds, style and connections;
        // only what it stands for changes.
        let views: Vec<(ViewId, String)> =
            self.views_with_ids().map(|(v, view)| (v, view.name.clone())).collect();
        for (v, name) in views {
            let view_node = self.view(v).node;
            let objects: Vec<NodeId> = self
                .doc
                .descendants(view_node)
                .into_iter()
                .filter(|n| self.doc.local_name(*n) == "child")
                .collect();
            let mut repointed = false;
            for n in &objects {
                if self.doc.attr(*n, "archimateElement").as_deref() == Some(&aid) {
                    self.doc.set_attr(*n, "archimateElement", &bid);
                    done.objects.extend(self.doc.attr(*n, "id"));
                    repointed = true;
                }
            }
            let shown = objects
                .iter()
                .filter(|n| self.doc.attr(**n, "archimateElement").as_deref() == Some(&bid))
                .count();
            if repointed && shown > 1 {
                done.duplicated_on.push(name);
            }
        }

        // A dropped twin's lines draw the twin that stays.
        for (r, twin) in &doomed {
            let Some(twin) = twin else { continue };
            let dropped_id = self.concept(*r).id.clone();
            let lines: Vec<NodeId> = self
                .views()
                .flat_map(|v| self.doc.descendants(v.node))
                .filter(|n| {
                    self.doc.local_name(*n) == "sourceConnection"
                        && self.doc.attr(*n, "archimateRelationship").as_deref()
                            == Some(&dropped_id)
                })
                .collect();
            for n in lines {
                self.doc.set_attr(n, "archimateRelationship", twin);
            }
        }

        // Documentation and properties, before the merged node goes.
        if keep_doc && let Some(extra) = self.documentation(a_node).filter(|d| !d.is_empty()) {
            let own = self.documentation(b_node).unwrap_or_default();
            if own != extra {
                let text = if own.is_empty() { extra } else { format!("{own}\n\n{extra}") };
                self.set_documentation_node(b_node, &text)?;
            }
        }
        let mut ours = self.properties(b_node);
        for (k, v) in self.properties(a_node) {
            match ours.iter().find(|(ok, _)| *ok == k).map(|(_, ov)| ov.clone()) {
                None => {
                    self.set_property_node(b_node, &k, &v)?;
                    ours.push((k, v));
                }
                Some(cur) if cur != v && (k == "src" || k == "source") => {
                    let have = ours
                        .iter()
                        .find(|(ok, _)| ok == "supporting-source")
                        .map(|(_, ov)| ov.clone())
                        .unwrap_or_default();
                    if have.split(';').any(|s| s == v) {
                        continue;
                    }
                    let joined = if have.is_empty() { v } else { format!("{have};{v}") };
                    self.set_property_node(b_node, "supporting-source", &joined)?;
                    ours.retain(|(ok, _)| ok != "supporting-source");
                    ours.push(("supporting-source".to_string(), joined));
                }
                Some(_) => {}
            }
        }

        // The index still says the repointed relationships touch the merged
        // element; rebuilt, the deletes below take only what they should.
        self.reindex();
        for (r, _) in doomed {
            done.dropped.push(self.concept(r).id.clone());
            self.delete_concept(r)?;
        }
        self.delete_concept(merged)?;
        Ok(done)
    }
}

// ---- styling ---------------------------------------------------------------

/// A change to how one object or line on a view looks. `None` leaves a
/// setting as it is; `Some("")` clears it back to Archi's default, which is
/// the absent attribute.
///
/// Values are Archi's own encodings — a colour is `#rrggbb`, a font is the
/// string Archi writes (`1|Arial|19.0|1|COCOA|1|`), alignment is 1, 2 or 4,
/// text position 0, 1 or 2, border type 0, 1 or 2 — because they are stored
/// verbatim and a poster exported and applied again must come back the same.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StyleChange {
    pub fill: Option<String>,
    pub line: Option<String>,
    pub line_width: Option<String>,
    pub font: Option<String>,
    pub font_color: Option<String>,
    pub text_align: Option<String>,
    pub text_position: Option<String>,
    pub border: Option<String>,
    pub alpha: Option<String>,
    pub line_alpha: Option<String>,
    /// The `labelExpression` feature: what is written on the figure or
    /// the line instead of its name.
    pub label: Option<String>,
    /// The `iconVisible` feature: `2` hides the type icon, `1` always
    /// shows it, empty is Archi's default.
    pub icon: Option<String>,
    /// The `deriveElementLineColor` feature: `false` makes Archi draw an
    /// element's explicit `lineColor`, which it otherwise ignores in favour
    /// of a border derived from the fill; `true` or empty is that default.
    pub line_derived: Option<String>,
}

/// What a visual on a view is, when a caller names one by id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visual {
    Object(NodeId),
    Connection(NodeId),
}

impl Model {
    /// The object or connection with this id on the view.
    pub fn visual(&self, view: ViewId, id: &str) -> Result<Visual, EditError> {
        let view_node = self.view(view).node;
        for n in self.doc.descendants(view_node) {
            if self.doc.attr(n, "id").as_deref() != Some(id) {
                continue;
            }
            match self.doc.local_name(n) {
                "child" => return Ok(Visual::Object(n)),
                "sourceConnection" => return Ok(Visual::Connection(n)),
                _ => {}
            }
        }
        Err(EditError::NoSuchObject(id.to_string(), self.view(view).name.clone()))
    }

    /// The connections on a view that draw a relationship, in document order.
    pub fn view_connections_of(&self, view: ViewId, relationship_id: &str) -> Vec<String> {
        let view_node = self.view(view).node;
        self.doc
            .descendants(view_node)
            .into_iter()
            .filter(|n| self.doc.local_name(*n) == "sourceConnection")
            .filter(|n| {
                self.doc.attr(*n, "archimateRelationship").as_deref() == Some(relationship_id)
            })
            .filter_map(|n| self.doc.attr(n, "id"))
            .collect()
    }

    /// Change how an object or a line looks. Returns the settings it
    /// touched, by Archi's attribute names.
    ///
    /// A cleared setting is the attribute removed, not written empty: EMF
    /// omits a default, and an empty `fillColor=""` is a colour Archi
    /// refuses to parse.
    pub fn style_visual(
        &mut self,
        view: ViewId,
        id: &str,
        change: &StyleChange,
    ) -> Result<Vec<&'static str>, EditError> {
        let node = match self.visual(view, id)? {
            Visual::Object(n) | Visual::Connection(n) => n,
        };
        let mut touched = Vec::new();
        let attrs: [(&'static str, &Option<String>); 10] = [
            ("fillColor", &change.fill),
            ("lineColor", &change.line),
            ("lineWidth", &change.line_width),
            ("font", &change.font),
            ("fontColor", &change.font_color),
            ("textAlignment", &change.text_align),
            ("textPosition", &change.text_position),
            ("borderType", &change.border),
            ("alpha", &change.alpha),
            ("lineAlpha", &change.line_alpha),
        ];
        for (name, value) in attrs {
            let Some(v) = value else { continue };
            if v.is_empty() {
                self.doc.remove_attr(node, name);
            } else {
                self.doc.set_attr(node, name, v);
            }
            touched.push(name);
        }
        for (name, value) in [
            ("labelExpression", &change.label),
            ("iconVisible", &change.icon),
            ("deriveElementLineColor", &change.line_derived),
        ] {
            let Some(v) = value else { continue };
            self.set_feature(node, name, v)?;
            touched.push(name);
        }
        Ok(touched)
    }

    /// Set, replace or (with an empty value) remove a `<feature>` on a
    /// diagram object or connection, where Archi keeps them: after the
    /// bounds, before the connections and the nested objects.
    fn set_feature(&mut self, node: NodeId, name: &str, value: &str) -> Result<(), EditError> {
        let existing = self
            .doc
            .children(node)
            .filter(|c| self.doc.local_name(*c) == "feature")
            .find(|c| self.doc.attr(*c, "name").as_deref() == Some(name));
        match (existing, value.is_empty()) {
            (Some(f), true) => self.doc.remove_subtree(f),
            (Some(f), false) => self.doc.set_attr(f, "value", value),
            (None, true) => {}
            (None, false) => {
                let at = self.object_slot_for(node, "feature");
                self.doc.insert_child(
                    node,
                    at,
                    NodeBuilder::new("feature").attr("name", name).attr("value", value),
                )?;
            }
        }
        Ok(())
    }

    /// The style an object or line carries, in the same shape a change is
    /// given, so that what `export views` writes is what `view.style` reads.
    pub fn visual_style(&self, view: ViewId, id: &str) -> Result<StyleChange, EditError> {
        let node = match self.visual(view, id)? {
            Visual::Object(n) | Visual::Connection(n) => n,
        };
        let a = |k: &str| self.doc.attr(node, k);
        let feature =
            |k: &str| self.features(node).into_iter().find(|(n, _)| n == k).map(|(_, v)| v);
        Ok(StyleChange {
            fill: a("fillColor"),
            line: a("lineColor"),
            line_width: a("lineWidth"),
            font: a("font"),
            font_color: a("fontColor"),
            text_align: a("textAlignment"),
            text_position: a("textPosition"),
            border: a("borderType"),
            alpha: a("alpha"),
            line_alpha: a("lineAlpha"),
            label: feature("labelExpression"),
            icon: feature("iconVisible"),
            line_derived: feature("deriveElementLineColor"),
        })
    }

    /// The bendpoints stored on a connection, as Archi's relative offsets.
    pub fn view_connection_bendpoints(
        &self,
        view: ViewId,
        id: &str,
    ) -> Result<Vec<(i32, i32, i32, i32)>, EditError> {
        let Visual::Connection(n) = self.visual(view, id)? else {
            return Err(EditError::NoSuchObject(id.to_string(), self.view(view).name.clone()));
        };
        let num =
            |c: NodeId, k: &str| self.doc.attr(c, k).and_then(|v| v.parse().ok()).unwrap_or(0);
        Ok(self
            .doc
            .children(n)
            .filter(|c| self.doc.local_name(*c) == "bendpoint")
            .map(|c| (num(c, "startX"), num(c, "startY"), num(c, "endX"), num(c, "endY")))
            .collect())
    }

    /// The bounds an object stores: relative to its container, `-1` for a
    /// default size.
    pub fn view_object_bounds(
        &self,
        view: ViewId,
        id: &str,
    ) -> Result<(i32, i32, i32, i32), EditError> {
        let Visual::Object(n) = self.visual(view, id)? else {
            return Err(EditError::NoSuchObject(id.to_string(), self.view(view).name.clone()));
        };
        let b = self.doc.child_named(n, "bounds");
        let num = |k: &str, d: i32| {
            b.and_then(|b| self.doc.attr(b, k)).and_then(|v| v.parse().ok()).unwrap_or(d)
        };
        Ok((num("x", 0), num("y", 0), num("width", -1), num("height", -1)))
    }
}

/// One object on a view, as [`Model::view_tree`] lists them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewObject {
    pub id: String,
    /// `DiagramObject`, `Group`, `Note`, `DiagramModelReference`, …
    pub kind: String,
    /// The concept an element's box shows; `None` for a group or a note.
    pub concept: Option<String>,
    /// The object this one is nested in; `None` at the top of the view.
    pub parent: Option<String>,
    /// A group's name, a note's text; empty for an element's box.
    pub text: String,
}

// ---- views ---------------------------------------------------------------

impl Model {
    /// Create an empty view in the Views folder.
    pub fn add_view(&mut self, name: &str, viewpoint: Option<&str>) -> Result<ViewId, EditError> {
        let folder = self
            .top_folder(FolderType::Diagrams)
            .ok_or_else(|| EditError::NoSuchFolder("diagrams".to_string()))?;
        let id = self.fresh_id(&["view", name]);
        let mut b = NodeBuilder::new("element")
            .attr("xsi:type", "archimate:ArchimateDiagramModel")
            .attr("name", name)
            .attr("id", &*id);
        // An empty viewpoint means "no viewpoint", and EMF omits it.
        if let Some(v) = viewpoint.filter(|v| !v.is_empty()) {
            b = b.attr("viewpoint", v);
        }
        let folder_node = self.folder(folder).node;
        self.doc.append_child(folder_node, b)?;
        self.reindex();
        Ok(self.view_by_id(&id).expect("just added"))
    }

    /// Put a concept on a view at the given bounds, returning the new diagram
    /// object's id.
    ///
    /// A concept may legitimately appear on a view more than once, so this does
    /// not deduplicate; callers that want at-most-once should check first.
    pub fn add_view_object(
        &mut self,
        view: ViewId,
        concept: ConceptId,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    ) -> Result<String, EditError> {
        self.add_view_object_in(view, concept, None, x, y, w, h)
    }

    /// [`Self::add_view_object`], nested inside another object on the view
    /// when `into` names one.
    ///
    /// Nesting is how Archi draws containment: a box inside a box stands for
    /// the relationship between them, and the line is not drawn. The bounds
    /// are relative to the container, as Archi stores them.
    #[allow(clippy::too_many_arguments)] // one per coordinate, as the CLI passes them
    pub fn add_view_object_in(
        &mut self,
        view: ViewId,
        concept: ConceptId,
        into: Option<&str>,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    ) -> Result<String, EditError> {
        let concept_id = self.concept(concept).id.clone();
        let view_id = self.view(view).id.clone();
        let id = self.fresh_id(&["object", &view_id, &concept_id]);
        let b = NodeBuilder::new("child")
            .attr("xsi:type", "archimate:DiagramObject")
            .attr("id", &*id)
            .attr("archimateElement", &*concept_id);
        self.insert_object(view, into, b, x, y, w, h)?;
        Ok(id)
    }

    /// Put a Group — a titled box that holds other objects and stands for no
    /// concept — on a view, returning its id.
    #[allow(clippy::too_many_arguments)]
    pub fn add_view_group(
        &mut self,
        view: ViewId,
        name: &str,
        into: Option<&str>,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    ) -> Result<String, EditError> {
        let view_id = self.view(view).id.clone();
        let id = self.fresh_id(&["group", &view_id, name]);
        let b = NodeBuilder::new("child")
            .attr("xsi:type", "archimate:Group")
            .attr("id", &*id)
            .attr("name", name);
        self.insert_object(view, into, b, x, y, w, h)?;
        Ok(id)
    }

    /// Put a Note — free text on the canvas — on a view, returning its id.
    #[allow(clippy::too_many_arguments)]
    pub fn add_view_note(
        &mut self,
        view: ViewId,
        text: &str,
        into: Option<&str>,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    ) -> Result<String, EditError> {
        let view_id = self.view(view).id.clone();
        let id = self.fresh_id(&["note", &view_id, text]);
        let b = NodeBuilder::new("child").attr("xsi:type", "archimate:Note").attr("id", &*id);
        let obj = self.insert_object(view, into, b, x, y, w, h)?;
        if !text.is_empty() {
            let at = self.object_slot_for(obj, "content");
            self.doc.insert_child(obj, at, NodeBuilder::new("content").text(text))?;
        }
        self.reindex();
        Ok(id)
    }

    /// Replace a Note's text. An empty string removes it.
    pub fn set_note_content(
        &mut self,
        view: ViewId,
        object_id: &str,
        text: &str,
    ) -> Result<(), EditError> {
        let obj = self.object_node(view, object_id)?;
        match self.doc.child_named(obj, "content") {
            Some(c) if text.is_empty() => self.doc.remove_subtree(c),
            Some(c) => self.doc.set_text(c, text)?,
            None if text.is_empty() => {}
            None => {
                let at = self.object_slot_for(obj, "content");
                self.doc.insert_child(obj, at, NodeBuilder::new("content").text(text))?;
            }
        }
        Ok(())
    }

    /// The node that is `obj`, or the view itself for `None`, checked to be
    /// something that can hold objects: an element's box or a Group. A Note
    /// holds text, not boxes, and a connection holds nothing.
    fn container_node(&self, view: ViewId, into: Option<&str>) -> Result<NodeId, EditError> {
        let Some(id) = into else { return Ok(self.view(view).node) };
        let n = self.object_node(view, id)?;
        let xsi = self.doc.attr(n, "xsi:type").unwrap_or_default();
        match xsi.trim_start_matches("archimate:") {
            "DiagramObject" | "Group" => Ok(n),
            _ => Err(EditError::NotAContainer(id.to_string())),
        }
    }

    /// A diagram object on a view, by id.
    fn object_node(&self, view: ViewId, object_id: &str) -> Result<NodeId, EditError> {
        let view_node = self.view(view).node;
        self.doc
            .descendants(view_node)
            .into_iter()
            .find(|n| {
                self.doc.local_name(*n) == "child"
                    && self.doc.attr(*n, "id").as_deref() == Some(object_id)
            })
            .ok_or_else(|| {
                EditError::NoSuchObject(object_id.to_string(), self.view(view).name.clone())
            })
    }

    /// Create an object under the view or inside a container, with its bounds.
    #[allow(clippy::too_many_arguments)]
    fn insert_object(
        &mut self,
        view: ViewId,
        into: Option<&str>,
        b: NodeBuilder,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    ) -> Result<NodeId, EditError> {
        let parent = self.container_node(view, into)?;
        // After the last object, and before the view's documentation and
        // properties; inside an object, after its connections: see
        // `slot_for` and `object_slot_for`.
        let at = if into.is_some() {
            self.object_slot_for(parent, "child")
        } else {
            self.slot_for(parent, "child")
        };
        let obj = self.doc.insert_child(parent, at, b)?;
        // `<bounds>` is a child element, not attributes on the object.
        self.doc.append_child(
            obj,
            NodeBuilder::new("bounds")
                .attr("x", x.to_string())
                .attr("y", y.to_string())
                .attr("width", w.to_string())
                .attr("height", h.to_string()),
        )?;
        self.reindex();
        Ok(obj)
    }

    /// Where a new child of this kind goes among a diagram object's children,
    /// in the order Archi writes them: `<bounds>`, then `<feature>`, then the
    /// connections leaving it, then the objects nested in it, then a Note's
    /// `<content>`, then documentation and properties — the metamodel lists
    /// the object's own features before the container's.
    fn object_slot_for(&self, obj: NodeId, kind: &str) -> usize {
        fn rank(name: &str) -> u8 {
            match name {
                "bounds" => 0,
                "feature" => 1,
                "sourceConnection" => 2,
                "child" => 3,
                "content" => 4,
                "documentation" => 5,
                "property" => 6,
                _ => 3,
            }
        }
        let want = rank(kind);
        self.doc
            .children(obj)
            .enumerate()
            .filter(|(_, c)| rank(self.doc.local_name(*c)) <= want)
            .map(|(i, _)| i + 1)
            .last()
            .unwrap_or(0)
    }

    /// Move an object already on a view inside another object, or back to
    /// the top with `None`, keeping its place on the canvas.
    ///
    /// This is what Archi does when a box is dragged into another: the bounds
    /// become relative to the new container, and a line drawn between the
    /// two is deleted, because the nesting now stands for it. The ids of the
    /// connections removed are returned.
    pub fn nest_view_object(
        &mut self,
        view: ViewId,
        object_id: &str,
        into: Option<&str>,
    ) -> Result<Vec<String>, EditError> {
        let node = self.object_node(view, object_id)?;
        let target = self.container_node(view, into)?;
        if target == node || self.doc.descendants(node).contains(&target) {
            return Err(EditError::ObjectCycle(
                object_id.to_string(),
                into.unwrap_or_default().to_string(),
            ));
        }
        // Absolute position before the move, so it can be kept after it.
        let (ax, ay) = self.absolute_origin(view, node);
        let (px, py) = if into.is_some() { self.absolute_origin(view, target) } else { (0, 0) };
        let already = self.doc.parent(node) == Some(target);
        if !already {
            let at = if into.is_some() {
                self.object_slot_for(target, "child")
            } else {
                self.slot_for(target, "child")
            };
            self.doc.move_child(node, target, at);
            if let Some(b) = self.doc.child_named(node, "bounds") {
                self.doc.set_attr(b, "x", &(ax - px).to_string());
                self.doc.set_attr(b, "y", &(ay - py).to_string());
            }
        }
        // A connection between a container and what it now holds is hidden
        // by the nesting; Archi removes it, and so does this.
        let mut removed = Vec::new();
        if let Some(parent_id) = into {
            let doomed: Vec<(NodeId, String)> = self
                .doc
                .descendants(self.view(view).node)
                .into_iter()
                .filter(|n| self.doc.local_name(*n) == "sourceConnection")
                .filter(|n| {
                    let s = self.doc.attr(*n, "source").unwrap_or_default();
                    let t = self.doc.attr(*n, "target").unwrap_or_default();
                    (s == object_id && t == parent_id) || (s == parent_id && t == object_id)
                })
                .filter_map(|n| Some((n, self.doc.attr(n, "id")?)))
                .collect();
            for (n, id) in doomed {
                self.doc.remove_subtree(n);
                removed.push(id);
            }
            if !removed.is_empty() {
                self.recompute_target_connections(view);
            }
        }
        self.reindex();
        Ok(removed)
    }

    /// Where an object sits on the canvas: its own bounds plus every
    /// container's above it, since each is stored relative to its parent.
    fn absolute_origin(&self, view: ViewId, node: NodeId) -> (i32, i32) {
        let view_node = self.view(view).node;
        let (mut x, mut y) = (0, 0);
        let mut cur = Some(node);
        while let Some(n) = cur {
            if n == view_node {
                break;
            }
            if let Some(b) = self.doc.child_named(n, "bounds") {
                x += self.doc.attr(b, "x").and_then(|v| v.parse::<i32>().ok()).unwrap_or(0);
                y += self.doc.attr(b, "y").and_then(|v| v.parse::<i32>().ok()).unwrap_or(0);
            }
            cur = self.doc.parent(n);
        }
        (x, y)
    }

    /// Every object on a view with what it shows and what holds it, in
    /// document order: (object id, kind, concept id, container object id,
    /// name or text). `kind` is `DiagramObject`, `Group`, `Note` or whatever
    /// else Archi drew; the container is `None` at the top of the view.
    pub fn view_tree(&self, view: ViewId) -> Vec<ViewObject> {
        let view_node = self.view(view).node;
        self.doc
            .descendants(view_node)
            .into_iter()
            .filter(|n| self.doc.local_name(*n) == "child")
            .filter_map(|n| {
                let id = self.doc.attr(n, "id")?;
                let xsi = self.doc.attr(n, "xsi:type").unwrap_or_default();
                let parent = self
                    .doc
                    .parent(n)
                    .filter(|p| self.doc.local_name(*p) == "child")
                    .and_then(|p| self.doc.attr(p, "id"));
                let text = match xsi.trim_start_matches("archimate:") {
                    "Note" => self.doc.child_named(n, "content").map(|c| self.doc.text(c)),
                    _ => self.doc.attr(n, "name"),
                };
                Some(ViewObject {
                    id,
                    kind: xsi.trim_start_matches("archimate:").to_string(),
                    concept: self.doc.attr(n, "archimateElement"),
                    parent,
                    text: text.unwrap_or_default(),
                })
            })
            .collect()
    }

    /// The pairs of concepts a view shows one inside the other, as
    /// (container's concept, nested concept). A relationship between two of
    /// them is on the view without a line — that is what the nesting says.
    pub fn view_nestings(&self, view: ViewId) -> Vec<(String, String)> {
        let tree = self.view_tree(view);
        let concept_of: std::collections::HashMap<&str, &str> =
            tree.iter().filter_map(|o| Some((o.id.as_str(), o.concept.as_deref()?))).collect();
        tree.iter()
            .filter_map(|o| {
                let inner = o.concept.as_deref()?;
                let outer = concept_of.get(o.parent.as_deref()?)?;
                Some((outer.to_string(), inner.to_string()))
            })
            .collect()
    }

    /// Draw a relationship between two objects already on the view.
    ///
    /// The connection is a child of its *source* object, which is how Archi
    /// stores it, and `targetConnections` on the target is recomputed rather
    /// than appended to.
    /// `bendpoints` are Draw2D relative offsets — `(startX, startY, endX,
    /// endY)` — not positions. Converting an absolute waypoint into that form
    /// is `geometry::bendpoint_for` in amcli-view; the model layer stores what
    /// it is given.
    pub fn add_view_connection(
        &mut self,
        view: ViewId,
        relationship: ConceptId,
        source_object: &str,
        target_object: &str,
        bendpoints: &[(i32, i32, i32, i32)],
    ) -> Result<String, EditError> {
        let rel_id = self.concept(relationship).id.clone();
        let view_id = self.view(view).id.clone();
        let id = self.fresh_id(&["connection", &view_id, &rel_id, source_object, target_object]);
        let view_node = self.view(view).node;
        let src = self
            .doc
            .descendants(view_node)
            .into_iter()
            .find(|n| {
                self.doc.local_name(*n) == "child"
                    && self.doc.attr(*n, "id").as_deref() == Some(source_object)
            })
            .ok_or_else(|| EditError::NoSuchConcept(source_object.to_string()))?;

        // Among the object's other connections, before anything nested in
        // it: see `object_slot_for`.
        let at = self.object_slot_for(src, "sourceConnection");
        let conn = self.doc.insert_child(
            src,
            at,
            NodeBuilder::new("sourceConnection")
                .attr("xsi:type", "archimate:Connection")
                .attr("id", &*id)
                .attr("source", source_object)
                .attr("target", target_object)
                .attr("archimateRelationship", &*rel_id),
        )?;
        for (sx, sy, ex, ey) in bendpoints {
            self.doc.append_child(
                conn,
                NodeBuilder::new("bendpoint")
                    .attr("startX", sx.to_string())
                    .attr("startY", sy.to_string())
                    .attr("endX", ex.to_string())
                    .attr("endY", ey.to_string()),
            )?;
        }
        self.recompute_target_connections(view);
        self.reindex();
        Ok(id)
    }

    /// Where a relationship could be drawn on a view and is not yet: the
    /// source and target objects to join. `None` when an end is not on the
    /// view, or a line already stands for the relationship there.
    ///
    /// The first object per concept: a concept may be on a view more than
    /// once, and one line rather than one per copy is what Archi draws when
    /// an element is dropped onto a diagram. Every "draw what can be drawn"
    /// decision — `view add` wiring a new box to what is there, `relation
    /// add` reaching every view that shows both ends — asks this one question.
    ///
    /// Also `None` when one end's box sits directly inside the other's: the
    /// nesting stands for the relationship, and Archi draws no line for it —
    /// its default is to hide every relationship type between a container
    /// and what it holds.
    pub fn undrawn_connection(&self, view: ViewId, rel: ConceptId) -> Option<(String, String)> {
        let r = self.concept(rel);
        let (src, tgt) = (r.source.as_deref()?, r.target.as_deref()?);
        let view_node = self.view(view).node;
        let (mut src_obj, mut tgt_obj): (Option<NodeId>, Option<NodeId>) = (None, None);
        for n in self.doc.descendants(view_node) {
            match self.doc.local_name(n) {
                "child" => {
                    let Some(shown) = self.doc.attr(n, "archimateElement") else { continue };
                    if shown == src && src_obj.is_none() {
                        src_obj = Some(n);
                    }
                    if shown == tgt && tgt_obj.is_none() {
                        tgt_obj = Some(n);
                    }
                }
                "sourceConnection"
                    if self.doc.attr(n, "archimateRelationship").as_deref() == Some(&r.id) =>
                {
                    return None;
                }
                _ => {}
            }
        }
        let (s, t) = (src_obj?, tgt_obj?);
        if self.doc.parent(s) == Some(t) || self.doc.parent(t) == Some(s) {
            return None;
        }
        Some((self.doc.attr(s, "id")?, self.doc.attr(t, "id")?))
    }

    /// Draw a relationship on every view that already shows both of its ends,
    /// and say which. A view that draws it already is left alone.
    ///
    /// A relationship added to the model between two things a drawing shows
    /// used to reach no drawing: the model held a relationship on no view,
    /// and only a separate query revealed it. A view is a drawing of the
    /// model, so what the model now says between two boxes is drawn.
    pub fn draw_relation_on_views(&mut self, rel: ConceptId) -> Result<Vec<ViewId>, EditError> {
        let views: Vec<ViewId> = self.views_with_ids().map(|(i, _)| i).collect();
        let mut drawn = Vec::new();
        for v in views {
            if let Some((src, tgt)) = self.undrawn_connection(v, rel) {
                self.add_view_connection(v, rel, &src, &tgt, &[])?;
                drawn.push(v);
            }
        }
        Ok(drawn)
    }

    /// Move an object already on a view.
    pub fn set_view_object_bounds(
        &mut self,
        view: ViewId,
        object_id: &str,
        x: i32,
        y: i32,
    ) -> Result<(), EditError> {
        self.set_view_object_rect(view, object_id, x, y, None)
    }

    /// Move and resize an object already on a view. `size` of `None` leaves the
    /// width and height as they are.
    pub fn set_view_object_rect(
        &mut self,
        view: ViewId,
        object_id: &str,
        x: i32,
        y: i32,
        size: Option<(i32, i32)>,
    ) -> Result<(), EditError> {
        let view_node = self.view(view).node;
        let Some(obj) = self.doc.descendants(view_node).into_iter().find(|n| {
            self.doc.local_name(*n) == "child"
                && self.doc.attr(*n, "id").as_deref() == Some(object_id)
        }) else {
            return Err(EditError::NoSuchConcept(object_id.to_string()));
        };
        if let Some(b) = self.doc.child_named(obj, "bounds") {
            self.doc.set_attr(b, "x", &x.to_string());
            self.doc.set_attr(b, "y", &y.to_string());
            if let Some((w, h)) = size {
                self.doc.set_attr(b, "width", &w.to_string());
                self.doc.set_attr(b, "height", &h.to_string());
            }
        }
        Ok(())
    }

    /// Replace the bendpoints on a connection already on a view.
    ///
    /// The connection is found by its `id`. Existing bendpoints are removed and
    /// the given ones written in their place; an empty list straightens the
    /// line. Same encoding as [`Self::add_view_connection`].
    pub fn set_view_connection_bendpoints(
        &mut self,
        view: ViewId,
        connection_id: &str,
        bendpoints: &[(i32, i32, i32, i32)],
    ) -> Result<(), EditError> {
        let view_node = self.view(view).node;
        let Some(conn) = self.doc.descendants(view_node).into_iter().find(|n| {
            self.doc.local_name(*n) == "sourceConnection"
                && self.doc.attr(*n, "id").as_deref() == Some(connection_id)
        }) else {
            return Err(EditError::NoSuchConcept(connection_id.to_string()));
        };
        let old: Vec<NodeId> =
            self.doc.children(conn).filter(|c| self.doc.local_name(*c) == "bendpoint").collect();
        for c in old {
            self.doc.remove_subtree(c);
        }
        for (sx, sy, ex, ey) in bendpoints {
            self.doc.append_child(
                conn,
                NodeBuilder::new("bendpoint")
                    .attr("startX", sx.to_string())
                    .attr("startY", sy.to_string())
                    .attr("endX", ex.to_string())
                    .attr("endY", ey.to_string()),
            )?;
        }
        Ok(())
    }

    /// Replace, or clear, a view's documentation.
    ///
    /// A view carries documentation exactly as a concept does — the same
    /// `<documentation>` first child, which Archi shows in the Properties view
    /// — so this is `set_documentation` pointed at the diagram's node. Only
    /// the way in was missing: `ViewId` is not a `ConceptId`, and every route
    /// to the text went through one.
    pub fn set_view_documentation(&mut self, view: ViewId, text: &str) -> Result<(), EditError> {
        let node = self.view(view).node;
        self.set_documentation_node(node, text)
    }

    pub fn rename_view(&mut self, view: ViewId, name: &str) {
        let node = self.view(view).node;
        self.doc.set_attr(node, "name", name);
        self.reindex();
    }

    /// Set, or clear, the viewpoint of a view that already exists.
    ///
    /// An empty viewpoint is "no viewpoint", and EMF writes that as the absent
    /// attribute rather than as `viewpoint=""` — so clearing removes it, and a
    /// view that never had one stays byte-identical when cleared again.
    pub fn set_view_viewpoint(&mut self, view: ViewId, viewpoint: &str) {
        let node = self.view(view).node;
        if viewpoint.is_empty() {
            self.doc.remove_attr(node, "viewpoint");
        } else {
            self.doc.set_attr(node, "viewpoint", viewpoint);
        }
        self.reindex();
    }

    /// Re-file a view under another folder in the views tree.
    ///
    /// Archi files every diagram somewhere under the single top-level folder of
    /// type `diagrams`, and nests user folders inside it freely. A diagram moved
    /// outside that subtree still parses, but Archi will not show it, so the
    /// destination is checked rather than trusted: an ordinary typo like
    /// `/Business` should be an error the caller can read, not a model that
    /// opens with a view missing.
    pub fn move_view_to_folder(&mut self, view: ViewId, folder: FolderId) -> Result<(), EditError> {
        if !self.is_views_folder(folder) {
            return Err(EditError::NotAViewsFolder(self.folder(folder).path.clone()));
        }
        let node = self.view(view).node;
        let target = self.folder(folder).node;
        if self.doc.parent(node) == Some(target) {
            return Ok(());
        }
        let at = self.doc.children(target).count();
        self.doc.move_child(node, target, at);
        self.reindex();
        Ok(())
    }

    /// Where a view sits among its folder's children.
    ///
    /// Paired with `place_view_at` so that regenerating a view can put it back
    /// exactly where it was. Without that, "delete and recreate" moves it to
    /// the end of the folder, and a script that regenerates every view rewrites
    /// the whole views section on each run — the diff then shows everything and
    /// therefore nothing.
    pub fn view_position(&self, view: ViewId) -> Option<(FolderId, usize)> {
        let node = self.view(view).node;
        let parent = self.doc.parent(node)?;
        let at = self.doc.children(parent).position(|c| c == node)?;
        Some((self.view(view).folder, at))
    }

    /// Move a view to a given index among its folder's children.
    pub fn place_view_at(&mut self, view: ViewId, folder: FolderId, at: usize) {
        let node = self.view(view).node;
        let parent = self.folder(folder).node;
        if self.doc.parent(node) == Some(parent)
            && self.doc.children(parent).position(|c| c == node) == Some(at)
        {
            return;
        }
        let at = at.min(self.doc.children(parent).count());
        self.doc.move_child(node, parent, at);
        self.reindex();
    }

    /// Whether a folder is the views folder or nested inside it.
    pub fn is_views_folder(&self, folder: FolderId) -> bool {
        let Some(root) = self.top_folder(FolderType::Diagrams) else { return false };
        let mut at = Some(folder);
        while let Some(f) = at {
            if f == root {
                return true;
            }
            at = self.folder(f).parent;
        }
        false
    }

    /// Objects on *other* views that point at this one, as (view id, object id).
    ///
    /// A `DiagramModelReference` is a box on one view standing for another view.
    /// Deleting the target without dealing with these leaves `model="…"`
    /// pointing at nothing, which is a load error in Archi rather than a
    /// cosmetic problem — the same class of breakage as a dangling
    /// `archimateElement`.
    pub fn view_references(&self, view: ViewId) -> Vec<(String, String)> {
        let target = self.view(view).id.clone();
        let mut out = Vec::new();
        for v in self.views() {
            if v.id == target {
                continue;
            }
            for n in self.doc.descendants(v.node) {
                if self.doc.local_name(n) == "child"
                    && self.doc.attr(n, "model").as_deref() == Some(target.as_str())
                    && let Some(id) = self.doc.attr(n, "id")
                {
                    out.push((v.id.clone(), id));
                }
            }
        }
        out
    }

    /// Delete a view, and any box on another view that stood for it.
    ///
    /// No concept is touched: a view is a drawing of the model, not part of it.
    /// What the returned cascade counts is therefore visual only.
    pub fn delete_view(&mut self, view: ViewId) -> Result<Cascade, EditError> {
        let mut plan = Cascade::default();
        let view_node = self.view(view).node;
        plan.views.push(self.view(view).id.clone());

        // Everything drawn on the view goes with it, and is worth reporting:
        // "deleted a view" and "deleted a view holding sixty boxes" deserve
        // different reactions.
        let (objects, connections) = {
            let all: std::collections::HashSet<String> = self
                .doc
                .descendants(view_node)
                .into_iter()
                .filter_map(|n| self.doc.attr(n, "id"))
                .collect();
            self.classify_visuals(view_node, &all)
        };
        plan.diagram_objects.extend(objects);
        plan.connections.extend(connections);

        // References from elsewhere, expanded the same way a deleted concept's
        // visuals are: the reference box may itself be an endpoint.
        let referring = self.view_references(view);
        let mut dead_elsewhere: Vec<(ViewId, Vec<String>, Vec<String>)> = Vec::new();
        for v in self.views() {
            let seeds: std::collections::HashSet<String> = referring
                .iter()
                .filter(|(vid, _)| *vid == v.id)
                .map(|(_, oid)| oid.clone())
                .collect();
            if seeds.is_empty() {
                continue;
            }
            let mut dead = seeds;
            self.expand_dead_visuals(v.node, &mut dead);
            let (objects, connections) = self.classify_visuals(v.node, &dead);
            dead_elsewhere.push((
                self.view_by_id(&v.id).expect("iterating live views"),
                objects,
                connections,
            ));
            plan.views.push(v.id.clone());
        }

        let mut doomed_nodes = vec![view_node];
        for (v, objects, connections) in &dead_elsewhere {
            let node = self.view(*v).node;
            let ids: std::collections::HashSet<&str> =
                objects.iter().chain(connections.iter()).map(String::as_str).collect();
            for n in self.doc.descendants(node) {
                if self.doc.attr(n, "id").is_some_and(|id| ids.contains(id.as_str())) {
                    doomed_nodes.push(n);
                }
            }
            plan.diagram_objects.extend(objects.iter().cloned());
            plan.connections.extend(connections.iter().cloned());
        }

        for n in doomed_nodes {
            self.doc.remove_subtree(n);
        }
        // The mirror is recomputed, never patched — the same reason as in
        // `delete_concept`.
        let affected: Vec<ViewId> = dead_elsewhere.iter().map(|(v, _, _)| *v).collect();
        for v in affected {
            self.recompute_target_connections(v);
        }
        self.reindex();
        Ok(plan)
    }

    /// Every diagram object on a view, as (object id, concept id).
    pub fn view_objects(&self, view: ViewId) -> Vec<(String, Option<String>)> {
        let node = self.view(view).node;
        self.doc
            .descendants(node)
            .into_iter()
            .filter(|n| self.doc.local_name(*n) == "child")
            .filter_map(|n| Some((self.doc.attr(n, "id")?, self.doc.attr(n, "archimateElement"))))
            .collect()
    }

    /// Every connection on a view, as (connection id, source object id,
    /// target object id), in document order.
    pub fn view_connections(&self, view: ViewId) -> Vec<(String, String, String)> {
        let node = self.view(view).node;
        self.doc
            .descendants(node)
            .into_iter()
            .filter(|n| self.doc.local_name(*n) == "sourceConnection")
            .filter_map(|n| {
                Some((
                    self.doc.attr(n, "id")?,
                    self.doc.attr(n, "source")?,
                    self.doc.attr(n, "target")?,
                ))
            })
            .collect()
    }
}
