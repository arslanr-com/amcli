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
    pub fn add_relation(
        &mut self,
        ty: RelType,
        source: ConceptId,
        target: ConceptId,
        access_type: Option<i64>,
        documentation: Option<&str>,
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

        let mut b = NodeBuilder::new("element")
            .attr("xsi:type", ty.info().xsi)
            .attr("id", &*id)
            .attr("source", &*src_id)
            .attr("target", &*tgt_id);
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
                // Documentation is the first child, ahead of properties, which
                // is the order Archi writes.
                self.doc.insert_child(node, 0, NodeBuilder::new("documentation").text(text))?;
            }
        }
        Ok(())
    }

    pub fn set_property(&mut self, c: ConceptId, key: &str, value: &str) -> Result<(), EditError> {
        let node = self.concept(c).node;
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
        let concept_id = self.concept(concept).id.clone();
        let view_id = self.view(view).id.clone();
        let id = self.fresh_id(&["object", &view_id, &concept_id]);
        let view_node = self.view(view).node;
        let obj = self.doc.append_child(
            view_node,
            NodeBuilder::new("child")
                .attr("xsi:type", "archimate:DiagramObject")
                .attr("id", &*id)
                .attr("archimateElement", &*concept_id),
        )?;
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
        Ok(id)
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

        let conn = self.doc.append_child(
            src,
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
