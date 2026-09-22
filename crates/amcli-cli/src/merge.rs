//! `diff` and `merge`: two files compared, three reconciled, block by block.
//!
//! A block is one `<element …>` under the folder tree — a concept or a view —
//! or a folder, keyed by its id. Blocks are compared in their canonical form
//! (`amcli_model::canon`), so a save in Archi that reorders attributes,
//! drops a default `y="0"` or spells `>` as `&gt;` changes nothing here.
//!
//! `merge` is written to be a git merge driver as much as a command: with no
//! `-o` the result goes over OURS, which is `%A`; a clean merge exits 0 and a
//! conflicted one writes nothing and exits with the conflict code, so git
//! leaves the file for a person. Nothing is minted — every byte of the
//! result comes from OURS or is grafted out of THEIRS — and OURS is never
//! re-serialised where it did not change.

use std::collections::HashMap;
use std::path::Path;

use amcli_model::Model;
use amcli_model::canon::{Canon, describe};
use amcli_xml::{NodeBuilder, NodeId};

use crate::output::{CliError, Code, Output, Row};
use crate::write::Opts;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Element,
    Relation,
    View,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Element => "element",
            Kind::Relation => "relation",
            Kind::View => "view",
        }
    }
}

struct Block {
    kind: Kind,
    id: String,
    name: String,
    node: NodeId,
    /// The id and path of the folder it sits in.
    folder: String,
    folder_path: String,
    canon: Canon,
}

struct FolderInfo {
    id: String,
    name: String,
    path: String,
    /// Parent folder id; none on a top-level folder.
    parent: Option<String>,
    node: NodeId,
}

/// One of the files, read once: its model and the blocks in document order.
struct Side {
    model: Model,
    folders: Vec<FolderInfo>,
    folder_by_id: HashMap<String, usize>,
    folder_by_path: HashMap<String, usize>,
    blocks: Vec<Block>,
    block_by_id: HashMap<String, usize>,
}

impl Side {
    fn load(path: &Path) -> Result<Side, CliError> {
        let model = Model::open(path).map_err(|e| CliError::new(Code::Io, "io", e.to_string()))?;
        Ok(Side::scan(model))
    }

    fn scan(model: Model) -> Side {
        let doc = &model.doc;
        // Node ids are handed out in parse order, so sorting by node is
        // document order whatever order the indices keep.
        let mut folders: Vec<FolderInfo> = model
            .folders()
            .map(|f| FolderInfo {
                id: f.id.clone(),
                name: f.name.clone(),
                path: f.path.clone(),
                parent: f.parent.map(|p| model.folder(p).id.clone()),
                node: f.node,
            })
            .collect();
        folders.sort_by_key(|f| f.node);

        let mut blocks: Vec<Block> = model
            .concepts()
            .map(|c| {
                let folder = model.folder(c.folder);
                Block {
                    kind: if c.kind.is_relationship() { Kind::Relation } else { Kind::Element },
                    id: c.id.clone(),
                    name: c.name.clone(),
                    node: c.node,
                    folder: folder.id.clone(),
                    folder_path: folder.path.clone(),
                    canon: Canon::of(doc, c.node),
                }
            })
            .chain(model.views().map(|v| {
                let folder = model.folder(v.folder);
                Block {
                    kind: Kind::View,
                    id: v.id.clone(),
                    name: v.name.clone(),
                    node: v.node,
                    folder: folder.id.clone(),
                    folder_path: folder.path.clone(),
                    canon: Canon::of(doc, v.node),
                }
            }))
            .filter(|b| !b.id.is_empty())
            .collect();
        blocks.sort_by_key(|b| b.node);

        let folder_by_id = folders.iter().enumerate().map(|(i, f)| (f.id.clone(), i)).collect();
        let folder_by_path = folders.iter().enumerate().map(|(i, f)| (f.path.clone(), i)).collect();
        let block_by_id = blocks.iter().enumerate().map(|(i, b)| (b.id.clone(), i)).collect();
        Side { model, folders, folder_by_id, folder_by_path, blocks, block_by_id }
    }

    fn name(&self) -> String {
        self.model.name()
    }

    fn version(&self) -> String {
        self.model.version()
    }

    fn purpose(&self) -> Option<String> {
        self.model.purpose()
    }
}

/// The block with its name taken out, for telling a rename from a change.
fn nameless(c: &Canon) -> Canon {
    let mut c = c.clone();
    c.attrs.retain(|(a, _)| a != "name");
    c
}

fn row(status: &str, kind: &str, id: &str, name: &str, detail: impl Into<String>) -> Row {
    Row::new().s("status", status).s("kind", kind).s("id", id).s("name", name).s("detail", detail)
}

// ---- diff -------------------------------------------------------------------

pub fn diff(a: &Path, b: &Path) -> Result<Output, CliError> {
    let a = Side::load(a)?;
    let b = Side::load(b)?;
    let mut rows = Vec::new();

    // The model itself: its name, version and purpose.
    let (id, name) = (b.model.model_id(), b.name());
    if a.name() != name {
        rows.push(row("renamed", "model", &id, &name, format!("name {} → {}", a.name(), name)));
    }
    if a.version() != b.version() {
        let d = format!("version {} → {}", a.version(), b.version());
        rows.push(row("changed", "model", &id, &name, d));
    }
    if a.purpose() != b.purpose() {
        rows.push(row("changed", "model", &id, &name, "purpose"));
    }

    for fb in &b.folders {
        match a.folder_by_id.get(&fb.id) {
            None => rows.push(row("added", "folder", &fb.id, &fb.name, fb.path.clone())),
            Some(&i) => {
                let fa = &a.folders[i];
                if fa.parent != fb.parent {
                    let d = format!("folder {} → {}", parent_path(&fa.path), parent_path(&fb.path));
                    rows.push(row("moved", "folder", &fb.id, &fb.name, d));
                }
                if fa.name != fb.name {
                    let d = format!("name {} → {}", fa.name, fb.name);
                    rows.push(row("renamed", "folder", &fb.id, &fb.name, d));
                }
            }
        }
    }
    for fa in &a.folders {
        if !b.folder_by_id.contains_key(&fa.id) {
            rows.push(row("removed", "folder", &fa.id, &fa.name, fa.path.clone()));
        }
    }

    for bb in &b.blocks {
        let kind = bb.kind.as_str();
        match a.block_by_id.get(&bb.id) {
            None => rows.push(row("added", kind, &bb.id, &bb.name, bb.folder_path.clone())),
            Some(&i) => {
                let ba = &a.blocks[i];
                if ba.folder != bb.folder {
                    let d = format!("folder {} → {}", ba.folder_path, bb.folder_path);
                    rows.push(row("moved", kind, &bb.id, &bb.name, d));
                }
                if let Some(d) = describe(&ba.canon, &bb.canon) {
                    let status = if ba.name != bb.name && nameless(&ba.canon) == nameless(&bb.canon)
                    {
                        "renamed"
                    } else {
                        "changed"
                    };
                    rows.push(row(status, kind, &bb.id, &bb.name, d));
                }
            }
        }
    }
    for ba in &a.blocks {
        if !b.block_by_id.contains_key(&ba.id) {
            rows.push(row("removed", ba.kind.as_str(), &ba.id, &ba.name, ba.folder_path.clone()));
        }
    }

    let n = rows.len();
    Ok(Output::rows(rows).meta_n("differences", n as i64))
}

fn parent_path(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) | None => "/",
        Some(i) => &path[..i],
    }
}

// ---- merge ------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Prefer {
    Ours,
    Theirs,
}

impl Prefer {
    pub fn parse(s: &str) -> Result<Prefer, CliError> {
        match s {
            "ours" => Ok(Prefer::Ours),
            "theirs" => Ok(Prefer::Theirs),
            _ => Err(CliError::new(
                Code::Usage,
                "usage",
                format!("`{s}` is not a side; --prefer takes ours or theirs"),
            )),
        }
    }
}

/// One thing the merge will do to OURS. Indices are into the sides' vectors.
#[derive(Clone, Debug)]
enum Action {
    SetName(String),
    SetVersion(String),
    SetPurpose(Option<String>),
    /// A folder of THEIRS to create, ancestors included where they are missing.
    CreateFolder(usize),
    RenameFolder {
        ours: usize,
        name: String,
    },
    /// A folder of OURS to drop once the blocks are settled, if it is empty then.
    DeleteFolder(usize),
    /// OURS' block moved to the folder THEIRS' block sits in.
    Move {
        ours: usize,
        theirs: usize,
    },
    /// OURS' block's content replaced by THEIRS', where it stands.
    Replace {
        ours: usize,
        theirs: usize,
    },
    Delete(usize),
    Insert(usize),
}

/// A change both sides made differently. `theirs` is what taking their side
/// means; taking ours means doing nothing.
struct Conflict {
    kind: &'static str,
    id: String,
    name: String,
    reason: &'static str,
    theirs: Vec<Action>,
}

struct Plan {
    actions: Vec<Action>,
    conflicts: Vec<Conflict>,
}

fn plan(base: &Side, ours: &Side, theirs: &Side) -> Plan {
    let mut actions = Vec::new();
    let mut conflicts = Vec::new();

    // The model's own attributes, three-way each.
    let three = |b: &str, o: &str, t: &str| -> Option<bool> {
        // Some(true): take theirs. Some(false): conflict. None: nothing.
        if t == b || t == o {
            None
        } else if o == b {
            Some(true)
        } else {
            Some(false)
        }
    };
    let root_id = ours.model.model_id();
    let mut root = |what: &'static str, verdict: Option<bool>, action: Action| match verdict {
        Some(true) => actions.push(action),
        Some(false) => conflicts.push(Conflict {
            kind: "model",
            id: root_id.clone(),
            name: ours.name(),
            reason: what,
            theirs: vec![action],
        }),
        None => {}
    };
    root(
        "name changed on both sides",
        three(&base.name(), &ours.name(), &theirs.name()),
        Action::SetName(theirs.name()),
    );
    root(
        "version changed on both sides",
        three(&base.version(), &ours.version(), &theirs.version()),
        Action::SetVersion(theirs.version()),
    );
    let (bp, op, tp) = (base.purpose(), ours.purpose(), theirs.purpose());
    let purpose = if tp == bp || tp == op { None } else { Some(op == bp) };
    root("purpose changed on both sides", purpose, Action::SetPurpose(tp));

    // Folders: created, renamed, and — after the blocks — deleted.
    for (ti, tf) in theirs.folders.iter().enumerate() {
        match (base.folder_by_id.get(&tf.id), ours.folder_by_id.get(&tf.id)) {
            (Some(&bi), Some(&oi)) => {
                let (bf, of) = (&base.folders[bi], &ours.folders[oi]);
                let rename = Action::RenameFolder { ours: oi, name: tf.name.clone() };
                if tf.name != bf.name && tf.name != of.name {
                    if of.name == bf.name {
                        actions.push(rename);
                    } else {
                        conflicts.push(Conflict {
                            kind: "folder",
                            id: tf.id.clone(),
                            name: of.name.clone(),
                            reason: "renamed on both sides",
                            theirs: vec![rename],
                        });
                    }
                }
            }
            // Deleted in ours, or added on both sides under one seed: as it is.
            (Some(_), None) | (None, Some(_)) => {}
            (None, None) => actions.push(Action::CreateFolder(ti)),
        }
    }
    for (oi, of) in ours.folders.iter().enumerate() {
        if of.parent.is_some()
            && base.folder_by_id.contains_key(&of.id)
            && !theirs.folder_by_id.contains_key(&of.id)
        {
            actions.push(Action::DeleteFolder(oi));
        }
    }

    // Blocks.
    for (ti, tb) in theirs.blocks.iter().enumerate() {
        let kind = tb.kind.as_str();
        match (base.block_by_id.get(&tb.id), ours.block_by_id.get(&tb.id)) {
            (Some(&bi), Some(&oi)) => {
                let (bb, ob) = (&base.blocks[bi], &ours.blocks[oi]);
                let replace = Action::Replace { ours: oi, theirs: ti };
                if tb.canon != bb.canon && tb.canon != ob.canon {
                    if ob.canon == bb.canon {
                        actions.push(replace);
                    } else {
                        conflicts.push(Conflict {
                            kind,
                            id: tb.id.clone(),
                            name: ob.name.clone(),
                            reason: "changed on both sides",
                            theirs: vec![replace],
                        });
                    }
                }
                let mv = Action::Move { ours: oi, theirs: ti };
                if tb.folder != bb.folder && tb.folder != ob.folder {
                    if ob.folder == bb.folder {
                        actions.push(mv);
                    } else {
                        conflicts.push(Conflict {
                            kind,
                            id: tb.id.clone(),
                            name: ob.name.clone(),
                            reason: "moved to different folders on both sides",
                            theirs: vec![mv],
                        });
                    }
                }
            }
            (Some(&bi), None) => {
                if tb.canon != base.blocks[bi].canon {
                    conflicts.push(Conflict {
                        kind,
                        id: tb.id.clone(),
                        name: tb.name.clone(),
                        reason: "deleted in ours, changed in theirs",
                        theirs: vec![Action::Insert(ti)],
                    });
                }
            }
            (None, Some(&oi)) => {
                if tb.canon != ours.blocks[oi].canon {
                    conflicts.push(Conflict {
                        kind,
                        id: tb.id.clone(),
                        name: ours.blocks[oi].name.clone(),
                        reason: "added on both sides, differently",
                        theirs: vec![Action::Replace { ours: oi, theirs: ti }],
                    });
                }
            }
            (None, None) => actions.push(Action::Insert(ti)),
        }
    }
    for (oi, ob) in ours.blocks.iter().enumerate() {
        if theirs.block_by_id.contains_key(&ob.id) {
            continue;
        }
        if let Some(&bi) = base.block_by_id.get(&ob.id) {
            if ob.canon == base.blocks[bi].canon {
                actions.push(Action::Delete(oi));
            } else {
                conflicts.push(Conflict {
                    kind: ob.kind.as_str(),
                    id: ob.id.clone(),
                    name: ob.name.clone(),
                    reason: "changed in ours, deleted in theirs",
                    theirs: vec![Action::Delete(oi)],
                });
            }
        }
    }

    Plan { actions, conflicts }
}

/// Applies a plan to OURS, grafting out of THEIRS.
struct Merger<'a> {
    ours: &'a mut Side,
    theirs: &'a Side,
    /// THEIRS' folder id → the node in OURS that stands for it.
    resolved: HashMap<String, NodeId>,
    rows: Vec<Row>,
    counts: HashMap<&'static str, i64>,
}

impl Merger<'_> {
    fn report(&mut self, action: &'static str, kind: &str, id: &str, name: &str) {
        self.rows.push(Row::new().s("kind", kind).s("id", id).s("name", name).s("action", action));
        *self.counts.entry(action).or_insert(0) += 1;
    }

    /// The node in OURS for one of THEIRS' folders: the same id, else the
    /// same path, else created under its parent — which is resolved the same
    /// way, so a whole missing branch comes into being from the top down.
    fn folder(&mut self, ti: usize) -> Result<NodeId, CliError> {
        let tf = &self.theirs.folders[ti];
        if let Some(n) = self.resolved.get(&tf.id) {
            return Ok(*n);
        }
        let known = self
            .ours
            .folder_by_id
            .get(&tf.id)
            .or_else(|| self.ours.folder_by_path.get(&tf.path))
            .map(|&i| self.ours.folders[i].node);
        if let Some(n) = known {
            self.resolved.insert(tf.id.clone(), n);
            return Ok(n);
        }
        let parent = match &tf.parent {
            Some(pid) => {
                let pi = self.theirs.folder_by_id[pid];
                self.folder(pi)?
            }
            None => self.ours.model.doc.root(),
        };
        // Folders come before elements in a folder, as Archi writes them.
        let at = self
            .ours
            .model
            .doc
            .children(parent)
            .take_while(|c| self.ours.model.doc.local_name(*c) == "folder")
            .count();
        let tf = &self.theirs.folders[ti];
        let mut b = NodeBuilder::new(self.theirs.model.doc.name(tf.node));
        for a in self.theirs.model.doc.attr_names(tf.node) {
            b = b.attr(a, self.theirs.model.doc.attr(tf.node, a).unwrap_or_default());
        }
        let node = self.ours.model.doc.insert_child(parent, at, b).map_err(internal)?;
        self.resolved.insert(tf.id.clone(), node);
        let (id, name) = (tf.id.clone(), tf.name.clone());
        self.report("created", "folder", &id, &name);
        Ok(node)
    }

    fn apply(&mut self, actions: &[Action]) -> Result<(), CliError> {
        // Order matters: a block is moved before its content is replaced so
        // the replacement lands in the folder it was moved to; inserts come
        // after every other block is settled so "after the sibling ours also
        // has" sees the final neighbours; folders go last, when they are
        // empty or not.
        let rank = |a: &Action| match a {
            Action::SetName(_) | Action::SetVersion(_) | Action::SetPurpose(_) => 0,
            Action::RenameFolder { .. } => 1,
            Action::CreateFolder(_) => 2,
            Action::Move { .. } => 3,
            Action::Replace { .. } => 4,
            Action::Delete(_) => 5,
            Action::Insert(_) => 6,
            Action::DeleteFolder(_) => 7,
        };
        let mut ordered: Vec<&Action> = actions.iter().collect();
        ordered.sort_by_key(|a| rank(a));
        // Folder deletes go bottom-up, so an emptied branch falls whole.
        let first_delete = ordered.iter().position(|a| matches!(a, Action::DeleteFolder(_)));
        if let Some(i) = first_delete {
            ordered[i..].reverse();
        }

        for a in ordered {
            match a {
                Action::SetName(n) => {
                    let root = self.ours.model.doc.root();
                    self.ours.model.doc.set_attr(root, "name", n);
                    let id = self.ours.model.model_id();
                    self.report("renamed", "model", &id, n);
                }
                Action::SetVersion(v) => {
                    let root = self.ours.model.doc.root();
                    self.ours.model.doc.set_attr(root, "version", v);
                    let (id, name) = (self.ours.model.model_id(), self.ours.name());
                    self.report("replaced", "model", &id, &name);
                }
                Action::SetPurpose(p) => {
                    self.set_purpose(p.as_deref())?;
                    let (id, name) = (self.ours.model.model_id(), self.ours.name());
                    self.report("replaced", "model", &id, &name);
                }
                Action::RenameFolder { ours, name } => {
                    let node = self.ours.folders[*ours].node;
                    self.ours.model.doc.set_attr(node, "name", name);
                    let id = self.ours.folders[*ours].id.clone();
                    self.report("renamed", "folder", &id, name);
                }
                Action::CreateFolder(ti) => {
                    self.folder(*ti)?;
                }
                Action::Move { ours, theirs } => {
                    let tb = &self.theirs.blocks[*theirs];
                    let ti = self.theirs.folder_by_id[&tb.folder];
                    let target = self.folder(ti)?;
                    let node = self.ours.blocks[*ours].node;
                    let at = self.ours.model.doc.children(target).count();
                    self.ours.model.doc.move_child(node, target, at);
                    let (kind, id, name) = (tb.kind.as_str(), tb.id.clone(), tb.name.clone());
                    self.report("moved", kind, &id, &name);
                }
                Action::Replace { ours, theirs } => {
                    let node = self.ours.blocks[*ours].node;
                    let doc = &mut self.ours.model.doc;
                    let parent =
                        doc.parent(node).ok_or_else(|| internal("block without folder"))?;
                    let at = doc.children(parent).position(|c| c == node).unwrap_or(0);
                    doc.remove_subtree(node);
                    let tb = &self.theirs.blocks[*theirs];
                    doc.graft(parent, at, &self.theirs.model.doc, tb.node).map_err(internal)?;
                    let (kind, id, name) = (tb.kind.as_str(), tb.id.clone(), tb.name.clone());
                    self.report("replaced", kind, &id, &name);
                }
                Action::Delete(oi) => {
                    let ob = &self.ours.blocks[*oi];
                    let (node, kind, id, name) =
                        (ob.node, ob.kind.as_str(), ob.id.clone(), ob.name.clone());
                    self.ours.model.doc.remove_subtree(node);
                    self.report("deleted", kind, &id, &name);
                }
                Action::Insert(ti) => {
                    let tb = &self.theirs.blocks[*ti];
                    let tfi = self.theirs.folder_by_id[&tb.folder];
                    let target = self.folder(tfi)?;
                    let at = self.insertion_point(target, tfi, tb.node);
                    let doc = &mut self.ours.model.doc;
                    doc.graft(target, at, &self.theirs.model.doc, tb.node).map_err(internal)?;
                    let (kind, id, name) = (tb.kind.as_str(), tb.id.clone(), tb.name.clone());
                    self.report("inserted", kind, &id, &name);
                }
                Action::DeleteFolder(oi) => {
                    let node = self.ours.folders[*oi].node;
                    if self.ours.model.doc.children(node).next().is_some() {
                        continue; // something of ours still lives there
                    }
                    self.ours.model.doc.remove_subtree(node);
                    let (id, name) =
                        (self.ours.folders[*oi].id.clone(), self.ours.folders[*oi].name.clone());
                    self.report("deleted", "folder", &id, &name);
                }
            }
        }
        Ok(())
    }

    /// Where in `target` (ours) a block of THEIRS goes: right after the
    /// nearest block before it in THEIRS' folder that ours also holds, else
    /// at the end.
    fn insertion_point(&self, target: NodeId, theirs_folder: usize, block: NodeId) -> usize {
        let tdoc = &self.theirs.model.doc;
        let tnode = self.theirs.folders[theirs_folder].node;
        let before: Vec<String> = tdoc
            .children(tnode)
            .take_while(|c| *c != block)
            .filter(|c| tdoc.local_name(*c) == "element")
            .filter_map(|c| tdoc.attr(c, "id"))
            .collect();
        let odoc = &self.ours.model.doc;
        let ours: Vec<Option<String>> = odoc.children(target).map(|c| odoc.attr(c, "id")).collect();
        before
            .iter()
            .rev()
            .find_map(|id| ours.iter().position(|o| o.as_deref() == Some(id.as_str())))
            .map(|i| i + 1)
            .unwrap_or(ours.len())
    }

    fn set_purpose(&mut self, text: Option<&str>) -> Result<(), CliError> {
        let doc = &mut self.ours.model.doc;
        let root = doc.root();
        match (doc.child_named(root, "purpose"), text) {
            (Some(n), None) => doc.remove_subtree(n),
            (Some(n), Some(t)) => doc.set_text(n, t).map_err(internal)?,
            (None, None) => {}
            (None, Some(t)) => {
                // After the folders, which is where Archi writes it.
                let at = doc
                    .children(root)
                    .enumerate()
                    .filter(|(_, c)| doc.local_name(*c) == "folder")
                    .map(|(i, _)| i + 1)
                    .last()
                    .unwrap_or(0);
                doc.insert_child(root, at, NodeBuilder::new("purpose").text(t))
                    .map_err(internal)?;
            }
        }
        Ok(())
    }
}

fn internal(e: impl std::fmt::Display) -> CliError {
    CliError::new(Code::Failed, "internal", e.to_string())
}

pub fn merge(
    opts: &Opts,
    base: &Path,
    ours_path: &Path,
    theirs: &Path,
    out: Option<&Path>,
    prefer: Option<Prefer>,
) -> Result<Output, CliError> {
    let base = Side::load(base)?;
    let mut ours = Side::load(ours_path)?;
    let theirs = Side::load(theirs)?;

    let Plan { mut actions, conflicts } = plan(&base, &ours, &theirs);
    let resolved = conflicts.len() as i64;
    match prefer {
        Some(Prefer::Theirs) => actions.extend(conflicts.into_iter().flat_map(|c| c.theirs)),
        Some(Prefer::Ours) => {}
        None if conflicts.is_empty() => {}
        None => {
            let rows = conflicts
                .iter()
                .map(|c| {
                    Row::new()
                        .s("kind", c.kind)
                        .s("id", c.id.clone())
                        .s("name", c.name.clone())
                        .s("reason", c.reason)
                })
                .collect();
            return Ok(Output::rows(rows)
                .meta_n("conflicts", resolved)
                .note("nothing was written; re-run with --prefer ours or --prefer theirs")
                .exit(Code::Conflict));
        }
    }

    let mut merger = Merger {
        ours: &mut ours,
        theirs: &theirs,
        resolved: HashMap::new(),
        rows: Vec::new(),
        counts: HashMap::new(),
    };
    merger.apply(&actions)?;
    let Merger { rows, counts, .. } = merger;
    ours.model.reindex_public();

    let target = out.unwrap_or(ours_path);
    let changed = !ours.model.is_unmodified();
    let written = !opts.dry_run && (changed || out.is_some());
    if written {
        ours.model.save_as(target).map_err(|e| CliError::new(Code::Io, "io", e.to_string()))?;
    }

    let mut o = Output::rows(rows).wrote(written);
    for k in ["inserted", "replaced", "deleted", "moved", "renamed", "created"] {
        o = o.meta_n(k, counts.get(k).copied().unwrap_or(0));
    }
    o = o
        .meta_n("resolved", if prefer.is_some() { resolved } else { 0 })
        .meta("out", target.display().to_string())
        .meta_b("dry_run", opts.dry_run);
    Ok(if opts.dry_run {
        o.note("dry run: nothing was written")
    } else if !changed {
        o.note("nothing to merge: ours already holds everything theirs changed")
    } else {
        o
    })
}
