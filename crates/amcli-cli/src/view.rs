//! View commands: listing, authoring and rendering.

use amcli_graph::{Dir, EdgeFilter, Graph};
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

// A command is built once per run; boxing its largest variant would buy nothing.
#[allow(clippy::large_enum_variant)]
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
    /// A concept already there is left where it is.
    Add {
        view: String,
        selector: String,
        /// Draw it inside this object: a concept already on the view, a
        /// group's name, or an object id. The container grows to hold it,
        /// and no line is drawn between the two — the nesting says it.
        #[arg(long)]
        into: Option<String>,
        /// Position; inside a container, relative to it.
        #[arg(long)]
        x: Option<i32>,
        #[arg(long)]
        y: Option<i32>,
        #[arg(long)]
        width: Option<i32>,
        #[arg(long)]
        height: Option<i32>,
        /// Place the box only; do not draw its relationships.
        #[arg(long)]
        no_connect: bool,
        /// Draw a second box for a concept the view already shows.
        #[arg(long)]
        again: bool,
    },
    /// Put a Group — a titled box that holds other objects and stands for
    /// no concept — on a view.
    Group {
        view: String,
        name: String,
        /// Draw it inside this object, as `view add --into` does.
        #[arg(long)]
        into: Option<String>,
        #[arg(long)]
        x: Option<i32>,
        #[arg(long)]
        y: Option<i32>,
        #[arg(long)]
        width: Option<i32>,
        #[arg(long)]
        height: Option<i32>,
    },
    /// Put a Note — free text on the canvas — on a view, or with `--object`
    /// replace the text of one already there (`""` removes the text).
    Note {
        view: String,
        text: String,
        #[arg(long)]
        into: Option<String>,
        #[arg(long)]
        x: Option<i32>,
        #[arg(long)]
        y: Option<i32>,
        #[arg(long)]
        width: Option<i32>,
        #[arg(long)]
        height: Option<i32>,
        /// The id of a note already on the view whose text this replaces.
        #[arg(long)]
        object: Option<String>,
    },
    /// Change how a box, a group, a note or a line looks: what a person
    /// sets in Archi's properties. The target is an object id, a group's
    /// name, a concept on the view, a connection id, or `rel:<selector>`
    /// for every line drawing that relationship. A value of `""` clears a
    /// setting back to Archi's default.
    Style {
        view: String,
        target: String,
        /// `#rrggbb`.
        #[arg(long)]
        fill: Option<String>,
        /// `#rrggbb`, the border or the line.
        #[arg(long)]
        line: Option<String>,
        #[arg(long)]
        line_width: Option<String>,
        /// Points; a point is a pixel, as on a Mac.
        #[arg(long)]
        font_size: Option<String>,
        #[arg(long)]
        font_face: Option<String>,
        /// normal | bold | italic | bold-italic
        #[arg(long)]
        font_style: Option<String>,
        /// Archi's whole font string, when you have one.
        #[arg(long)]
        font: Option<String>,
        #[arg(long)]
        font_color: Option<String>,
        /// left | center | right
        #[arg(long)]
        text_align: Option<String>,
        /// top | center | bottom; on a line: source | middle | target
        #[arg(long)]
        text_position: Option<String>,
        /// On a group: tabbed | rectangle. On a note: dogear | rectangle | none.
        #[arg(long)]
        border: Option<String>,
        /// Fill opacity 0–255.
        #[arg(long)]
        alpha: Option<String>,
        #[arg(long)]
        line_alpha: Option<String>,
        /// What is written on it instead of its name; `${name}` and friends expand.
        #[arg(long)]
        label: Option<String>,
        /// show | hide, the type icon.
        #[arg(long)]
        icon: Option<String>,
        /// yes | no: whether an element's border is derived from its fill.
        /// Archi ignores --line on an element unless this is no.
        #[arg(long)]
        line_derived: Option<String>,
    },
    /// Route a line through points on the canvas, or straighten it.
    Route {
        view: String,
        /// A connection id, or `rel:<selector>` for every line drawing that relationship.
        target: String,
        /// Absolute canvas points, `x,y` separated by spaces; none straightens the line.
        #[arg(long, num_args = 0.., value_delimiter = ' ')]
        points: Vec<String>,
    },
    /// Draw one relationship between two objects already on the view — the
    /// one line a poster wants, where `view sync` would draw them all.
    Connect {
        view: String,
        /// The source object: an id, a group's name or a concept on the view.
        source: String,
        target: String,
        /// The relationship to draw; when omitted, the one relationship the
        /// model has between the two concepts.
        #[arg(long)]
        relationship: Option<String>,
    },
    /// Move an object already on the view inside another one, or with no
    /// `--into` back to the top. It keeps its place on the canvas, and a
    /// line between it and its new container is removed: the nesting
    /// stands for it, exactly as when a box is dragged into another in Archi.
    Nest {
        view: String,
        /// A concept on the view, a group's name, or an object id.
        target: String,
        #[arg(long)]
        into: Option<String>,
    },
    /// Draw every relationship the model holds between two members of the
    /// view that the view does not show yet. What the nesting already says
    /// is not drawn.
    Sync { view: String },
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
        ViewCmd::Add { view, selector, into, x, y, width, height, no_connect, again } => add(
            opts,
            m,
            view,
            selector,
            into.as_deref(),
            *x,
            *y,
            (*width, *height),
            !*no_connect,
            *again,
        ),
        ViewCmd::Group { view, name, into, x, y, width, height } => {
            group(opts, m, view, name, into.as_deref(), *x, *y, *width, *height)
        }
        ViewCmd::Note { view, text, into, x, y, width, height, object } => {
            note(opts, m, view, text, into.as_deref(), *x, *y, (*width, *height), object.as_deref())
        }
        ViewCmd::Style {
            view,
            target,
            fill,
            line,
            line_width,
            font_size,
            font_face,
            font_style,
            font,
            font_color,
            text_align,
            text_position,
            border,
            alpha,
            line_alpha,
            label,
            icon,
            line_derived,
        } => style(
            opts,
            m,
            view,
            target,
            StyleFlags {
                fill: fill.clone(),
                line: line.clone(),
                line_width: line_width.clone(),
                font_size: font_size.clone(),
                font_face: font_face.clone(),
                font_style: font_style.clone(),
                font: font.clone(),
                font_color: font_color.clone(),
                text_align: text_align.clone(),
                text_position: text_position.clone(),
                border: border.clone(),
                alpha: alpha.clone(),
                line_alpha: line_alpha.clone(),
                label: label.clone(),
                icon: icon.clone(),
                line_derived: line_derived.clone(),
            },
        ),
        ViewCmd::Route { view, target, points } => route(opts, m, view, target, points),
        ViewCmd::Connect { view, source, target, relationship } => {
            connect(opts, m, view, source, target, relationship.as_deref())
        }
        ViewCmd::Nest { view, target, into } => nest(opts, m, view, target, into.as_deref()),
        ViewCmd::Sync { view } => sync(opts, m, view),
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

/// What `--replace` took away, and what of it the replacement keeps.
///
/// A replace is "draw this view again", not "forget everything about it":
/// the objects and connections are rebuilt, but the viewpoint, the
/// documentation and the properties are what the view *says* rather than
/// what it draws, and a batch that names none of them means to leave them.
/// `export views` round-tripped through a replace used to delete every
/// view's documentation, and said nothing.
#[derive(Default)]
struct Replaced {
    ids: Vec<String>,
    slot: Option<Slot>,
    viewpoint: String,
    documentation: Option<String>,
    properties: Vec<(String, String)>,
}

fn claim_name(
    m: &mut Model,
    name: &str,
    except: Option<ViewId>,
    replace: bool,
) -> Result<Replaced, CliError> {
    let clash: Vec<ViewId> = m
        .views_with_ids()
        .filter(|(i, v)| v.name == name && Some(*i) != except)
        .map(|(i, _)| i)
        .collect();
    if clash.is_empty() {
        return Ok(Replaced::default());
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

    // The first clash is the view the caller is regenerating: its position,
    // and what it said about itself, are read before the delete.
    let first = clash[0];
    let kept = Replaced {
        ids: Vec::new(),
        slot: m.view_position(first),
        viewpoint: m.view(first).viewpoint.clone(),
        documentation: m.documentation(m.view(first).node).filter(|d| !d.is_empty()),
        properties: m.properties(m.view(first).node),
    };

    let mut ids = Vec::new();
    for v in clash {
        let id = m.view(v).id.clone();
        m.delete_view(v).map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
        ids.push(id);
    }
    Ok(Replaced { ids, ..kept })
}

/// Put a freshly made view back where the one it replaced was, saying what
/// it said.
///
/// The viewpoint is the caller's when given — `""` clears it — and the
/// replaced view's otherwise. Documentation and properties are copied whole;
/// a `view.doc` line later in the batch overwrites them as it always did.
fn reseat(m: &mut Model, v: ViewId, kept: &Replaced) -> Result<(), CliError> {
    let invalid =
        |e: amcli_model::EditError| CliError::new(Code::Invalid, "invalid", e.to_string());
    if let Some((folder, at)) = kept.slot {
        m.place_view_at(v, folder, at);
    }
    if let Some(doc) = &kept.documentation {
        m.set_view_documentation(v, doc).map_err(invalid)?;
    }
    for (k, val) in &kept.properties {
        m.set_view_property(v, k, val).map_err(invalid)?;
    }
    Ok(())
}

/// The viewpoint a created view gets: the one asked for, else the one the
/// view it replaces had. An explicit empty string is "none", not "keep".
fn viewpoint_for<'a>(asked: Option<&'a str>, kept: &'a Replaced) -> Option<&'a str> {
    match asked {
        Some(v) => Some(v).filter(|v| !v.is_empty()),
        None => Some(kept.viewpoint.as_str()).filter(|v| !v.is_empty()),
    }
}

fn find_view(m: &Model, sel: &str) -> Result<ViewId, CliError> {
    // `id:` takes the same three spellings it does for a concept.
    if let Some(id) = sel.strip_prefix("id:") {
        let found = amcli_graph::select::by_id_or_prefix(
            m.views_with_ids().map(|(i, v)| (i, v.id.as_str())),
            id,
        );
        if let [one] = found.as_slice() {
            return Ok(*one);
        }
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
    let kept = claim_name(m, name, None, replace)?;
    let v = m
        .add_view(name, viewpoint_for(vp, &kept))
        .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
    if let Some(f) = dest {
        m.move_view_to_folder(v, f)
            .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
    }
    reseat(m, v, &kept)?;
    let row = Row::new()
        .s("id", m.view(v).id.clone())
        .s("name", name.to_string())
        .s("folder", m.folder(m.view(v).folder).path.clone())
        .n("replaced", kept.ids.len() as i64)
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
    let out = Output::one(row).wrote(!opts.dry_run);
    Ok(if opts.dry_run { out.note("dry run: nothing was written") } else { out })
}

fn resolve(m: &Model, sel: &str) -> Result<ConceptId, CliError> {
    crate::read::resolve(&Graph::build(m), sel)
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
    let mut out: Vec<(ConceptId, String, String)> = Vec::new();
    let mut seen: std::collections::HashSet<ConceptId> = Default::default();
    for c in objects {
        for arc in g.neighbors(*c, Dir::Both, &EdgeFilter::default()) {
            if !seen.insert(arc.rel) {
                continue;
            }
            // Which object is the source is the relationship's business, not
            // the traversal's; the model answers with the ends in its order,
            // or with nothing when the line is already there.
            if let Some((src, tgt)) = m.undrawn_connection(v, arc.rel) {
                out.push((arc.rel, src, tgt));
            }
        }
    }
    out
}

/// The room a container leaves above its children: a group's tab and its
/// name, or an element's name across the top of its box.
fn container_header(figure: Figure) -> i32 {
    match figure {
        Figure::Tabbed => amcli_view::geometry::GROUP_HEADER + 30,
        _ => 40,
    }
}

/// The margin inside a container, and between a container's edge and what
/// it holds.
const CONTAINER_PAD: i32 = amcli_view::layout::GRID;

/// An object on the view named by the caller: an object id, a group's name,
/// or a concept that is drawn there.
fn find_object(m: &Model, scene: &amcli_view::Scene, sel: &str) -> Result<String, CliError> {
    let bare = sel.strip_prefix("id:").unwrap_or(sel);
    if let Some(n) = scene.nodes.iter().find(|n| n.id == bare) {
        return Ok(n.id.clone());
    }
    if let Some(n) = scene
        .nodes
        .iter()
        .find(|n| matches!(n.figure, Figure::Tabbed) && n.concept_id.is_none() && n.label == sel)
    {
        return Ok(n.id.clone());
    }
    let c = resolve(m, sel)?;
    let concept_id = m.concept(c).id.as_str();
    scene
        .nodes
        .iter()
        .find(|n| n.concept_id.as_deref() == Some(concept_id))
        .map(|n| n.id.clone())
        .ok_or_else(|| {
            CliError::new(
                Code::NotFound,
                "not_found",
                format!("`{}` is not on view `{}`", m.concept(c).name, scene.view_name),
            )
            .hint("`amcli view add` it first, or name a group or an object id")
        })
}

/// Where a new box of `w`×`h` goes: clear of everything at the top of the
/// view, or, inside a container, clear of the container's other children
/// and below its header, in the container's own coordinates. A container
/// too small for the new box is widened or deepened to hold it, and the
/// new size comes back with the slot.
fn slot_for_new(
    scene: &amcli_view::Scene,
    parent: Option<&str>,
    w: i32,
    h: i32,
    at: (Option<i32>, Option<i32>),
) -> (Rect, Option<(String, i32, i32)>) {
    let Some(p) = parent else {
        let taken: Vec<Rect> =
            scene.nodes.iter().filter(|n| n.parent_id.is_none()).map(|n| n.abs).collect();
        return (
            match at {
                (Some(x), Some(y)) => Rect { x, y, w, h },
                _ => free_slot(&taken, w, h),
            },
            None,
        );
    };
    let container = scene.nodes.iter().find(|n| n.id == p).expect("resolved above");
    let (ox, oy) = (CONTAINER_PAD, container_header(container.figure));
    // Siblings, relative to the container's content origin.
    let taken: Vec<Rect> = scene
        .nodes
        .iter()
        .filter(|n| n.parent_id.as_deref() == Some(p))
        .map(|n| Rect {
            x: n.abs.x - container.abs.x - ox,
            y: n.abs.y - container.abs.y - oy,
            w: n.abs.w,
            h: n.abs.h,
        })
        .collect();
    let slot = match at {
        (Some(x), Some(y)) => Rect { x, y, w, h },
        _ => {
            let s = free_slot(&taken, w, h);
            Rect { x: s.x + ox, y: s.y + oy, w, h }
        }
    };
    let need_w = slot.x + slot.w + CONTAINER_PAD;
    let need_h = slot.y + slot.h + CONTAINER_PAD;
    let grown = (need_w > container.abs.w || need_h > container.abs.h)
        .then(|| (p.to_string(), need_w.max(container.abs.w), need_h.max(container.abs.h)));
    (slot, grown)
}

/// Widen or deepen a container so that what was just put in it fits.
fn grow(m: &mut Model, v: ViewId, grown: &Option<(String, i32, i32)>) -> Result<(), CliError> {
    let Some((id, w, h)) = grown else { return Ok(()) };
    let scene = amcli_view::compile(m, v);
    let n = scene.nodes.iter().find(|n| n.id == *id).expect("the container is on the view");
    let (px, py) = n
        .parent_id
        .as_deref()
        .and_then(|p| scene.nodes.iter().find(|q| q.id == p))
        .map(|q| (q.abs.x, q.abs.y))
        .unwrap_or((0, 0));
    m.set_view_object_rect(v, id, n.abs.x - px, n.abs.y - py, Some((*w, *h)))
        .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))
}

#[allow(clippy::too_many_arguments)] // one parameter per CLI flag
fn add(
    opts: &Opts,
    m: &mut Model,
    view: &str,
    sel: &str,
    into: Option<&str>,
    x: Option<i32>,
    y: Option<i32>,
    size: (Option<i32>, Option<i32>),
    connect: bool,
    again: bool,
) -> Result<Output, CliError> {
    let v = find_view(m, view)?;
    let c = resolve(m, sel)?;
    let note = viewpoint_note(m, v, c);

    let scene = amcli_view::compile(m, v);
    let parent = into.map(|p| find_object(m, &scene, p)).transpose()?;
    // A concept already on the view stays as it is: a second box for the
    // same element is what a re-run "refresh" used to leave behind, and
    // nothing flagged it. The relationships it can draw are still drawn, so
    // adding a present member is how a view catches up with the model.
    // `--again` is the deliberate second box a poster sometimes wants.
    let present = scene
        .nodes
        .iter()
        .find(|n| n.concept_id.as_deref() == Some(m.concept(c).id.as_str()))
        .filter(|_| !again)
        .map(|n| (n.id.clone(), n.abs));
    let (id, slot, added) = match present {
        Some((id, rect)) => (id, rect, false),
        None => {
            let (dw, dh) = match &m.concept(c).kind {
                ConceptKind::Element(e) => e.info().default_wh,
                _ => (120, 55),
            };
            let (w, h) = (size.0.unwrap_or(dw), size.1.unwrap_or(dh));
            // Placed clear of everything already there, so adding one object
            // never disturbs the rest of the diagram.
            let (slot, grown) = slot_for_new(&scene, parent.as_deref(), w, h, (x, y));
            grow(m, v, &grown)?;
            let id = m
                .add_view_object_in(v, c, parent.as_deref(), slot.x, slot.y, slot.w, slot.h)
                .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
            (id, slot, true)
        }
    };

    // Gather, then mutate: the graph borrows the model.
    let wire = if connect { induced_connections(m, v, &[c]) } else { Vec::new() };
    let mut drawn = 0;
    for (rel, src, tgt) in wire {
        if m.add_view_connection(v, rel, &src, &tgt, &[]).is_ok() {
            drawn += 1;
        }
    }

    // `into` is appended, never inserted: a column in the middle repoints
    // every `cut -f5` already written against this row.
    let row = Row::new()
        .s("object", id)
        .s("concept", m.concept(c).id.clone())
        .n("x", slot.x as i64)
        .n("y", slot.y as i64)
        .n("connections", drawn)
        .b("added", added)
        .b("dry_run", opts.dry_run)
        .s("into", parent.clone().unwrap_or_default());
    let out = finish(opts, m, row)?;
    let out = match note {
        Some(n) => out.note(n),
        None => out,
    };
    let out = if added {
        out
    } else {
        out.note(format!("`{}` is already on the view; nothing was added", m.concept(c).name))
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

#[allow(clippy::too_many_arguments)] // one parameter per CLI flag
fn group(
    opts: &Opts,
    m: &mut Model,
    view: &str,
    name: &str,
    into: Option<&str>,
    x: Option<i32>,
    y: Option<i32>,
    width: Option<i32>,
    height: Option<i32>,
) -> Result<Output, CliError> {
    let v = find_view(m, view)?;
    let scene = amcli_view::compile(m, v);
    let parent = into.map(|p| find_object(m, &scene, p)).transpose()?;
    let (dw, dh) = fit_group_size(name);
    let (w, h) = (
        width.unwrap_or(dw.max(amcli_view::geometry::GROUP_SIZE.0)),
        height.unwrap_or(dh.max(amcli_view::geometry::GROUP_SIZE.1)),
    );
    let (slot, grown) = slot_for_new(&scene, parent.as_deref(), w, h, (x, y));
    grow(m, v, &grown)?;
    let id = m
        .add_view_group(v, name, parent.as_deref(), slot.x, slot.y, slot.w, slot.h)
        .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
    let row = Row::new()
        .s("object", id)
        .s("name", name.to_string())
        .s("into", parent.unwrap_or_default())
        .n("x", slot.x as i64)
        .n("y", slot.y as i64)
        .n("width", slot.w as i64)
        .n("height", slot.h as i64)
        .b("dry_run", opts.dry_run);
    finish(opts, m, row)
}

#[allow(clippy::too_many_arguments)] // one parameter per CLI flag
fn note(
    opts: &Opts,
    m: &mut Model,
    view: &str,
    text: &str,
    into: Option<&str>,
    x: Option<i32>,
    y: Option<i32>,
    size: (Option<i32>, Option<i32>),
    object: Option<&str>,
) -> Result<Output, CliError> {
    let v = find_view(m, view)?;
    let scene = amcli_view::compile(m, v);
    if let Some(obj) = object {
        let id = find_object(m, &scene, obj)?;
        let is_note = scene.nodes.iter().any(|n| n.id == id && matches!(n.figure, Figure::Note));
        if !is_note {
            return Err(CliError::new(Code::Invalid, "invalid", format!("`{obj}` is not a note")));
        }
        m.set_note_content(v, &id, text)
            .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
        let row = Row::new()
            .s("object", id)
            .n("chars", text.chars().count() as i64)
            .b("dry_run", opts.dry_run);
        return finish(opts, m, row);
    }
    let parent = into.map(|p| find_object(m, &scene, p)).transpose()?;
    let (dw, dh) = fit_note_size(text);
    let (w, h) = (size.0.unwrap_or(dw), size.1.unwrap_or(dh));
    let (slot, grown) = slot_for_new(&scene, parent.as_deref(), w, h, (x, y));
    grow(m, v, &grown)?;
    let id = m
        .add_view_note(v, text, parent.as_deref(), slot.x, slot.y, slot.w, slot.h)
        .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
    let row = Row::new()
        .s("object", id)
        .s("into", parent.unwrap_or_default())
        .n("x", slot.x as i64)
        .n("y", slot.y as i64)
        .n("chars", text.chars().count() as i64)
        .b("dry_run", opts.dry_run);
    finish(opts, m, row)
}

/// The style flags as the prompt and a batch give them: colours as
/// `#rrggbb`, words for alignment and position, and a font either whole or
/// as size, face and style. Empty strings clear.
#[derive(Clone, Debug, Default)]
pub struct StyleFlags {
    pub fill: Option<String>,
    pub line: Option<String>,
    pub line_width: Option<String>,
    pub font_size: Option<String>,
    pub font_face: Option<String>,
    pub font_style: Option<String>,
    pub font: Option<String>,
    pub font_color: Option<String>,
    pub text_align: Option<String>,
    pub text_position: Option<String>,
    pub border: Option<String>,
    pub alpha: Option<String>,
    pub line_alpha: Option<String>,
    pub label: Option<String>,
    pub icon: Option<String>,
    pub line_derived: Option<String>,
}

/// Every visual a style or route target names: one object, or every line
/// drawing a relationship.
fn find_visuals(
    m: &Model,
    v: ViewId,
    scene: &amcli_view::Scene,
    sel: &str,
) -> Result<Vec<String>, CliError> {
    if let Some(rel) = sel.strip_prefix("rel:") {
        let r = resolve(m, rel)?;
        let ids = m.view_connections_of(v, &m.concept(r).id);
        if ids.is_empty() {
            return Err(CliError::new(
                Code::NotFound,
                "not_found",
                format!("`{}` is not drawn on view `{}`", m.concept(r).name, scene.view_name),
            )
            .hint("`amcli view connect` draws it between two objects"));
        }
        return Ok(ids);
    }
    let bare = sel.strip_prefix("id:").unwrap_or(sel);
    if scene.edges.iter().any(|e| e.id == bare) {
        return Ok(vec![bare.to_string()]);
    }
    Ok(vec![find_object(m, scene, sel)?])
}

/// Turn the flags into Archi's encodings, reading the visual's current
/// font when only part of one is given.
fn style_change(
    m: &Model,
    v: ViewId,
    id: &str,
    is_line: bool,
    is_note: bool,
    f: &StyleFlags,
) -> Result<amcli_model::StyleChange, CliError> {
    let usage = |what: &str, got: &str, want: &str| {
        CliError::new(Code::Usage, "usage", format!("`{got}` is not a {what}"))
            .hint(format!("one of: {want}"))
    };
    let colour = |c: &Option<String>, what: &str| -> Result<Option<String>, CliError> {
        match c {
            None => Ok(None),
            Some(s) if s.is_empty() => Ok(Some(String::new())),
            Some(s) => {
                let ok = s.len() == 7
                    && s.starts_with('#')
                    && s[1..].chars().all(|ch| ch.is_ascii_hexdigit());
                if ok { Ok(Some(s.to_ascii_lowercase())) } else { Err(usage(what, s, "#rrggbb")) }
            }
        }
    };
    let word = |o: &Option<String>,
                what: &str,
                table: &[(&str, &str)]|
     -> Result<Option<String>, CliError> {
        match o {
            None => Ok(None),
            Some(s) if s.is_empty() => Ok(Some(String::new())),
            // Archi's own code is accepted too: it is what `export views` writes.
            Some(s) if table.iter().any(|(_, code)| *code == s.as_str()) => Ok(Some(s.clone())),
            Some(s) => table
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(s))
                .map(|(_, code)| Some(code.to_string()))
                .ok_or_else(|| {
                    usage(what, s, &table.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(" | "))
                }),
        }
    };
    // The font: given whole, or composed from the parts over what is there.
    let font = match &f.font {
        Some(whole) => Some(whole.clone()),
        None if f.font_size.is_some() || f.font_face.is_some() || f.font_style.is_some() => {
            let current = m.visual_style(v, id).ok().and_then(|s| s.font).unwrap_or_default();
            let mut parts: Vec<&str> = current.split('|').collect();
            let (face, size, style) = if parts.len() >= 6 {
                (parts[1].to_string(), parts[2].to_string(), parts[3].to_string())
            } else {
                ("Arial".to_string(), "12.0".to_string(), "0".to_string())
            };
            let _ = &mut parts;
            let face = f.font_face.clone().filter(|s| !s.is_empty()).unwrap_or(face);
            let size = match &f.font_size {
                Some(s) if !s.is_empty() => {
                    let n: f64 =
                        s.parse().map_err(|_| usage("font size", s, "a number of points"))?;
                    format!("{n:.1}")
                }
                _ => size,
            };
            let style = match f.font_style.as_deref() {
                Some("normal") => "0".to_string(),
                Some("bold") => "1".to_string(),
                Some("italic") => "2".to_string(),
                Some("bold-italic") => "3".to_string(),
                Some(other) if !other.is_empty() => {
                    return Err(usage("font style", other, "normal | bold | italic | bold-italic"));
                }
                _ => style,
            };
            Some(format!("1|{face}|{size}|{style}|COCOA|1|"))
        }
        None => None,
    };
    let border_table: &[(&str, &str)] = if is_note {
        &[("dogear", "0"), ("rectangle", "1"), ("none", "2")]
    } else {
        &[("tabbed", "0"), ("rectangle", "1")]
    };
    let position_table: &[(&str, &str)] = if is_line {
        &[("source", "0"), ("middle", "1"), ("target", "2")]
    } else {
        &[("top", "0"), ("center", "1"), ("centre", "1"), ("bottom", "2")]
    };
    Ok(amcli_model::StyleChange {
        fill: colour(&f.fill, "colour")?,
        line: colour(&f.line, "colour")?,
        line_width: f.line_width.clone(),
        font,
        font_color: colour(&f.font_color, "colour")?,
        text_align: word(
            &f.text_align,
            "text alignment",
            &[("left", "1"), ("center", "2"), ("centre", "2"), ("right", "4")],
        )?,
        text_position: word(&f.text_position, "text position", position_table)?,
        border: word(&f.border, "border", border_table)?,
        alpha: f.alpha.clone(),
        line_alpha: f.line_alpha.clone(),
        label: f.label.clone(),
        icon: word(&f.icon, "icon setting", &[("show", "1"), ("hide", "2"), ("default", "")])?,
        // Archi's default is `true`, and it writes the feature only when
        // `false` — the one value that lets an explicit line colour show.
        line_derived: word(
            &f.line_derived,
            "line derivation",
            &[("yes", "true"), ("no", "false")],
        )?,
    })
}

pub fn style(
    opts: &Opts,
    m: &mut Model,
    view: &str,
    target: &str,
    flags: StyleFlags,
) -> Result<Output, CliError> {
    let v = find_view(m, view)?;
    let scene = amcli_view::compile(m, v);
    let ids = find_visuals(m, v, &scene, target)?;
    let mut rows = Vec::new();
    for id in &ids {
        let is_line = scene.edges.iter().any(|e| e.id == *id);
        let is_note = scene.nodes.iter().any(|n| n.id == *id && matches!(n.figure, Figure::Note));
        let change = style_change(m, v, id, is_line, is_note, &flags)?;
        let touched = m
            .style_visual(v, id, &change)
            .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
        rows.push(
            Row::new()
                .s("object", id.clone())
                .s("kind", if is_line { "line" } else { "object" })
                .s("set", touched.join(","))
                .b("dry_run", opts.dry_run),
        );
    }
    if !opts.dry_run {
        crate::write::save(m)?;
    }
    let out = Output::rows(rows).wrote(!opts.dry_run);
    Ok(if opts.dry_run { out.note("dry run: nothing was written") } else { out })
}

pub fn route(
    opts: &Opts,
    m: &mut Model,
    view: &str,
    target: &str,
    points: &[String],
) -> Result<Output, CliError> {
    let v = find_view(m, view)?;
    let scene = amcli_view::compile(m, v);
    let ids = find_visuals(m, v, &scene, target)?;
    let pts: Vec<amcli_view::Pt> = points
        .iter()
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (x, y) = p
                .split_once(',')
                .and_then(|(a, b)| Some((a.trim().parse().ok()?, b.trim().parse().ok()?)))
                .ok_or_else(|| {
                    CliError::new(Code::Usage, "usage", format!("`{p}` is not a point"))
                        .hint("write each point as x,y")
                })?;
            Ok(amcli_view::Pt { x, y })
        })
        .collect::<Result<_, CliError>>()?;
    let mut rows = Vec::new();
    for id in &ids {
        let Some((_, src, tgt)) = m.view_connections(v).into_iter().find(|(c, _, _)| c == id)
        else {
            return Err(CliError::new(Code::Invalid, "invalid", format!("`{id}` is not a line")));
        };
        let rect = |o: &str| scene.nodes.iter().find(|n| n.id == o).map(|n| n.abs);
        let (Some(sb), Some(tb)) = (rect(&src), rect(&tgt)) else {
            return Err(CliError::new(
                Code::Invalid,
                "invalid",
                format!("`{id}` ends on something with no bounds"),
            ));
        };
        let bends: Vec<(i32, i32, i32, i32)> = pts
            .iter()
            .map(|p| {
                let b = amcli_view::geometry::bendpoint_for(sb, tb, *p);
                (b.start_x, b.start_y, b.end_x, b.end_y)
            })
            .collect();
        m.set_view_connection_bendpoints(v, id, &bends)
            .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
        rows.push(
            Row::new()
                .s("object", id.clone())
                .n("points", bends.len() as i64)
                .b("dry_run", opts.dry_run),
        );
    }
    if !opts.dry_run {
        crate::write::save(m)?;
    }
    let out = Output::rows(rows).wrote(!opts.dry_run);
    Ok(if opts.dry_run { out.note("dry run: nothing was written") } else { out })
}

pub fn connect(
    opts: &Opts,
    m: &mut Model,
    view: &str,
    source: &str,
    target: &str,
    relationship: Option<&str>,
) -> Result<Output, CliError> {
    let v = find_view(m, view)?;
    let scene = amcli_view::compile(m, v);
    let src = find_object(m, &scene, source)?;
    let tgt = find_object(m, &scene, target)?;
    let concept_of =
        |o: &str| scene.nodes.iter().find(|n| n.id == o).and_then(|n| n.concept_id.clone());
    let rel = match relationship {
        Some(sel) => resolve(m, sel)?,
        None => {
            let (Some(a), Some(b)) = (concept_of(&src), concept_of(&tgt)) else {
                return Err(CliError::new(
                    Code::Usage,
                    "usage",
                    "both ends must show a concept, or pass --relationship",
                ));
            };
            let between: Vec<ConceptId> = m
                .concepts_with_ids()
                .filter(|(_, c)| {
                    c.kind.is_relationship()
                        && ((c.source.as_deref() == Some(a.as_str())
                            && c.target.as_deref() == Some(b.as_str()))
                            || (c.source.as_deref() == Some(b.as_str())
                                && c.target.as_deref() == Some(a.as_str())))
                })
                .map(|(i, _)| i)
                .collect();
            match between.as_slice() {
                [one] => *one,
                [] => {
                    return Err(CliError::new(
                        Code::NotFound,
                        "not_found",
                        "the model has no relationship between the two",
                    )
                    .hint("`amcli relation add` first, with --no-draw, then connect"));
                }
                many => {
                    return Err(CliError::new(
                        Code::Ambiguous,
                        "ambiguous",
                        format!("{} relationships between the two", many.len()),
                    )
                    .hint("pass --relationship id:…")
                    .rows(
                        many.iter()
                            .map(|r| {
                                Row::new()
                                    .s("selector", format!("id:{}", m.concept(*r).id))
                                    .s("type", m.concept(*r).kind.name())
                            })
                            .collect(),
                    ));
                }
            }
        }
    };
    // The line runs the relationship's way whichever way the objects were named.
    let r = m.concept(rel);
    let (from, to) = if r.source.as_deref() == concept_of(&tgt).as_deref()
        && r.target.as_deref() == concept_of(&src).as_deref()
    {
        (tgt.clone(), src.clone())
    } else {
        (src.clone(), tgt.clone())
    };
    let id = m
        .add_view_connection(v, rel, &from, &to, &[])
        .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
    let row = Row::new()
        .s("object", id)
        .s("relationship", m.concept(rel).id.clone())
        .s("source", from)
        .s("target", to)
        .b("dry_run", opts.dry_run);
    finish(opts, m, row)
}

fn nest(
    opts: &Opts,
    m: &mut Model,
    view: &str,
    target: &str,
    into: Option<&str>,
) -> Result<Output, CliError> {
    let v = find_view(m, view)?;
    let scene = amcli_view::compile(m, v);
    let object = find_object(m, &scene, target)?;
    let parent = into.map(|p| find_object(m, &scene, p)).transpose()?;
    let removed = m
        .nest_view_object(v, &object, parent.as_deref())
        .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
    // The container may be too small for what it now holds.
    let scene = amcli_view::compile(m, v);
    if let Some(p) = &parent
        && let (Some(c), Some(n)) =
            (scene.nodes.iter().find(|n| n.id == *p), scene.nodes.iter().find(|n| n.id == object))
    {
        let need_w = n.abs.x + n.abs.w - c.abs.x + CONTAINER_PAD;
        let need_h = n.abs.y + n.abs.h - c.abs.y + CONTAINER_PAD;
        if need_w > c.abs.w || need_h > c.abs.h {
            grow(m, v, &Some((p.clone(), need_w.max(c.abs.w), need_h.max(c.abs.h))))?;
        }
    }
    let row = Row::new()
        .s("object", object)
        .s("into", parent.unwrap_or_default())
        .n("connections_removed", removed.len() as i64)
        .b("dry_run", opts.dry_run);
    let out = finish(opts, m, row)?;
    Ok(if removed.is_empty() {
        out
    } else {
        out.note(format!(
            "{} line(s) between the object and its container were removed; the nesting now stands for them",
            removed.len()
        ))
    })
}

fn sync(opts: &Opts, m: &mut Model, view: &str) -> Result<Output, CliError> {
    let v = find_view(m, view)?;
    let scene = amcli_view::compile(m, v);
    let mut members: Vec<ConceptId> = Vec::new();
    for n in &scene.nodes {
        if let Some(c) = n.concept_id.as_deref().and_then(|c| m.concept_by_id(c))
            && !members.contains(&c)
        {
            members.push(c);
        }
    }
    let wire = induced_connections(m, v, &members);
    let mut drawn = 0;
    for (rel, src, tgt) in wire {
        if m.add_view_connection(v, rel, &src, &tgt, &[]).is_ok() {
            drawn += 1;
        }
    }
    let row = Row::new()
        .s("view", m.view(v).id.clone())
        .n("members", members.len() as i64)
        .n("connections", drawn)
        .b("dry_run", opts.dry_run);
    let out = finish(opts, m, row)?;
    Ok(if drawn > 0 {
        out.note(format!(
            "drew {drawn} relationship(s) the view did not show; \
             `amcli view layout {view} --relayout-all` will tidy the placement"
        ))
    } else {
        out.note("the view already shows every relationship between its members")
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
    let kept = claim_name(m, name, None, replace)?;

    // Gather first, mutate second: the graph borrows the model.
    let (items, edges, concepts, rels) = {
        let g = Graph::build(m);
        let root = crate::read::resolve(&g, from)?;
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
    let v = m
        .add_view(name, viewpoint_for(vp, &kept))
        .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
    if let Some(f) = dest {
        m.move_view_to_folder(v, f)
            .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
    }
    reseat(m, v, &kept)?;

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
        .n("replaced", kept.ids.len() as i64)
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

/// The size a box comes back at when it is relaid: fitted to its label,
/// unless it is a small figure — a junction is a circle whatever it is
/// called — or a container, whose size is what it holds.
fn relaid_size(n: &amcli_view::Node) -> (i32, i32) {
    if n.abs.w >= 60 && n.abs.h >= 30 {
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
    }
}

fn relayout(
    opts: &Opts,
    m: &mut Model,
    view: &str,
    algorithm: &str,
    all: bool,
) -> Result<Output, CliError> {
    use std::collections::HashMap;
    let v = find_view(m, view)?;
    let algo = parse_algorithm(algorithm)?;

    let scene = amcli_view::compile(m, v);
    let node_by_id: HashMap<&str, &amcli_view::Node> =
        scene.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    // Each object's position in its own container's coordinates, which is
    // what the file stores and what "never placed" is judged by.
    let rel = |n: &amcli_view::Node| -> Rect {
        let (px, py) = n
            .parent_id
            .as_deref()
            .and_then(|p| node_by_id.get(p))
            .map(|p| (p.abs.x, p.abs.y))
            .unwrap_or((0, 0));
        Rect { x: n.abs.x - px, y: n.abs.y - py, w: n.abs.w, h: n.abs.h }
    };
    // What each container holds, in document order. `None` is the view.
    let mut kids: HashMap<Option<String>, Vec<usize>> = HashMap::new();
    for (i, n) in scene.nodes.iter().enumerate() {
        kids.entry(n.parent_id.clone()).or_default().push(i);
    }

    // Only objects that have never been placed move, unless told otherwise.
    // Reflowing everything by default is how one added element turns into a
    // four-hundred-line diff. A container is relaid when any of its own
    // children has never been placed; the others stay as they are.
    let mut containers: Vec<Option<String>> = kids
        .iter()
        .filter(|(_, ids)| {
            all || ids.iter().any(|i| {
                let r = rel(&scene.nodes[*i]);
                r.x == 0 && r.y == 0
            })
        })
        .map(|(c, _)| c.clone())
        .collect();
    if containers.is_empty() {
        return Ok(Output::empty().note("nothing to move; pass --relayout-all to reflow the view"));
    }
    // Deepest first, so a container's size is known before its own
    // container is laid out around it.
    let depth_of = |c: &Option<String>| {
        c.as_deref().and_then(|id| node_by_id.get(id)).map(|n| n.depth + 1).unwrap_or(0)
    };
    containers.sort_by_key(|c| std::cmp::Reverse((depth_of(c), c.clone())));

    // The lines drawn on the view, each lifted to the pair of direct
    // children of a container that its ends sit under. A line from deep
    // inside one box to deep inside another is an edge between the two
    // boxes at the level where they are siblings.
    let connections = m.view_connections(v);
    let lift = |obj: &str, container: &Option<String>| -> Option<String> {
        let mut cur = obj.to_string();
        loop {
            let n = node_by_id.get(cur.as_str())?;
            if n.parent_id == *container {
                return Some(cur);
            }
            cur = n.parent_id.clone()?;
        }
    };

    let mut sizes: HashMap<String, (i32, i32)> = HashMap::new();
    let mut moved = 0;
    let mut edge_count = 0;
    let mut used = algo;
    let mut straighten: Vec<String> = Vec::new();
    for container in &containers {
        let ids = &kids[container];
        // The label has to come from the node being moved. Indexing the
        // scene by the *filtered* position read some other node's name,
        // which fed the wrong sort key into a layout that is otherwise
        // deterministic.
        let items: Vec<Item> = ids
            .iter()
            .map(|i| {
                let n = &scene.nodes[*i];
                let (w, h) = match sizes.get(n.id.as_str()) {
                    Some(s) => *s,
                    None if kids.contains_key(&Some(n.id.clone())) => (n.abs.w, n.abs.h),
                    None => relaid_size(n),
                };
                Item { id: n.id.clone(), name: n.label.clone(), w, h }
            })
            .collect();
        let index: HashMap<&str, usize> =
            items.iter().enumerate().map(|(i, it)| (it.id.as_str(), i)).collect();
        // Without the edges every layered relayout saw an edgeless graph,
        // ranked everything at zero, and produced one enormous row.
        let mut edges: Vec<(usize, usize)> = Vec::new();
        for (id, src, tgt) in &connections {
            let (Some(a), Some(b)) = (lift(src, container), lift(tgt, container)) else { continue };
            let (Some(&a), Some(&b)) = (index.get(a.as_str()), index.get(b.as_str())) else {
                continue;
            };
            if a != b && !edges.contains(&(a, b)) && !edges.contains(&(b, a)) {
                edges.push((a, b));
            }
            straighten.push(id.clone());
        }
        let placed = place(&items, &edges, algo);
        if container.is_none() {
            used = placed.algorithm;
        }
        // Inside a container the children sit below its header and inside
        // its margin; at the top of the view the layout's origin is the
        // canvas's.
        let (ox, oy) = match container.as_deref().and_then(|c| node_by_id.get(c)) {
            Some(c) => (CONTAINER_PAD, container_header(c.figure)),
            None => (0, 0),
        };
        let min_x = placed.rects.iter().map(|r| r.x).min().unwrap_or(0);
        let min_y = placed.rects.iter().map(|r| r.y).min().unwrap_or(0);
        let (mut far_x, mut far_y) = (0, 0);
        for (item, r) in items.iter().zip(placed.rects.iter()) {
            let (x, y) = (r.x - min_x + ox, r.y - min_y + oy);
            m.set_view_object_rect(v, &item.id, x, y, Some((r.w, r.h)))
                .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
            far_x = far_x.max(x + r.w);
            far_y = far_y.max(y + r.h);
        }
        moved += items.len();
        edge_count += edges.len();
        // The container is sized to what it now holds: remembered for the
        // layout of its own container, and written now in case that one is
        // not being relaid.
        if let Some(c) = container {
            let size = (far_x + CONTAINER_PAD, far_y + CONTAINER_PAD);
            sizes.insert(c.clone(), size);
            let r = rel(node_by_id[c.as_str()]);
            m.set_view_object_rect(v, c, r.x, r.y, Some(size))
                .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
        }
    }
    // And every connection among what moved is straightened. Moving the
    // boxes and leaving old bendpoints where they were drew each such line
    // through whatever now sat on its former path; a relaid view has straight
    // lines, and the layout is what keeps them off the boxes.
    straighten.sort();
    straighten.dedup();
    for conn_id in &straighten {
        m.set_view_connection_bendpoints(v, conn_id, &[])
            .map_err(|e| CliError::new(Code::Invalid, "invalid", e.to_string()))?;
    }

    let row = Row::new()
        .s("view", m.view(v).id.clone())
        .n("moved", moved as i64)
        .n("edges", edge_count as i64)
        .n("containers", containers.iter().filter(|c| c.is_some()).count() as i64)
        .s("algorithm", used.as_str())
        .b("dry_run", opts.dry_run);
    let out = finish(opts, m, row)?;
    Ok(fallback_note(out, algo, used))
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
