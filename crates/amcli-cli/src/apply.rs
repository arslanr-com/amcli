//! Atomic batches.
//!
//! An agent building a coherent subgraph needs many edits to land together or
//! not at all. Everything here is applied in memory, validated as a whole, and
//! written exactly once — so "rollback" is the absence of a mechanism rather
//! than a mechanism, which is the only kind that cannot itself fail.
//!
//! Two features carry the design. `ref` names a line's result so a later line
//! can point at it before its id exists, which is what makes a batch composable
//! at all. `if_absent` binds the ref to an existing concept instead of failing,
//! which makes a batch re-runnable after a half-finished attempt — and
//! `if_present` does the same for the ops that delete, so that a batch which
//! *replaces* something can be re-run too.
//!
//! View operations ride along. A view built member by member — create it,
//! add each element, lay it out — is a dozen or a hundred commands, and run
//! one at a time each parses and writes the whole file and a failure halfway
//! leaves the view half drawn. In a batch they land together with the
//! concept edits they belong to, once, or not at all, and `--dry-run` covers
//! them too. They reuse the `view` subcommand's own code, told not to write.

use std::collections::HashMap;
use std::io::Read;

use amcli_graph::{Graph, Resolution, Selector};
use amcli_model::{ConceptId, ElementType, Model, RelType};
use serde::Deserialize;

use crate::output::{CliError, Code, Output, Row};
use crate::view::ViewCmd;
use crate::write::{Opts, guard_checksum, save};

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
enum Op {
    #[serde(rename = "element.add")]
    ElementAdd {
        #[serde(rename = "type")]
        ty: String,
        name: String,
        folder: Option<String>,
        doc: Option<String>,
        #[serde(default)]
        props: HashMap<String, String>,
        #[serde(rename = "ref")]
        reference: Option<String>,
        #[serde(default)]
        if_absent: bool,
    },
    #[serde(rename = "relation.add")]
    RelationAdd {
        #[serde(rename = "type")]
        ty: String,
        source: String,
        target: String,
        access: Option<String>,
        doc: Option<String>,
        #[serde(rename = "ref")]
        reference: Option<String>,
        #[serde(default)]
        if_absent: bool,
    },
    #[serde(rename = "element.rename")]
    ElementRename { target: String, name: String },
    #[serde(rename = "element.doc")]
    ElementDoc { target: String, text: String },
    #[serde(rename = "element.delete")]
    ElementDelete {
        target: String,
        #[serde(default)]
        if_present: bool,
    },
    #[serde(rename = "relation.delete")]
    RelationDelete {
        target: String,
        #[serde(default)]
        if_present: bool,
    },
    #[serde(rename = "prop.set")]
    PropSet { target: String, key: String, value: String },
    #[serde(rename = "prop.unset")]
    PropUnset { target: String, key: String },
    #[serde(rename = "folder.add")]
    FolderAdd { parent: String, name: String },
    #[serde(rename = "folder.delete")]
    FolderDelete { path: String },

    // View operations. Each mirrors a `view` subcommand and takes the same
    // names for the same things; a concept may be given as `ref:name`.
    #[serde(rename = "view.create")]
    ViewCreate {
        name: String,
        viewpoint: Option<String>,
        folder: Option<String>,
        #[serde(default)]
        replace: bool,
    },
    #[serde(rename = "view.add")]
    ViewAdd {
        view: String,
        target: String,
        x: Option<i32>,
        y: Option<i32>,
        #[serde(default)]
        no_connect: bool,
    },
    #[serde(rename = "view.auto")]
    ViewAuto {
        name: String,
        from: String,
        #[serde(default = "default_depth")]
        depth: u32,
        #[serde(default = "default_direction")]
        direction: String,
        #[serde(default = "default_layout")]
        layout: String,
        viewpoint: Option<String>,
        folder: Option<String>,
        #[serde(default)]
        replace: bool,
    },
    #[serde(rename = "view.layout")]
    ViewLayout {
        view: String,
        #[serde(default = "default_layout")]
        algorithm: String,
        #[serde(default)]
        relayout_all: bool,
    },
    #[serde(rename = "view.delete")]
    ViewDelete { view: String },
    #[serde(rename = "view.rename")]
    ViewRename { view: String, name: String },
    #[serde(rename = "view.move")]
    ViewMove { view: String, folder: String },
    #[serde(rename = "view.viewpoint")]
    ViewViewpoint { view: String, viewpoint: String },
    #[serde(rename = "view.doc")]
    ViewDoc { view: String, text: String },
}

fn default_depth() -> u32 {
    2
}
fn default_direction() -> String {
    "both".into()
}
fn default_layout() -> String {
    "auto".into()
}

pub fn run(opts: &Opts, m: &mut Model, file: Option<&str>) -> Result<Output, CliError> {
    guard_checksum(m, opts)?;
    let before = m.checksum().map_err(|e| CliError::new(Code::Io, "io", e.to_string()))?;

    let text = match file {
        None | Some("-") => {
            let mut s = String::new();
            std::io::stdin()
                .read_to_string(&mut s)
                .map_err(|e| CliError::new(Code::Io, "io", e.to_string()))?;
            s
        }
        Some(p) => std::fs::read_to_string(p)
            .map_err(|e| CliError::new(Code::Io, "io", format!("`{p}`: {e}")))?,
    };

    let mut refs: HashMap<String, String> = HashMap::new();
    let mut rows: Vec<Row> = Vec::new();

    for (i, line) in text.lines().enumerate() {
        let n = i + 1;
        let line = line.trim();
        if line.is_empty() || line.starts_with("//") || line.starts_with('#') {
            continue;
        }
        let op: Op = serde_json::from_str(line).map_err(|e| {
            let complaint = e.to_string();
            // An op this binary has never heard of is the same skew
            // `parse_or_hint` answers for subcommands: the skill ships from the
            // branch and the binary from the newest tag, so a batch written
            // against the newer document reaches an older binary. Say so, or
            // the reader goes looking for a typo that is not there.
            let hint = if complaint.starts_with("unknown variant") {
                format!(
                    "one JSON operation per line; see references/batch.md. This amcli is \
                     {}. If a skill named that operation, this binary is older than that \
                     document — upgrade it: sh ~/.agents/skills/amcli/scripts/install.sh",
                    crate::VERSION
                )
            } else {
                "one JSON operation per line; see references/batch.md".to_string()
            };
            CliError::new(Code::Usage, "usage", format!("line {n}: {complaint}")).hint(hint)
        })?;

        match apply_one(opts, m, &op, &mut refs) {
            Ok(row) => rows.push(row.n("line", n as i64)),
            Err(e) => {
                // The in-memory model may well have changed by now — earlier
                // lines applied — but `save` is only ever called after the
                // whole batch succeeds, so the file on disk is untouched. Say
                // exactly that and nothing more.
                return Err(CliError::new(e.code, e.kind, format!("line {n}: {}", e.message))
                    .hint(format!(
                        "{}; the file was not written, so re-run the whole batch",
                        e.hint.unwrap_or_else(|| "the batch was abandoned".into())
                    ))
                    .rows(e.rows));
            }
        }
    }

    if rows.is_empty() {
        return Ok(Output::empty().note("no operations"));
    }

    if !opts.dry_run {
        save(m)?;
    }
    let after = m.checksum().map_err(|e| CliError::new(Code::Io, "io", e.to_string()))?;
    let applied = rows.len();
    let mut out = Output::rows(rows)
        .meta_n("applied", applied as i64)
        .meta("checksum_before", before)
        .meta("checksum_after", after)
        .meta_b("written", !opts.dry_run);
    if opts.dry_run {
        out = out.note("dry run: nothing was written");
    }
    Ok(out)
}

fn apply_one(
    opts: &Opts,
    m: &mut Model,
    op: &Op,
    refs: &mut HashMap<String, String>,
) -> Result<Row, CliError> {
    match op {
        Op::ElementAdd { ty, name, folder, doc, props, reference, if_absent } => {
            let t = ElementType::from_str(ty).ok_or_else(|| {
                CliError::new(Code::Usage, "usage", format!("`{ty}` is not an element type"))
            })?;

            let existing = if *if_absent { find_by_type_and_name(m, ty, name) } else { None };
            let (c, created) = match existing {
                Some(c) => (c, false),
                None => {
                    let f = folder.as_deref().map(|p| folder_of(m, p)).transpose()?;
                    let c = m
                        .add_element(t, name, f, doc.as_deref())
                        .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
                    (c, true)
                }
            };
            // By key, not in `HashMap` order. A JSON object has no order to
            // preserve, and `HashMap` iteration is randomised per process — so
            // applying the same batch twice wrote the same properties in a
            // different order, and a rebuild that changed nothing still produced
            // a diff. Deterministic ids do not help if the lines around them
            // move.
            let mut props: Vec<(&String, &String)> = props.iter().collect();
            props.sort();
            for (k, v) in props {
                m.set_property(c, k, v)
                    .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
            }
            let id = m.concept(c).id.clone();
            if let Some(r) = reference {
                refs.insert(r.clone(), id.clone());
            }
            Ok(Row::new().s("op", "element.add").s("id", id).b("created", created))
        }

        Op::RelationAdd { ty, source, target, access, doc, reference, if_absent } => {
            let t = RelType::from_str(ty).ok_or_else(|| {
                CliError::new(Code::Usage, "usage", format!("`{ty}` is not a relationship type"))
            })?;
            let s = resolve(m, source, refs)?;
            let g = resolve(m, target, refs)?;
            let a = access.as_deref().map(access_value).transpose()?;

            if *if_absent && m.check_relationship(t, s, g).is_err() {
                // Already there, or not permitted. Only the first is a reason
                // to skip quietly, so the check is repeated to tell them apart.
                if let Err(amcli_model::EditError::DuplicateRelationship { existing, .. }) =
                    m.check_relationship(t, s, g)
                {
                    if let Some(r) = reference {
                        refs.insert(r.clone(), existing.clone());
                    }
                    return Ok(Row::new()
                        .s("op", "relation.add")
                        .s("id", existing)
                        .b("created", false));
                }
            }

            let c = m
                .add_relation(t, s, g, a, doc.as_deref())
                .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
            let id = m.concept(c).id.clone();
            if let Some(r) = reference {
                refs.insert(r.clone(), id.clone());
            }
            Ok(Row::new().s("op", "relation.add").s("id", id).b("created", true))
        }

        Op::ElementRename { target, name } => {
            let c = resolve(m, target, refs)?;
            m.rename(c, name);
            Ok(Row::new().s("op", "element.rename").s("id", m.concept(c).id.clone()))
        }
        Op::ElementDoc { target, text } => {
            let c = resolve(m, target, refs)?;
            m.set_documentation(c, text)
                .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
            Ok(Row::new().s("op", "element.doc").s("id", m.concept(c).id.clone()))
        }
        Op::ElementDelete { target, if_present } => {
            delete(m, refs, "element.delete", target, *if_present, None)
        }
        Op::RelationDelete { target, if_present } => {
            // Deleting a relationship used to mean `element.delete` aimed at
            // one, which reads as a mistake and silently accepts a real one.
            // Named for what it deletes, it can insist on it instead.
            delete(m, refs, "relation.delete", target, *if_present, Some(true))
        }
        Op::PropSet { target, key, value } => {
            let c = resolve(m, target, refs)?;
            m.set_property(c, key, value)
                .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
            Ok(Row::new()
                .s("op", "prop.set")
                .s("id", m.concept(c).id.clone())
                .s("key", key.clone()))
        }
        Op::PropUnset { target, key } => {
            let c = resolve(m, target, refs)?;
            // No `if_present`: a key that is not there is already what this
            // asks for, and a concept that is not there is a broken batch.
            let had = m.properties(m.concept(c).node).iter().any(|(k, _)| k == key);
            m.remove_property(c, key);
            Ok(Row::new()
                .s("op", "prop.unset")
                .s("id", m.concept(c).id.clone())
                .s("key", key.clone())
                .b("removed", had))
        }
        Op::ViewCreate { name, viewpoint, folder, replace } => view_op(
            opts,
            m,
            "view.create",
            ViewCmd::Create {
                name: name.clone(),
                viewpoint: viewpoint.clone(),
                folder: folder.clone(),
                replace: *replace,
            },
        ),
        Op::ViewAdd { view, target, x, y, no_connect } => {
            let selector = deref(m, refs, target)?;
            view_op(
                opts,
                m,
                "view.add",
                ViewCmd::Add {
                    view: view.clone(),
                    selector,
                    x: *x,
                    y: *y,
                    no_connect: *no_connect,
                },
            )
        }
        Op::ViewAuto { name, from, depth, direction, layout, viewpoint, folder, replace } => {
            let from = deref(m, refs, from)?;
            view_op(
                opts,
                m,
                "view.auto",
                ViewCmd::Auto {
                    name: name.clone(),
                    from,
                    depth: *depth,
                    direction: direction.clone(),
                    layout: layout.clone(),
                    viewpoint: viewpoint.clone(),
                    folder: folder.clone(),
                    replace: *replace,
                },
            )
        }
        Op::ViewLayout { view, algorithm, relayout_all } => view_op(
            opts,
            m,
            "view.layout",
            ViewCmd::Layout {
                view: view.clone(),
                algorithm: algorithm.clone(),
                relayout_all: *relayout_all,
            },
        ),
        Op::ViewDelete { view } => {
            view_op(opts, m, "view.delete", ViewCmd::Delete { view: view.clone() })
        }
        Op::ViewRename { view, name } => view_op(
            opts,
            m,
            "view.rename",
            ViewCmd::Rename { view: view.clone(), name: name.clone() },
        ),
        Op::ViewMove { view, folder } => view_op(
            opts,
            m,
            "view.move",
            ViewCmd::Move { view: view.clone(), folder: folder.clone() },
        ),
        Op::ViewViewpoint { view, viewpoint } => view_op(
            opts,
            m,
            "view.viewpoint",
            ViewCmd::Viewpoint { view: view.clone(), viewpoint: viewpoint.clone() },
        ),
        Op::ViewDoc { view, text } => {
            view_op(opts, m, "view.doc", ViewCmd::Doc { view: view.clone(), text: text.clone() })
        }
        Op::FolderAdd { parent, name } => {
            let p = folder_of(m, parent)?;
            let before = m.folders().count();
            let f = m
                .add_folder(p, name)
                .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
            let created = m.folders().count() > before;
            Ok(Row::new()
                .s("op", "folder.add")
                .s("path", m.folder(f).path.clone())
                .b("created", created))
        }
        Op::FolderDelete { path } => {
            let f = folder_of(m, path)?;
            let full = m.folder(f).path.clone();
            m.delete_folder(f)
                .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
            Ok(Row::new().s("op", "folder.delete").s("path", full))
        }
    }
}

/// One delete, for both ops that do one.
///
/// `want_relationship` is what tells them apart: `relation.delete` refuses
/// anything that is not a relationship, because a batch is machine-written and
/// aiming it at an element by accident would take the element's whole cascade
/// with it. `element.delete` stays permissive — it is how relationships were
/// deleted before this op existed, and breaking those batches would buy
/// nothing.
///
/// A skipped delete reports no id and `removed` 0. Nothing else can report 0:
/// a delete that happens always removes at least the concept itself.
fn delete(
    m: &mut Model,
    refs: &HashMap<String, String>,
    op: &'static str,
    target: &str,
    if_present: bool,
    want_relationship: Option<bool>,
) -> Result<Row, CliError> {
    let Some(c) = optional(m, target, refs, if_present)? else {
        return Ok(Row::new().s("op", op).s("id", String::new()).n("removed", 0));
    };
    if let Some(want) = want_relationship
        && m.concept(c).kind.is_relationship() != want
    {
        return Err(CliError::new(
            Code::Usage,
            "usage",
            format!("`{target}` is not a relationship"),
        )
        .hint(
            "`relation.delete` takes the relationship, by id or by `ref:`; for an element \
             use `element.delete`",
        ));
    }
    let id = m.concept(c).id.clone();
    let done =
        m.delete_concept(c).map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
    Ok(Row::new().s("op", op).s("id", id).n("removed", done.total() as i64))
}

/// `resolve`, except that `if_present` turns "nothing matches" into a skip.
///
/// This is the counterpart of `if_absent`, and it is what makes a batch that
/// replaces one relationship with another re-runnable: the second run finds
/// the old one already gone and the new one already there, and writes nothing.
///
/// Two misses are never skipped. An ambiguous selector still fails — the thing
/// is there and the batch has not said which one — and so does a `ref:`, which
/// names something an earlier line in this same batch was supposed to produce,
/// so a miss is a typo rather than a state the model might be in.
fn optional(
    m: &Model,
    sel: &str,
    refs: &HashMap<String, String>,
    if_present: bool,
) -> Result<Option<ConceptId>, CliError> {
    match resolve(m, sel, refs) {
        Ok(c) => Ok(Some(c)),
        Err(e) if if_present && e.code == Code::NotFound && !sel.starts_with("ref:") => Ok(None),
        Err(e) => Err(e),
    }
}

fn find_by_type_and_name(m: &Model, ty: &str, name: &str) -> Option<ConceptId> {
    m.concepts_with_ids()
        .find(|(_, c)| c.name == name && c.kind.name().eq_ignore_ascii_case(ty))
        .map(|(i, _)| i)
}

/// `ref:name` refers to something an earlier line produced; anything else is an
/// ordinary selector. Refs resolve forwards only, so a typo is an error at the
/// line that used it rather than a mystery later on.
/// Run one `view` subcommand against the in-memory model without writing.
///
/// The batch writes once at the end, so the command is handed a copy of the
/// options marked dry-run: it does everything it would do at the prompt except
/// save, and its row comes back with the op named the way the batch names it.
fn view_op(opts: &Opts, m: &mut Model, op: &str, cmd: ViewCmd) -> Result<Row, CliError> {
    let deferred = Opts { dry_run: true, yes: opts.yes, expect_checksum: None };
    let out = crate::view::run(&deferred, m, &cmd)?;
    let row = out.rows.into_iter().next().unwrap_or_default();
    // The command's own `dry_run` column would say `true` on a batch that is
    // about to write; the batch reports `written` for all its lines at once.
    Ok(row.without("dry_run").s("op", op))
}

/// A concept selector for a view op: `ref:name` becomes the id an earlier line
/// bound, checked the same way `resolve` checks it; anything else passes
/// through and the view command resolves it itself.
fn deref(m: &Model, refs: &HashMap<String, String>, sel: &str) -> Result<String, CliError> {
    if sel.starts_with("ref:") {
        let c = resolve(m, sel, refs)?;
        return Ok(format!("id:{}", m.concept(c).id));
    }
    Ok(sel.to_string())
}

fn resolve(m: &Model, sel: &str, refs: &HashMap<String, String>) -> Result<ConceptId, CliError> {
    if let Some(name) = sel.strip_prefix("ref:") {
        let id = refs.get(name).ok_or_else(|| {
            CliError::new(Code::NotFound, "not_found", format!("no earlier line named `{name}`"))
                .hint("a ref must be defined by a previous line")
        })?;
        return m.concept_by_id(id).ok_or_else(|| {
            CliError::new(
                Code::NotFound,
                "not_found",
                format!("ref `{name}` points at a deleted concept"),
            )
        });
    }
    let g = Graph::build(m);
    match Selector::parse(sel).resolve_one(&g) {
        Resolution::One(c) => Ok(c),
        Resolution::Ambiguous(cs) => Err(CliError::new(
            Code::Ambiguous,
            "ambiguous",
            format!("{} concepts match `{sel}`", cs.len()),
        )
        .rows(
            cs.iter()
                .map(|c| Row::new().s("selector", format!("id:{}", m.concept(*c).id)))
                .collect(),
        )),
        Resolution::NotFound { .. } => {
            Err(CliError::new(Code::NotFound, "not_found", format!("nothing matches `{sel}`")))
        }
    }
}

fn folder_of(m: &Model, path: &str) -> Result<amcli_model::FolderId, CliError> {
    m.folder_by_path(path)
        .ok_or_else(|| CliError::new(Code::NotFound, "not_found", format!("no folder at `{path}`")))
}

fn access_value(s: &str) -> Result<i64, CliError> {
    Ok(match s {
        "write" | "w" => 0,
        "read" | "r" => 1,
        "unspecified" | "none" => 2,
        "rw" | "readwrite" => 3,
        _ => {
            return Err(CliError::new(
                Code::Usage,
                "usage",
                format!("`{s}` is not an access type"),
            ));
        }
    })
}
