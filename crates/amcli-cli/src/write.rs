//! Write commands.
//!
//! Two rules hold everywhere here. A write is checked against the ArchiMate
//! rules before it lands, and a write that would take other things with it says
//! so and stops unless told otherwise — with the impact report *as* the error,
//! so the natural retry is an informed one.

use amcli_graph::Graph;
use amcli_model::{ConceptId, ElementType, Model, RelType};
use clap::Subcommand;

use crate::output::{CliError, Code, Output, Row};

pub struct Opts {
    pub dry_run: bool,
    pub yes: bool,
    pub expect_checksum: Option<String>,
}

pub enum WriteCmd {
    Element(ElementCmd),
    Relation(RelationCmd),
    Folder(FolderCmd),
    Prop(PropCmd),
}

#[derive(Subcommand, Clone)]
pub enum ElementCmd {
    /// Add an element. With no folder it lands where Archi would file it.
    Add {
        /// ArchiMate type, e.g. ApplicationComponent.
        r#type: String,
        name: String,
        #[arg(short = 'f', long)]
        folder: Option<String>,
        #[arg(long)]
        doc: Option<String>,
    },
    /// Change an element's name.
    Rename { selector: String, name: String },
    /// Replace an element's documentation. An empty string removes it.
    Doc { selector: String, text: String },
    /// Re-file an element. Unknown attributes travel with it.
    Move {
        selector: String,
        #[arg(short = 'f', long)]
        folder: String,
    },
    /// Delete an element and everything that depends on it.
    Delete { selector: String },
    /// Change an element's type in place: same id, name, documentation,
    /// properties, relationships and boxes; only the figure drawn changes.
    /// Refused when a relationship it has would break the matrix.
    Retype {
        selector: String,
        /// The new ArchiMate type, e.g. TechnologyService.
        r#type: String,
    },
    /// Fold an element into another that means the same thing: its
    /// relationships and boxes are repointed, its documentation and
    /// properties carried over, and it is deleted.
    Merge {
        selector: String,
        /// The element that survives.
        #[arg(long)]
        into: String,
        /// Leave the merged element's documentation behind rather than
        /// appending it to the survivor's.
        #[arg(long)]
        drop_doc: bool,
    },
}

#[derive(Subcommand, Clone)]
pub enum RelationCmd {
    /// Add a relationship, and draw it on every view that shows both ends.
    /// Source and target are positional, in that order.
    Add {
        r#type: String,
        source: String,
        target: String,
        /// read | write | rw | unspecified — Access relationships only.
        #[arg(long)]
        access: Option<String>,
        #[arg(long)]
        doc: Option<String>,
        /// A label for the line, e.g. "owns" on an Association.
        #[arg(long)]
        name: Option<String>,
        /// Add it to the model only; draw it on no view.
        #[arg(long)]
        no_draw: bool,
    },
    /// Delete a relationship.
    Delete { selector: String },
    /// Replace a relationship's documentation.
    Doc { selector: String, text: String },
}

#[derive(Subcommand, Clone)]
pub enum FolderCmd {
    /// List folders with their paths.
    List,
    /// Create a folder under an existing one. A repeat is the folder already there.
    Add {
        /// Parent folder path, e.g. /Application.
        parent: String,
        name: String,
    },
    /// Delete an empty folder. Refuses if it holds anything.
    Delete { path: String },
    /// Rename a folder. Its id and everything in it stay as they are.
    Rename { path: String, name: String },
    /// Re-file a folder, with everything in it, under another folder of the
    /// same tree.
    Move {
        path: String,
        /// The folder to move it under, e.g. /Views/Programme.
        #[arg(short = 'p', long)]
        parent: String,
    },
}

#[derive(Subcommand, Clone)]
pub enum PropCmd {
    /// Set a property, replacing any existing value for the key.
    Set { selector: String, key: String, value: String },
    /// Remove a property.
    Unset { selector: String, key: String },
    /// List a concept's properties.
    List { selector: String },
}

pub fn run(opts: Opts, m: &mut Model, cmd: &WriteCmd) -> Result<Output, CliError> {
    guard_checksum(m, &opts)?;
    match cmd {
        WriteCmd::Element(c) => element(&opts, m, c),
        WriteCmd::Relation(c) => relation(&opts, m, c),
        WriteCmd::Folder(c) => folder(&opts, m, c),
        WriteCmd::Prop(c) => prop(&opts, m, c),
    }
}

/// Refuse to write over a file that changed since the caller last read it.
///
/// This, not a lock, is what protects an agent: it reads on one turn and writes
/// three turns later, and no sane design holds a lock across that.
pub fn guard_checksum(m: &Model, opts: &Opts) -> Result<(), CliError> {
    let Some(expected) = &opts.expect_checksum else { return Ok(()) };
    let actual = m.checksum().map_err(io_err)?;
    if &actual != expected {
        return Err(CliError::new(
            Code::Conflict,
            "checksum_mismatch",
            "the model changed since that checksum was taken",
        )
        .hint("re-read the model and decide again; your edit was not applied")
        .rows(vec![Row::new().s("expected", expected.clone()).s("actual", actual)]));
    }
    Ok(())
}

fn io_err(e: impl std::fmt::Display) -> CliError {
    CliError::new(Code::Io, "io", e.to_string())
}

fn invalid(e: impl std::fmt::Display) -> CliError {
    CliError::new(Code::Invalid, "invalid", e.to_string())
}

/// A refused retype or merge. When the refusal is a list of relationships
/// the matrix would no longer permit, each is a row — id, type, the other
/// end and what is permitted for that pair — so the fix is a read away.
/// There is no `--force`: the relationships are fixed first, on purpose.
pub fn refused(e: amcli_model::EditError) -> CliError {
    let amcli_model::EditError::IllegalRelationships(list) = e else { return invalid(e) };
    let rows = list
        .iter()
        .map(|r| {
            Row::new()
                .s("id", r.id.clone())
                .s("type", r.rel)
                .s("direction", r.direction)
                .s("other", r.other.clone())
                .s("other_name", r.other_name.clone())
                .s("other_type", r.other_type.clone())
                .s("permitted", r.permitted.join(","))
        })
        .collect();
    CliError::new(
        Code::Invalid,
        "illegal_relationships",
        format!(
            "{} relationship(s) would break the ArchiMate matrix; nothing was changed",
            list.len()
        ),
    )
    .hint("delete or re-type the listed relationships first, then re-run")
    .rows(rows)
}

pub fn save(m: &Model) -> Result<(), CliError> {
    m.save().map_err(io_err)
}

/// Resolve within a temporarily built index. Writes rebuild the index anyway, so
/// there is no point keeping one alive across the mutation.
pub fn resolve(m: &Model, sel: &str) -> Result<ConceptId, CliError> {
    crate::read::resolve(&Graph::build(m), sel)
}

fn element_type(name: &str) -> Result<ElementType, CliError> {
    ElementType::from_str(name).ok_or_else(|| {
        let close: Vec<&str> = ElementType::ALL
            .iter()
            .map(|e| e.info().short)
            .filter(|s| s.to_lowercase().contains(&name.to_lowercase()))
            .take(5)
            .collect();
        let e = CliError::new(Code::Usage, "usage", format!("`{name}` is not an element type"));
        if RelType::from_str(name).is_some() {
            e.hint("that is a relationship type; an element cannot become one")
        } else if close.is_empty() {
            e.hint("run `amcli stats` to see the types this model uses")
        } else {
            e.hint(format!("did you mean: {}", close.join(", ")))
        }
    })
}

fn rel_type(name: &str) -> Result<RelType, CliError> {
    RelType::from_str(name).ok_or_else(|| {
        CliError::new(Code::Usage, "usage", format!("`{name}` is not a relationship type")).hint(
            format!(
                "one of: {}",
                RelType::ALL.iter().map(|r| r.info().short).collect::<Vec<_>>().join(", ")
            ),
        )
    })
}

fn access_value(s: &str) -> Result<i64, CliError> {
    Ok(match s {
        // 0 is Write in the schema, which is the opposite of most people's guess.
        "write" | "w" => 0,
        "read" | "r" => 1,
        "unspecified" | "none" => 2,
        "rw" | "readwrite" | "read/write" => 3,
        _ => {
            return Err(CliError::new(
                Code::Usage,
                "usage",
                format!("`{s}` is not an access type"),
            )
            .hint("one of: read, write, rw, unspecified"));
        }
    })
}

/// A folder by path, or by `id:` when a path is not enough.
///
/// Paths are normally unique and are what anyone types. The `id:` form exists
/// for the case that broke this rule — two folders that ended up with the same
/// path — where addressing by path can only ever reach the first of them.
fn folder_id(m: &Model, path: &str) -> Result<amcli_model::FolderId, CliError> {
    if let Some(id) = path.strip_prefix("id:") {
        return m.folder_id_by_id(id).ok_or_else(|| {
            CliError::new(Code::NotFound, "not_found", format!("no folder with id `{id}`"))
                .hint("run `amcli folder list` for ids")
        });
    }
    m.folder_by_path(path).ok_or_else(|| {
        let mut paths: Vec<String> = m.folders().map(|f| f.path.clone()).collect();
        paths.sort();
        CliError::new(Code::NotFound, "not_found", format!("no folder at `{path}`"))
            .hint("existing folders below")
            .rows(paths.into_iter().map(|p| Row::new().s("folder", p)).collect())
    })
}

fn written(m: &Model, opts: &Opts, mut row: Row) -> Result<Output, CliError> {
    row = row.b("dry_run", opts.dry_run);
    if !opts.dry_run {
        save(m)?;
    }
    let out = Output::one(row).meta("checksum", m.checksum().map_err(io_err)?).wrote(!opts.dry_run);
    Ok(if opts.dry_run { out.note("dry run: nothing was written") } else { out })
}

fn element(opts: &Opts, m: &mut Model, cmd: &ElementCmd) -> Result<Output, CliError> {
    match cmd {
        ElementCmd::Add { r#type, name, folder, doc } => {
            let ty = element_type(r#type)?;
            let f = folder.as_deref().map(|p| folder_id(m, p)).transpose()?;
            let c = m.add_element(ty, name, f, doc.as_deref()).map_err(invalid)?;
            let concept = m.concept(c);
            written(
                m,
                opts,
                Row::new()
                    .s("id", concept.id.clone())
                    .s("type", concept.kind.name())
                    .s("name", concept.name.clone())
                    .s("folder", m.folder_path_of(concept)),
            )
        }
        ElementCmd::Rename { selector, name } => {
            let c = resolve(m, selector)?;
            let old = m.concept(c).name.clone();
            m.rename(c, name);
            written(
                m,
                opts,
                Row::new().s("id", m.concept(c).id.clone()).s("from", old).s("to", name.clone()),
            )
        }
        ElementCmd::Doc { selector, text } => {
            let c = resolve(m, selector)?;
            m.set_documentation(c, text).map_err(invalid)?;
            written(
                m,
                opts,
                Row::new().s("id", m.concept(c).id.clone()).n("length", text.len() as i64),
            )
        }
        ElementCmd::Move { selector, folder } => {
            let c = resolve(m, selector)?;
            let f = folder_id(m, folder)?;
            m.move_to_folder(c, f).map_err(invalid)?;
            written(
                m,
                opts,
                Row::new().s("id", m.concept(c).id.clone()).s("folder", folder.clone()),
            )
        }
        ElementCmd::Delete { selector } => delete(opts, m, selector),
        ElementCmd::Retype { selector, r#type } => {
            let c = resolve(m, selector)?;
            let to = element_type(r#type)?;
            let row = retype(m, c, to)?;
            written(m, opts, row)
        }
        ElementCmd::Merge { selector, into, drop_doc } => {
            let a = resolve(m, selector)?;
            let b = resolve(m, into)?;
            let row = merge(m, a, b, !*drop_doc)?;
            written(m, opts, row)
        }
    }
}

/// Retype in memory and describe it: the old and new type, where it is filed
/// now, and how many views draw it — the figure on each of them changes.
pub fn retype(m: &mut Model, c: ConceptId, to: ElementType) -> Result<Row, CliError> {
    let from = m.concept(c).kind.name().to_string();
    m.retype(c, to).map_err(refused)?;
    let views = Graph::build(m).views_of(c).len();
    let concept = m.concept(c);
    Ok(Row::new()
        .s("id", concept.id.clone())
        .s("name", concept.name.clone())
        .s("from", from)
        .s("to", to.info().short)
        .s("folder", m.folder_path_of(concept))
        .n("views", views as i64))
}

/// Merge in memory and describe it by count: relationships repointed,
/// relationships dropped as twins or loops, boxes repointed, and the views
/// that now show the survivor more than once.
pub fn merge(m: &mut Model, a: ConceptId, b: ConceptId, keep_doc: bool) -> Result<Row, CliError> {
    let (aid, bid) = (m.concept(a).id.clone(), m.concept(b).id.clone());
    let done = m.merge_elements(a, b, keep_doc).map_err(refused)?;
    Ok(Row::new()
        .s("merged", aid)
        .s("into", bid)
        .n("relationships", done.relationships.len() as i64)
        .n("dropped", done.dropped.len() as i64)
        .n("objects", done.objects.len() as i64)
        .s("duplicated_on", done.duplicated_on.join(", ")))
}

fn delete(opts: &Opts, m: &mut Model, selector: &str) -> Result<Output, CliError> {
    let c = resolve(m, selector)?;
    let plan = m.delete_plan(c);

    let impact = |p: &amcli_model::Cascade| -> Vec<Row> {
        let mut rows = vec![
            Row::new().s("kind", "relationships").n("count", p.relationships.len() as i64),
            Row::new().s("kind", "diagram_objects").n("count", p.diagram_objects.len() as i64),
            Row::new().s("kind", "connections").n("count", p.connections.len() as i64),
        ];
        rows.retain(|r| !matches!(r.0.get(1), Some((_, crate::output::Value::Num(0)))));
        rows
    };

    // The refusal carries the impact report, so the retry is informed rather
    // than blind.
    if !plan.is_empty() && !opts.yes && !opts.dry_run {
        return Err(CliError::new(
            Code::Invalid,
            "cascade",
            format!(
                "deleting `{}` also removes {} other thing(s)",
                m.concept(c).name,
                plan.total() - 1
            ),
        )
        .hint("re-run with -y to go ahead, or --dry-run to see the detail")
        .rows(impact(&plan)));
    }

    if opts.dry_run {
        let row = Row::new()
            .s("id", m.concept(c).id.clone())
            .b("dry_run", true)
            .n("total", plan.total() as i64)
            .list("impact", impact(&plan));
        return Ok(Output::one(row).note("dry run: nothing was written"));
    }

    let id = m.concept(c).id.clone();
    let done = m.delete_concept(c).map_err(invalid)?;
    let mut row = Row::new()
        .s("id", id)
        .n("total", done.total() as i64)
        .n("relationships", done.relationships.len() as i64)
        .n("diagram_objects", done.diagram_objects.len() as i64)
        .n("connections", done.connections.len() as i64);
    if !done.degenerate_junctions.is_empty() {
        row = row.s("degenerate_junctions", done.degenerate_junctions.join(", "));
    }
    let mut out = written(m, opts, row)?;
    if !done.degenerate_junctions.is_empty() {
        out = out.note(format!(
            "junction(s) now joining fewer than two things: {} — left in place deliberately",
            done.degenerate_junctions.join(", ")
        ));
    }
    Ok(out)
}

fn relation(opts: &Opts, m: &mut Model, cmd: &RelationCmd) -> Result<Output, CliError> {
    match cmd {
        RelationCmd::Add { r#type, source, target, access, doc, name, no_draw } => {
            let ty = rel_type(r#type)?;
            let s = resolve(m, source)?;
            let t = resolve(m, target)?;
            let a = access.as_deref().map(access_value).transpose()?;
            let c = m
                .add_relation(ty, s, t, a, doc.as_deref(), name.as_deref())
                .map_err(|e| invalid(&e).hint("run `amcli get` on either end to see what it is"))?;
            let drawn = if *no_draw { Vec::new() } else { draw(m, c)? };
            let out = written(
                m,
                opts,
                Row::new()
                    .s("id", m.concept(c).id.clone())
                    .s("type", ty.info().short)
                    .s("source", m.concept(s).id.clone())
                    .s("target", m.concept(t).id.clone())
                    .n("views", drawn.len() as i64),
            )?;
            Ok(match drawn_note(m, &[s, t], &drawn) {
                Some(n) => out.note(n),
                None => out,
            })
        }
        RelationCmd::Delete { selector } => delete(opts, m, selector),
        RelationCmd::Doc { selector, text } => {
            let c = resolve(m, selector)?;
            m.set_documentation(c, text).map_err(invalid)?;
            written(m, opts, Row::new().s("id", m.concept(c).id.clone()))
        }
    }
}

/// Draw a relationship on every view that shows both of its ends, and say
/// which views those were.
///
/// A relationship between two things a drawing already shows used to reach
/// no drawing, and nothing said so: the model then held a relationship on
/// no view, which only a separate query revealed, and the one way to draw it
/// was to rebuild the view member by member. `view add` draws every
/// relationship the added element brings to what is there; this is the same
/// rule from the other side.
pub fn draw(m: &mut Model, rel: ConceptId) -> Result<Vec<String>, CliError> {
    let views = m.draw_relation_on_views(rel).map_err(invalid)?;
    Ok(views.into_iter().map(|v| m.view(v).name.clone()).collect())
}

/// What to say about where a relationship was drawn: the views by name, or
/// that no view shows both ends when at least one end is drawn somewhere.
/// Silence when neither end is on any view — a model without drawings has
/// nothing to hear.
pub fn drawn_note(m: &Model, ends: &[ConceptId], drawn: &[String]) -> Option<String> {
    if !drawn.is_empty() {
        return Some(format!("drawn on {} view(s): {}", drawn.len(), drawn.join(", ")));
    }
    let g = Graph::build(m);
    let on_some_view = ends.iter().any(|c| !g.views_of(*c).is_empty());
    on_some_view.then(|| {
        "not drawn: no view shows both ends; `amcli view add` the missing end to draw it there"
            .to_string()
    })
}

fn folder(opts: &Opts, m: &mut Model, cmd: &FolderCmd) -> Result<Output, CliError> {
    match cmd {
        FolderCmd::List => {
            let rows = m
                .folders()
                .map(|f| {
                    Row::new()
                        .s("path", f.path.clone())
                        .s("type", f.folder_type.as_str())
                        .s("id", f.id.clone())
                })
                .collect::<Vec<_>>();
            let total = rows.len();
            Ok(Output::rows(rows).meta_n("total", total as i64))
        }
        FolderCmd::Add { parent, name } => {
            let p = folder_id(m, parent)?;
            let before = m.folders().count();
            let f = m.add_folder(p, name).map_err(invalid)?;
            let created = m.folders().count() > before;
            written(
                m,
                opts,
                Row::new()
                    .s("path", m.folder(f).path.clone())
                    .s("id", m.folder(f).id.clone())
                    .b("created", created),
            )
        }
        FolderCmd::Delete { path } => {
            let f = folder_id(m, path)?;
            let full = m.folder(f).path.clone();
            m.delete_folder(f).map_err(invalid)?;
            written(m, opts, Row::new().s("path", full))
        }
        FolderCmd::Rename { path, name } => {
            let f = folder_id(m, path)?;
            let from = m.folder(f).path.clone();
            m.rename_folder(f, name).map_err(invalid)?;
            written(
                m,
                opts,
                Row::new()
                    .s("from", from)
                    .s("path", m.folder(f).path.clone())
                    .s("id", m.folder(f).id.clone()),
            )
        }
        FolderCmd::Move { path, parent } => {
            let f = folder_id(m, path)?;
            let p = folder_id(m, parent)?;
            let from = m.folder(f).path.clone();
            m.move_folder(f, p).map_err(invalid)?;
            written(
                m,
                opts,
                Row::new()
                    .s("from", from)
                    .s("path", m.folder(f).path.clone())
                    .s("id", m.folder(f).id.clone()),
            )
        }
    }
}

fn prop(opts: &Opts, m: &mut Model, cmd: &PropCmd) -> Result<Output, CliError> {
    match cmd {
        PropCmd::Set { selector, key, value } => {
            let c = resolve(m, selector)?;
            m.set_property(c, key, value).map_err(invalid)?;
            written(
                m,
                opts,
                Row::new()
                    .s("id", m.concept(c).id.clone())
                    .s("key", key.clone())
                    .s("value", value.clone()),
            )
        }
        PropCmd::Unset { selector, key } => {
            let c = resolve(m, selector)?;
            m.remove_property(c, key);
            written(m, opts, Row::new().s("id", m.concept(c).id.clone()).s("key", key.clone()))
        }
        PropCmd::List { selector } => {
            let c = resolve(m, selector)?;
            let rows = m
                .properties(m.concept(c).node)
                .into_iter()
                .map(|(k, v)| Row::new().s("key", k).s("value", v))
                .collect::<Vec<_>>();
            let total = rows.len();
            Ok(Output::rows(rows).meta_n("total", total as i64))
        }
    }
}
