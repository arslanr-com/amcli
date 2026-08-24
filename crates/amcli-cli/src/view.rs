//! View commands: listing, authoring and rendering.

use amcli_graph::{Dir, EdgeFilter, Graph, Resolution, Selector};
use amcli_model::{ConceptId, ConceptKind, Model, ViewId, viewpoints};
use amcli_render::Options;
use amcli_view::geometry::Rect;
use amcli_view::layout::{
    Algorithm, Item, fit_group_size, fit_note_size, fit_size, free_slot, place,
};
use amcli_view::notation::Figure;
use clap::Subcommand;

use crate::output::{CliError, Code, Output, Row};
use crate::write::Opts;

#[derive(Subcommand, Clone)]
pub enum ViewCmd {
    /// List views.
    List,
    /// Create an empty view.
    Create {
        name: String,
        /// One of the 25 ArchiMate viewpoint ids, e.g. layered.
        #[arg(long)]
        viewpoint: Option<String>,
        /// File it here instead of at the top of the views folder, e.g.
        /// `/Views/Motivation`. The folder must already exist.
        #[arg(short = 'f', long)]
        folder: Option<String>,
        /// Delete any view already using this name instead of refusing.
        #[arg(long)]
        replace: bool,
    },
    /// Put a concept on a view, drawing the relationships it brings with it.
    Add {
        view: String,
        selector: String,
        #[arg(long)]
        x: Option<i32>,
        #[arg(long)]
        y: Option<i32>,
        /// Place the box only; do not draw its relationships.
        #[arg(long)]
        no_connect: bool,
    },
    /// Build a view from a concept and its neighbourhood, laid out and wired up.
    Auto {
        name: String,
        /// The concept to start from.
        #[arg(long)]
        from: String,
        #[arg(short = 'n', long, default_value_t = 2)]
        depth: u32,
        #[arg(short = 'D', long, default_value = "both")]
        direction: String,
        /// auto (the default) | layered | grid. `--algorithm` is the same flag.
        #[arg(long, alias = "algorithm", default_value = "auto")]
        layout: String,
        #[arg(long)]
        viewpoint: Option<String>,
        /// File it here instead of at the top of the views folder.
        #[arg(short = 'f', long)]
        folder: Option<String>,
        /// Delete any view already using this name instead of refusing.
        #[arg(long)]
        replace: bool,
    },
    /// Re-place the objects on a view.
    Layout {
        view: String,
        /// auto (the default) | layered | grid. `--layout` is the same flag.
        #[arg(long, alias = "layout", default_value = "auto")]
        algorithm: String,
        /// Move everything, not just objects that have never been placed.
        #[arg(long)]
        relayout_all: bool,
    },
    /// Delete a view. No concept is touched — only the drawing.
    Delete { view: String },
    /// Change a view's name.
    Rename { view: String, name: String },
    /// Replace a view's documentation. An empty string removes it.
    Doc { view: String, text: String },
    /// Set or clear a view's viewpoint.
    Viewpoint {
        view: String,
        /// One of the 25 ArchiMate viewpoint ids, e.g. layered. Empty clears it.
        viewpoint: String,
    },
    /// Re-file a view under another folder in the views tree.
    Move {
        view: String,
        #[arg(short = 'f', long)]
        folder: String,
    },
    /// Draw a view.
    Render {
        view: String,
        /// svg | png | json. Defaults to the extension of `-o`, else svg.
        /// This is `--as`, not the global `-F`: one controls what is drawn,
        /// the other how amcli reports. The field is named `draw_as` because
        /// a second `format` would be merged into the global one by clap.
        #[arg(long = "as")]
        draw_as: Option<String>,
        /// Write here instead of to stdout.
        #[arg(short = 'o', long)]
        out: Option<String>,
        #[arg(long, default_value_t = 10)]
        margin: i32,
        /// For png, the resolution: 2 draws every pixel of the view as two.
        #[arg(long, default_value_t = 1.0)]
        scale: f64,
    },
}

pub fn run(opts: &Opts, m: &mut Model, cmd: &ViewCmd) -> Result<Output, CliError> {
    match cmd {
        ViewCmd::List => list(m),
        ViewCmd::Create { name, viewpoint, folder, replace } => {
            create(opts, m, name, viewpoint.as_deref(), folder.as_deref(), *replace)
        }
        ViewCmd::Add { view, selector, x, y, no_connect } => {
            add(opts, m, view, selector, *x, *y, !*no_connect)
        }
        ViewCmd::Auto { name, from, depth, direction, layout, viewpoint, folder, replace } => auto(
            opts,
            m,
            name,
            from,
            *depth,
            direction,
            layout,
            viewpoint.as_deref(),
            folder.as_deref(),
            *replace,
        ),
        ViewCmd::Layout { view, algorithm, relayout_all } => {
            relayout(opts, m, view, algorithm, *relayout_all)
        }
        ViewCmd::Delete { view } => delete(opts, m, view),
        ViewCmd::Rename { view, name } => rename(opts, m, view, name),
        ViewCmd::Doc { view, text } => doc(opts, m, view, text),
        ViewCmd::Viewpoint { view, viewpoint } => set_viewpoint(opts, m, view, viewpoint),
        ViewCmd::Move { view, folder } => move_view(opts, m, view, folder),
        ViewCmd::Render { view, draw_as, out, margin, scale } => {
            // `-o v.png` says what it wants without a second flag.
            let inferred = out
                .as_deref()
                .and_then(|p| std::path::Path::new(p).extension())
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase());
            let format = draw_as.clone().or(inferred).unwrap_or_else(|| "svg".to_string());
            render(m, view, &format, out.as_deref(), *margin, *scale)
        }
    }
}

/// Make a view name available, or refuse to.
///
/// Creating a second view with the same name used to succeed silently, which
/// left two indistinguishable views behind and no CLI way to remove either. A
/// name clash is a conflict (exit 6) unless the caller says what to do about it.
/// Where a replaced view sat, so its replacement can take the same place.
///
/// `--replace` used to delete and let the new view be appended, which moved it
/// to the end of its folder. That is invisible with three views and fatal to a
/// script that regenerates all of them: each pass reorders the file, so the
/// diff is the whole views section every time and two passes never agree.
type Slot = (amcli_model::FolderId, usize);

fn claim_name(
    m: &mut Model,
    name: &str,
    except: Option<ViewId>,
    replace: bool,
) -> Result<(Vec<String>, Option<Slot>), CliError> {
    let clash: Vec<ViewId> = m
        .views_with_ids()
        .filter(|(i, v)| v.name == name && Some(*i) != except)
        .map(|(i, _)| i)
        .collect();
    if clash.is_empty() {
        return Ok((Vec::new(), None));
    }
    if !replace {
        return Err(CliError::new(
            Code::Conflict,
            "conflict",
            format!("{} view(s) are already called `{name}`", clash.len()),
        )
        .hint("pass --replace to overwrite, choose another name, or `amcli view delete` first")
        .rows(
            clash
                .iter()
                .map(|v| {
                    Row::new()
                        .s("selector", format!("id:{}", m.view(*v).id))
                        .s("name", m.view(*v).name.clone())
                })
                .collect(),
        ));
    }

    // The first clash's position is the one worth keeping: it is the view the
    // caller is regenerating. Read it before the delete, when it still exists.
    let slot = clash.first().and_then(|v| m.view_position(*v));

    let mut replaced = Vec::new();
    for v in clash {
        let id = m.view(v).id.clone();
        m.delete_view(v).map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
        replaced.push(id);
    }
    Ok((replaced, slot))
}

/// Put a freshly made view back where the one it replaced was.
fn reseat(m: &mut Model, v: ViewId, slot: Option<Slot>) {
    if let Some((folder, at)) = slot {
        m.place_view_at(v, folder, at);
    }
}

fn find_view(m: &Model, sel: &str) -> Result<ViewId, CliError> {
    if let Some(id) = sel.strip_prefix("id:").and_then(|i| m.view_by_id(i)) {
        return Ok(id);
    }
    let matches: Vec<(ViewId, String)> = m
        .views_with_ids()
        .filter(|(_, v)| v.name == sel || v.id == sel)
        .map(|(i, v)| (i, v.name.clone()))
        .collect();
    match matches.len() {
        1 => Ok(matches[0].0),
        0 => Err(CliError::new(Code::NotFound, "not_found", format!("no view called `{sel}`"))
            .hint("run `amcli view list`")
            .rows(
                m.views()
                    .map(|v| Row::new().s("id", v.id.clone()).s("name", v.name.clone()))
                    .collect(),
            )),
        _ => Err(CliError::new(
            Code::Ambiguous,
            "ambiguous",
            format!("{} views called `{sel}`", matches.len()),
        )
        .hint("use id:…")
        .rows(
            matches
                .iter()
                .map(|(i, n)| {
                    Row::new().s("selector", format!("id:{}", m.view(*i).id)).s("name", n.clone())
                })
                .collect(),
        )),
    }
}

fn list(m: &Model) -> Result<Output, CliError> {
    let rows: Vec<Row> = m
        .views()
        .map(|v| {
            Row::new()
                .s("id", v.id.clone())
                .s("name", v.name.clone())
                .s("kind", if v.is_sketch { "sketch" } else { "archimate" })
                .s("viewpoint", v.viewpoint.clone())
                .s("folder", m.folder(v.folder).path.clone())
        })
        .collect();
    let total = rows.len();
    Ok(Output::rows(rows).meta_n("total", total as i64))
}

fn check_viewpoint(vp: Option<&str>) -> Result<(), CliError> {
    let Some(v) = vp.filter(|v| !v.is_empty()) else { return Ok(()) };
    if viewpoints::by_id(v).is_some() {
        return Ok(());
    }
    Err(CliError::new(Code::Usage, "usage", format!("`{v}` is not a viewpoint id")).hint(format!(
        "one of: {}",
        viewpoints::VIEWPOINTS.iter().map(|v| v.id).collect::<Vec<_>>().join(", ")
    )))
}

fn create(
    opts: &Opts,
    m: &mut Model,
    name: &str,
    vp: Option<&str>,
    folder: Option<&str>,
    replace: bool,
) -> Result<Output, CliError> {
    check_viewpoint(vp)?;
    // Resolved before anything is created, so a misspelt folder leaves no
    // half-made view behind.
    let dest = folder.map(|f| views_folder(m, f)).transpose()?;
    let (replaced, slot) = claim_name(m, name, None, replace)?;
    let v =
        m.add_view(name, vp).map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
    if let Some(f) = dest {
        m.move_view_to_folder(v, f)
            .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
    }
    reseat(m, v, slot);
    let row = Row::new()
        .s("id", m.view(v).id.clone())
        .s("name", name.to_string())
        .s("folder", m.folder(m.view(v).folder).path.clone())
        .n("replaced", replaced.len() as i64)
        .b("dry_run", opts.dry_run);
    finish(opts, m, row)
}

/// A folder path that must exist and must be in the views tree.
///
/// Both halves are reported the same way `element move` reports a bad path —
/// with every folder listed — because the usual cause is a typo and the usual
/// fix is reading the real name off the list.
fn views_folder(m: &Model, path: &str) -> Result<amcli_model::FolderId, CliError> {
    let f = m.folder_by_path(path).ok_or_else(|| {
        let mut paths: Vec<String> = m.folders().map(|f| f.path.clone()).collect();
        paths.sort();
        CliError::new(Code::NotFound, "not_found", format!("no folder at `{path}`"))
            .hint("existing folders below; create one with `amcli folder add`")
            .rows(paths.into_iter().map(|p| Row::new().s("folder", p)).collect())
    })?;
    if !m.is_views_folder(f) {
        let mut paths: Vec<String> = m
            .folders_with_ids()
            .filter(|(i, _)| m.is_views_folder(*i))
            .map(|(_, f)| f.path.clone())
            .collect();
        paths.sort();
        return Err(CliError::new(
            Code::Invalid,
            "invalid",
            format!(
                "`{path}` is not under the views folder; Archi would not show a view filed there"
            ),
        )
        .hint("views folders below")
        .rows(paths.into_iter().map(|p| Row::new().s("folder", p)).collect()));
    }
    Ok(f)
}

fn move_view(opts: &Opts, m: &mut Model, view: &str, folder: &str) -> Result<Output, CliError> {
    let v = find_view(m, view)?;
    let f = views_folder(m, folder)?;
    let from = m.folder(m.view(v).folder).path.clone();
    m.move_view_to_folder(v, f)
        .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
    let row = Row::new()
        .s("id", m.view(v).id.clone())
        .s("name", m.view(v).name.clone())
        .s("from", from)
        .s("to", m.folder(m.view(v).folder).path.clone())
        .b("dry_run", opts.dry_run);
    finish(opts, m, row)
}

fn rename(opts: &Opts, m: &mut Model, view: &str, name: &str) -> Result<Output, CliError> {
    let v = find_view(m, view)?;
    claim_name(m, name, Some(v), false)?;

    let old = m.view(v).name.clone();
    m.rename_view(v, name);
    let row = Row::new()
        .s("id", m.view(v).id.clone())
        .s("from", old)
        .s("to", name.to_string())
        .b("dry_run", opts.dry_run);
    finish(opts, m, row)
}

/// Replace or clear a view's documentation.
///
/// A drawing is the one thing in a model an agent hands to a person, and the
/// paragraph saying what it is for had nowhere to live: `element doc` takes a
/// concept, and a view is not one. It is read back with
/// `view list --fields name,doc`.
fn doc(opts: &Opts, m: &mut Model, view: &str, text: &str) -> Result<Output, CliError> {
    let v = find_view(m, view)?;
    m.set_view_documentation(v, text)
        .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
    let row = Row::new()
        .s("id", m.view(v).id.clone())
        .n("chars", text.chars().count() as i64)
        .b("dry_run", opts.dry_run);
    finish(opts, m, row)
}

/// Set or clear the viewpoint of a view that already exists.
///
/// Until this, a viewpoint could only be chosen when the view was created, so
/// a drawing that grew past the one it was filed under could not be corrected
/// without deleting and rebuilding it.
fn set_viewpoint(
    opts: &Opts,
    m: &mut Model,
    view: &str,
    viewpoint: &str,
) -> Result<Output, CliError> {
    let v = find_view(m, view)?;
    let vp = viewpoint.trim();
    if !vp.is_empty() {
        check_viewpoint(Some(vp))?;
    }
    let from = m.view(v).viewpoint.clone();
    m.set_view_viewpoint(v, vp);
    let row = Row::new()
        .s("id", m.view(v).id.clone())
        .s("name", m.view(v).name.clone())
        .s("from", from)
        .s("to", vp.to_string())
        .b("dry_run", opts.dry_run);
    finish(opts, m, row)
}

fn delete(opts: &Opts, m: &mut Model, view: &str) -> Result<Output, CliError> {
    let v = find_view(m, view)?;

    // A view drawn *on another view* as a reference box is the one case where
    // deleting this one changes something else, so it gets the same treatment as
    // a cascading concept delete: refuse, and let the refusal be the report.
    let refs = m.view_references(v);
    if !refs.is_empty() && !opts.yes && !opts.dry_run {
        return Err(CliError::new(
            Code::Invalid,
            "cascade",
            format!(
                "`{}` is drawn as a reference on {} other view(s); deleting it removes those boxes too",
                m.view(v).name,
                refs.iter().map(|(view, _)| view).collect::<std::collections::HashSet<_>>().len()
            ),
        )
        .hint("re-run with -y to go ahead, or --dry-run to see the detail")
        .rows(
            refs.iter()
                .map(|(view, object)| {
                    Row::new()
                        .s("on_view", m.view_by_id(view).map(|i| m.view(i).name.clone()).unwrap_or_default())
                        .s("object", object.clone())
                })
                .collect(),
        ));
    }

    let name = m.view(v).name.clone();
    let id = m.view(v).id.clone();
    // Deleting in memory even for a dry run: nothing is written unless the write
    // happens at the end, so this reports exactly what would go.
    let plan =
        m.delete_view(v).map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
    let row = Row::new()
        .s("id", id)
        .s("name", name)
        .n("objects", plan.diagram_objects.len() as i64)
        .n("connections", plan.connections.len() as i64)
        .n("references", refs.len() as i64)
        .b("dry_run", opts.dry_run);
    let out = finish(opts, m, row)?;
    Ok(out.note("no concept was deleted; a view is a drawing of the model, not part of it"))
}

fn finish(opts: &Opts, m: &Model, row: Row) -> Result<Output, CliError> {
    if !opts.dry_run {
        crate::write::save(m)?;
    }
    let out = Output::one(row);
    Ok(if opts.dry_run { out.note("dry run: nothing was written") } else { out })
}

fn resolve(m: &Model, sel: &str) -> Result<ConceptId, CliError> {
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

/// Warn rather than refuse when a concept is outside the view's viewpoint.
///
/// Archi ghosts non-conforming elements instead of blocking, and an agent
/// mid-task should not be stopped by a modelling convention.
fn viewpoint_note(m: &Model, view: ViewId, c: ConceptId) -> Option<String> {
    let vp = &m.view(view).viewpoint;
    if vp.is_empty() {
        return None;
    }
    let ConceptKind::Element(e) = &m.concept(c).kind else { return None };
    (!viewpoints::allows(vp, *e))
        .then(|| format!("viewpoint `{vp}` does not cover {}; added anyway", e.info().short))
}

/// Relationships that become drawable once `objects` are on the view, as
/// (relationship, source object, target object).
///
/// This is what `view auto` does for a whole neighbourhood, applied to whatever
/// is on the view now. Without it, `view add` left a floating box even when its
/// counterpart was right there on the same diagram — and no amount of
/// re-laying-out could fix that, because the connection was never in the file.
fn induced_connections(
    m: &Model,
    v: ViewId,
    objects: &[ConceptId],
) -> Vec<(ConceptId, String, String)> {
    let g = Graph::build(m);

    // First object per concept. A concept may appear on a view more than once;
    // drawing one line rather than one per copy is what Archi does when you drop
    // an element onto a diagram.
    let mut object_of: std::collections::HashMap<String, String> = Default::default();
    for (object, concept) in m.view_objects(v) {
        if let Some(concept) = concept {
            object_of.entry(concept).or_insert(object);
        }
    }

    // Relationships already drawn here, so re-running is a no-op.
    let drawn: std::collections::HashSet<String> = m
        .doc
        .descendants(m.view(v).node)
        .into_iter()
        .filter_map(|n| m.doc.attr(n, "archimateRelationship"))
        .collect();

    let mut out: Vec<(ConceptId, String, String)> = Vec::new();
    let mut seen: std::collections::HashSet<String> = Default::default();
    for c in objects {
        for arc in g.neighbors(*c, Dir::Both, &EdgeFilter::default()) {
            let rel = m.concept(arc.rel);
            if drawn.contains(&rel.id) || !seen.insert(rel.id.clone()) {
                continue;
            }
            // The connection's ends are the *objects*, and which is source is
            // the relationship's business, not the traversal's.
            let Some((s, t)) = g.ends(arc.rel) else { continue };
            let (Some(src), Some(tgt)) =
                (object_of.get(&m.concept(s).id), object_of.get(&m.concept(t).id))
            else {
                continue;
            };
            out.push((arc.rel, src.clone(), tgt.clone()));
        }
    }
    out
}

#[allow(clippy::too_many_arguments)] // one parameter per CLI flag
fn add(
    opts: &Opts,
    m: &mut Model,
    view: &str,
    sel: &str,
    x: Option<i32>,
    y: Option<i32>,
    connect: bool,
) -> Result<Output, CliError> {
    let v = find_view(m, view)?;
    let c = resolve(m, sel)?;
    let note = viewpoint_note(m, v, c);

    let (w, h) = match &m.concept(c).kind {
        ConceptKind::Element(e) => e.info().default_wh,
        _ => (120, 55),
    };
    // Placed clear of everything already there, so adding one object never
    // disturbs the rest of the diagram.
    let taken: Vec<Rect> = amcli_view::compile(m, v).nodes.iter().map(|n| n.abs).collect();
    let slot = match (x, y) {
        (Some(x), Some(y)) => Rect { x, y, w, h },
        _ => free_slot(&taken, w, h),
    };

    let id = m
        .add_view_object(v, c, slot.x, slot.y, slot.w, slot.h)
        .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;

    // Gather, then mutate: the graph borrows the model.
    let wire = if connect { induced_connections(m, v, &[c]) } else { Vec::new() };
    let mut drawn = 0;
    for (rel, src, tgt) in wire {
        if m.add_view_connection(v, rel, &src, &tgt, &[]).is_ok() {
            drawn += 1;
        }
    }

    let row = Row::new()
        .s("object", id)
        .s("concept", m.concept(c).id.clone())
        .n("x", slot.x as i64)
        .n("y", slot.y as i64)
        .n("connections", drawn)
        .b("dry_run", opts.dry_run);
    let out = finish(opts, m, row)?;
    let out = match note {
        Some(n) => out.note(n),
        None => out,
    };
    Ok(if drawn > 0 {
        out.note(format!(
            "drew {drawn} relationship(s) to what was already there; \
             `amcli view layout {view} --relayout-all` will tidy the placement"
        ))
    } else {
        out
    })
}

#[allow(clippy::too_many_arguments)] // one parameter per CLI flag; grouping them would only hide the surface
fn auto(
    opts: &Opts,
    m: &mut Model,
    name: &str,
    from: &str,
    depth: u32,
    dir: &str,
    algorithm: &str,
    vp: Option<&str>,
    folder: Option<&str>,
    replace: bool,
) -> Result<Output, CliError> {
    check_viewpoint(vp)?;
    let dest = folder.map(|f| views_folder(m, f)).transpose()?;
    let algo = parse_algorithm(algorithm)?;
    let dir = Dir::parse(dir).ok_or_else(|| {
        CliError::new(Code::Usage, "usage", format!("`{dir}` is not a direction"))
            .hint("one of: out, in, both")
    })?;
    let (replaced, slot) = claim_name(m, name, None, replace)?;

    // Gather first, mutate second: the graph borrows the model.
    let (items, edges, concepts, rels) = {
        let g = Graph::build(m);
        let root = match Selector::parse(from).resolve_one(&g) {
            Resolution::One(c) => c,
            _ => {
                return Err(CliError::new(
                    Code::NotFound,
                    "not_found",
                    format!("nothing matches `{from}`"),
                ));
            }
        };
        let sub = g.k_hop(&[root], depth, dir, &EdgeFilter::default(), 500);
        let concepts: Vec<ConceptId> = sub.nodes.iter().map(|(c, _)| *c).collect();

        let items: Vec<Item> = concepts
            .iter()
            .map(|c| {
                let concept = m.concept(*c);
                let (w, h) = match &concept.kind {
                    ConceptKind::Element(e) => e.info().default_wh,
                    _ => (120, 55),
                };
                // Sized to the label, unless the figure has its own size — a
                // junction is a small circle whatever it is called.
                let (w, h) = if (w, h) == (120, 55) { fit_size(&concept.name) } else { (w, h) };
                Item { id: concept.id.clone(), name: concept.name.clone(), w, h }
            })
            .collect();

        let index = |c: ConceptId| concepts.iter().position(|x| *x == c);
        let mut edges = Vec::new();
        let mut rels = Vec::new();
        for e in &sub.edges {
            if let Some((s, t)) = g.ends(*e)
                && let (Some(a), Some(b)) = (index(s), index(t))
            {
                edges.push((a, b));
                rels.push((*e, a, b));
            }
        }
        (items, edges, concepts, rels)
    };

    if concepts.is_empty() {
        return Err(CliError::new(Code::NotFound, "not_found", "nothing to put on the view"));
    }

    let placed = place(&items, &edges, algo);
    let v =
        m.add_view(name, vp).map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
    if let Some(f) = dest {
        m.move_view_to_folder(v, f)
            .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
    }
    reseat(m, v, slot);

    let mut object_ids = Vec::with_capacity(concepts.len());
    for (c, r) in concepts.iter().zip(placed.rects.iter()) {
        let id = m
            .add_view_object(v, *c, r.x, r.y, r.w, r.h)
            .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
        object_ids.push(id);
    }

    // Every connection is a straight line: the layout keeps lines off boxes
    // by where it puts the boxes, and writes no bendpoints.
    let mut drawn = 0;
    for (rel, a, b) in rels {
        if m.add_view_connection(v, rel, &object_ids[a], &object_ids[b], &[]).is_ok() {
            drawn += 1;
        }
    }

    let row = Row::new()
        .s("id", m.view(v).id.clone())
        .s("name", name.to_string())
        .s("folder", m.folder(m.view(v).folder).path.clone())
        .n("objects", object_ids.len() as i64)
        .n("connections", drawn)
        // Which algorithm ran, because under `auto` it may not be the one the
        // caller would have guessed.
        .s("algorithm", placed.algorithm.as_str())
        .n("replaced", replaced.len() as i64)
        .b("dry_run", opts.dry_run);
    let out = finish(opts, m, row)?;
    Ok(fallback_note(out, algo, placed.algorithm))
}

fn parse_algorithm(name: &str) -> Result<Algorithm, CliError> {
    Algorithm::parse(name).ok_or_else(|| {
        CliError::new(Code::Usage, "usage", format!("`{name}` is not a layout"))
            .hint(format!("one of: {}", Algorithm::NAMES))
    })
}

/// Say so when `auto` declined to layer, rather than leaving someone to wonder
/// why the diagram came out as a grid.
fn fallback_note(out: Output, asked: Algorithm, used: Algorithm) -> Output {
    if asked == Algorithm::Auto && used == Algorithm::Grid {
        return out.note(
            "this graph is too wide and shallow to layer usefully, so it was laid out as a \
             grid; pass --layout layered to force layering anyway",
        );
    }
    out
}

fn relayout(
    opts: &Opts,
    m: &mut Model,
    view: &str,
    algorithm: &str,
    all: bool,
) -> Result<Output, CliError> {
    let v = find_view(m, view)?;
    let algo = parse_algorithm(algorithm)?;

    // Only objects that have never been placed move, unless told otherwise.
    // Reflowing everything by default is how one added element turns into a
    // four-hundred-line diff.
    let scene = amcli_view::compile(m, v);
    let movable: Vec<Item> = scene
        .nodes
        .iter()
        .filter(|n| all || (n.abs.x == 0 && n.abs.y == 0))
        // The label has to come from the node being moved. Indexing the scene by
        // the *filtered* position read some other node's name, which fed the
        // wrong sort key into a layout that is otherwise deterministic.
        // Every box being moved is also sized to its label. A box the user
        // widened by hand is a box being relaid, and it comes back at the
        // width its name needs; a junction and other small figures keep
        // theirs.
        .map(|n| {
            let (w, h) = if n.abs.w >= 60 && n.abs.h >= 30 {
                // A note and a group carry no type icon, so Archi leaves their
                // text the whole box less its margin; an element loses the
                // icon's width off both sides.
                match n.figure {
                    Figure::Tabbed => fit_group_size(&n.label),
                    Figure::Note => fit_note_size(&n.label),
                    _ => fit_size(&n.label),
                }
            } else {
                (n.abs.w, n.abs.h)
            };
            Item { id: n.id.clone(), name: n.label.clone(), w, h }
        })
        .collect();

    if movable.is_empty() {
        return Ok(Output::empty().note("nothing to move; pass --relayout-all to reflow the view"));
    }

    // The edges between the objects being moved. Without them every layered
    // relayout saw an edgeless graph, ranked everything at zero, and produced
    // one enormous row — the layout was never given the chance to do its job.
    let (edges, connections) = edges_between(m, v, &movable);
    let placed = place(&movable, &edges, algo);

    // The layout may have resized a box — to fit its label, or widened a hub
    // so its many edges hang straight — so the size goes back as well as the
    // position.
    for (item, r) in movable.iter().zip(placed.rects.iter()) {
        m.set_view_object_rect(v, &item.id, r.x, r.y, Some((r.w, r.h)))
            .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
    }
    // And every connection among them is straightened. Moving the boxes and
    // leaving old bendpoints where they were drew each such line through
    // whatever now sat on its former path; a relaid view has straight lines,
    // and the layout is what keeps them off the boxes.
    for (conn_id, _, _) in &connections {
        m.set_view_connection_bendpoints(v, conn_id, &[])
            .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
    }

    let row = Row::new()
        .s("view", m.view(v).id.clone())
        .n("moved", movable.len() as i64)
        .n("edges", edges.len() as i64)
        .s("algorithm", placed.algorithm.as_str())
        .b("dry_run", opts.dry_run);
    let out = finish(opts, m, row)?;
    Ok(fallback_note(out, algo, placed.algorithm))
}

/// A connection on the view: its id, and its endpoints as indices into the
/// items being laid out.
type Connection = (String, usize, usize);

/// The connections drawn among the given diagram objects, as index pairs into
/// `items` — one edge per connection, in document order — with each
/// connection's id and endpoints alongside so its routing can be written back.
///
/// Read from the view rather than from the model graph: what is drawn is the
/// view's connections, and a concept placed twice on one view has two objects
/// and two sets of lines.
fn edges_between(m: &Model, v: ViewId, items: &[Item]) -> (Vec<(usize, usize)>, Vec<Connection>) {
    let index: std::collections::HashMap<&str, usize> =
        items.iter().enumerate().map(|(i, it)| (it.id.as_str(), i)).collect();
    let mut edges = Vec::new();
    let mut connections = Vec::new();
    for (id, src, tgt) in m.view_connections(v) {
        if let (Some(&a), Some(&b)) = (index.get(src.as_str()), index.get(tgt.as_str())) {
            edges.push((a, b));
            connections.push((id, a, b));
        }
    }
    (edges, connections)
}

fn render(
    m: &Model,
    view: &str,
    format: &str,
    out_path: Option<&str>,
    margin: i32,
    scale: f64,
) -> Result<Output, CliError> {
    let v = find_view(m, view)?;
    let scene = amcli_view::compile(m, v);

    let body: Vec<u8> = match format {
        "svg" => amcli_render::svg(&scene, &Options { margin, scale, ..Default::default() }).into(),
        "json" => amcli_render::scene_json(&scene).into(),
        "png" => amcli_render::png(&scene, &Options { margin, scale, ..Default::default() })
            .map_err(|e| CliError::new(Code::Unsupported, "unsupported", e))?,
        other => {
            return Err(CliError::new(
                Code::Unsupported,
                "unsupported",
                format!("`{other}` is not a render format"),
            )
            .hint("svg, png or json"));
        }
    };

    match out_path {
        Some(p) => {
            std::fs::write(p, &body)
                .map_err(|e| CliError::new(Code::Io, "io", format!("`{p}`: {e}")))?;
            let mut o = Output::one(
                Row::new()
                    .s("path", p.to_string())
                    .n("bytes", body.len() as i64)
                    .n("nodes", scene.nodes.len() as i64)
                    .n("edges", scene.edges.len() as i64),
            );
            for w in &scene.warnings {
                o = o.note(w.clone());
            }
            Ok(o)
        }
        None => {
            // The drawing itself is the output, so it goes to stdout raw.
            use std::io::Write;
            let mut stdout = std::io::stdout().lock();
            let _ = stdout.write_all(&body);
            let _ = stdout.flush();
            let mut o = Output::empty();
            for w in &scene.warnings {
                o = o.note(w.clone());
            }
            Ok(o)
        }
    }
}
