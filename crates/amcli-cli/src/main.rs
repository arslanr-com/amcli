//! `amcli` — a CLI over ArchiMate models.

use std::io::Write;
use std::path::{Path, PathBuf};

use amcli_graph::Graph;
use amcli_model::Model;
use clap::{Parser, Subcommand};

mod apply;
mod export;
mod init;
mod output;
mod read;
mod skill;
mod view;
mod web;
mod write;

use output::{CliError, Code, Format, Output, Printer};

/// The version, plus enough to tell two builds of it apart. See `build.rs`.
const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (", env!("AMCLI_BUILD"), ")");

#[derive(Parser)]
#[command(
    name = "amcli",
    version = VERSION,
    about = "A CLI over ArchiMate models. No Archi, no JVM, no daemon.",
    after_help = "\
Reads are bare verbs; anything that changes the model is noun-verb, which makes
a write one word longer than a read on purpose.

Flags mean one thing everywhere:
  -t concept type   -r relationship type   -f folder      -D direction
  -n depth          -l limit               -m model       -F format

Subjects are positional, never flags:
  amcli element  add ApplicationComponent \"Refund Service\"
  amcli relation add Serving \"Refund Service\" \"Checkout Service\"

Exit codes, so you can branch without parsing prose:
  0 ok   2 usage   3 not found   4 ambiguous   5 invalid   6 conflict
  7 io   8 unsupported

ArchiMate(R) is a registered trademark of The Open Group. This project is
independent and is not affiliated with or endorsed by them or by Archi."
)]
pub struct Cli {
    /// Model file. Defaults to $AMCLI_MODEL, else the nearest *.archimate
    /// walking up from the working directory.
    #[arg(short = 'm', long, global = true)]
    model: Option<PathBuf>,

    /// Output format: text (tab-separated records), json, jsonl.
    #[arg(short = 'F', long, global = true, default_value = "text")]
    format: String,

    /// Drop the column headers and the notes from stderr. Stdout is unchanged,
    /// JSON envelope included.
    #[arg(short = 'q', long, global = true)]
    quiet: bool,

    /// Keep only these fields, or drop them with a leading `-`.
    #[arg(long, global = true, value_delimiter = ',', allow_hyphen_values = true)]
    fields: Option<Vec<String>>,

    /// Print how many results there would be, and nothing else. Never writes.
    #[arg(long, global = true)]
    count: bool,

    /// Maximum records to return. 0 means no limit.
    #[arg(short = 'l', long, global = true, default_value_t = 50)]
    limit: usize,

    /// Report what a write would do and change nothing.
    #[arg(long, global = true)]
    dry_run: bool,

    /// Refuse the write if the file has changed since this checksum was read.
    #[arg(long, global = true)]
    expect_checksum: Option<String>,

    /// Skip the confirmation on a cascading delete.
    #[arg(short = 'y', long, global = true)]
    yes: bool,

    /// Derive new ids from what they name instead of at random, so that
    /// rebuilding a model from the same batches produces the same file.
    /// Also read from $AMCLI_ID_SEED.
    #[arg(long, global = true, value_name = "SEED")]
    id_seed: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// A concept with its inbound and outbound relationships.
    Get {
        /// id:… , a name, Type:Name, or a glob.
        selector: String,
        /// Show documentation in full instead of truncating it.
        #[arg(long)]
        full: bool,
    },
    /// Substring search over names, then documentation, then property values.
    Search {
        query: String,
        #[arg(short = 't', long)]
        r#type: Option<String>,
    },
    /// Enumerate concepts.
    List {
        #[arg(short = 't', long)]
        r#type: Option<String>,
        /// Folder path prefix, e.g. /Application.
        #[arg(short = 'f', long)]
        folder: Option<String>,
    },
    /// Filter expression, e.g. 'type=ApplicationComponent and name~pay'.
    Query { expr: String },
    /// One hop out from a concept.
    Neighbors {
        selector: String,
        #[arg(short = 'D', long, default_value = "both")]
        direction: String,
        #[arg(short = 'r', long)]
        rel: Option<String>,
        /// Keep only concepts of this type.
        #[arg(short = 't', long)]
        r#type: Option<String>,
    },
    /// The neighbourhood within N hops, as an induced subgraph.
    Trace {
        selector: String,
        #[arg(short = 'D', long, default_value = "both")]
        direction: String,
        #[arg(short = 'n', long, default_value_t = 2)]
        depth: u32,
        #[arg(short = 'r', long)]
        rel: Option<String>,
        /// Show only concepts of this type. The walk still crosses every type,
        /// so a multi-hop query stays useful.
        #[arg(short = 't', long)]
        r#type: Option<String>,
    },
    /// How two concepts are connected.
    Path {
        from: String,
        to: String,
        #[arg(short = 'D', long, default_value = "out")]
        direction: String,
        /// Every simple path, not just the shortest.
        #[arg(long)]
        all: bool,
        #[arg(short = 'n', long, default_value_t = 6)]
        depth: u32,
    },
    /// What is reachable, and the relationship that pulled each thing in.
    Impact {
        selector: String,
        #[arg(short = 'D', long, default_value = "in")]
        direction: String,
        #[arg(short = 'n', long)]
        depth: Option<u32>,
        /// Report only concepts of this type. The walk still crosses every
        /// type, so asking for components two hops away still finds them.
        #[arg(short = 't', long)]
        r#type: Option<String>,
    },
    /// Composition and aggregation upwards.
    Ancestors { selector: String },
    /// Composition and aggregation downwards.
    Descendants { selector: String },
    /// Dependency cycles.
    Cycles {
        #[arg(short = 'r', long)]
        rel: Option<String>,
    },
    /// Counts by type, layer and folder, plus orphans.
    Stats,
    /// Views: list, create, populate, lay out and draw.
    #[command(subcommand)]
    View(view::ViewCmd),
    /// Check the model.
    Validate {
        /// How far to check. Each level includes the ones before it:
        /// types, rules, integrity, all.
        #[arg(long, default_value = "all")]
        level: String,
        /// Apply the repairs that are derived rather than chosen.
        #[arg(long)]
        fix: bool,
        /// Treat warnings as failure.
        #[arg(long)]
        strict: bool,
    },
    /// Create, change and delete elements.
    #[command(subcommand)]
    Element(write::ElementCmd),
    /// Create, change and delete relationships.
    #[command(subcommand)]
    Relation(write::RelationCmd),
    /// Folders.
    #[command(subcommand)]
    Folder(write::FolderCmd),
    /// Properties on a concept.
    #[command(subcommand)]
    Prop(write::PropCmd),
    /// Apply a batch of edits atomically: all of them land, or none do.
    Apply {
        /// A JSONL file, or `-` for stdin.
        #[arg(default_value = "-")]
        file: String,
    },
    /// Export the whole model.
    Export {
        /// csv | json | mermaid | dot | views. Named `to` rather than `format`
        /// because clap merges a same-named subcommand field into the global
        /// -F, which controls how amcli reports rather than what it writes.
        to: String,
        #[arg(short = 'o', long)]
        out: Option<String>,
    },
    /// Browse the model in a local web page: views, elements, relationships
    /// and a graph. Read-only; the page follows the file as it changes.
    Web {
        /// Port to listen on. Defaults to a free one chosen by the OS.
        /// Also read from $AMCLI_WEB_PORT.
        #[arg(long, env = "AMCLI_WEB_PORT")]
        port: Option<u16>,
        /// Interface to listen on. Loopback by default; a container that has
        /// to be reached from outside needs 0.0.0.0. Also read from
        /// $AMCLI_WEB_BIND.
        #[arg(long, env = "AMCLI_WEB_BIND", default_value = "127.0.0.1", value_name = "ADDR")]
        bind: String,
        /// Host headers to serve besides localhost, comma-separated — the
        /// name a reverse proxy puts in front of the viewer. Without it a
        /// request for any other name is refused, which is what keeps a page
        /// on another origin from reading the model. Also read from
        /// $AMCLI_WEB_ALLOW_HOST.
        #[arg(
            long,
            env = "AMCLI_WEB_ALLOW_HOST",
            value_delimiter = ',',
            value_name = "HOST",
            num_args = 1..
        )]
        allow_host: Vec<String>,
        /// Print the URL and serve; do not open a browser.
        #[arg(long)]
        no_open: bool,
    },
    /// Install the agent skill.
    #[command(subcommand)]
    Skill(skill::SkillCmd),
    /// Model-level facts.
    Info,
    /// Create an empty model with the folders Archi expects.
    Init {
        /// The model's name.
        name: String,
        /// Where to write it. Defaults to a slug of the name.
        #[arg(short = 'o', long)]
        out: Option<PathBuf>,
        /// Overwrite an existing file.
        #[arg(long)]
        force: bool,
    },
}

/// Parse, but turn "no such subcommand" into a version-skew hint.
///
/// The skill is installed from the default branch by `npx skills add` while
/// the binary comes from the newest release, so the skill can legitimately be
/// *newer* than the binary and document a command it does not have. clap
/// answers that with "unrecognized subcommand" and exits before any of our
/// code runs, which reads like a broken skill rather than an old binary.
///
/// This is deliberately the only place the two versions are reconciled: a
/// `metadata:` field would be inert (the skills CLI reads only `name` and
/// `description`), and asking an agent to compare version strings by eye is
/// the kind of step that fails quietly.
///
/// An unknown *flag* is emphatically not the same thing, and used to get the
/// same footer. It sent a reader off to reinstall a current binary over what was
/// a misremembered flag name, so that case gets the one thing that actually
/// helps: the flags the command does have.
fn parse_or_hint() -> Cli {
    use clap::error::ErrorKind;
    match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            let kind = e.kind();
            let _ = e.print();
            match kind {
                ErrorKind::InvalidSubcommand => eprintln!(
                    "\nThis amcli is {VERSION}. If a skill or documentation said that \
                     subcommand exists,\nthis binary is older than that document. \
                     Upgrade it:\n  sh ~/.agents/skills/amcli/scripts/install.sh"
                ),
                ErrorKind::UnknownArgument => {
                    if let Some(flags) = flags_for(std::env::args().skip(1)) {
                        eprintln!("\n{flags}");
                    }
                }
                _ => {}
            }
            std::process::exit(if e.use_stderr() { Code::Usage as i32 } else { 0 });
        }
    }
}

/// The flags accepted by the deepest subcommand named in an argument list.
///
/// Read out of clap's own definitions, so it cannot list a flag that does not
/// exist or miss one that does.
///
/// A token that is not a subcommand is skipped rather than ending the walk: the
/// arguments still contain flag values and positionals (`-m model.archimate view
/// layout …`), and stopping at the first one of those reported only the global
/// flags — which is not the question being answered.
fn flags_for(args: impl Iterator<Item = String>) -> Option<String> {
    use clap::CommandFactory;
    let mut cmd = Cli::command();
    let mut path = vec!["amcli".to_string()];
    for arg in args {
        if arg.starts_with('-') {
            continue;
        }
        let Some(next) = cmd.find_subcommand(&arg).cloned() else { continue };
        path.push(next.get_name().to_string());
        cmd = next;
    }

    let mut names: Vec<String> = cmd
        .get_arguments()
        .filter_map(|a| a.get_long())
        .map(|l| format!("--{l}"))
        .chain(
            Cli::command().get_arguments().filter_map(|a| a.get_long()).map(|l| format!("--{l}")),
        )
        .collect();
    names.sort();
    names.dedup();
    (!names.is_empty()).then(|| {
        format!("Flags of `{}`, and the global ones:\n  {}", path.join(" "), names.join(" "))
    })
}

fn main() {
    let cli = parse_or_hint();
    let Some(format) = Format::parse(&cli.format) else {
        eprintln!("error: unknown format `{}`; expected text, json or jsonl", cli.format);
        std::process::exit(Code::Usage as i32);
    };
    let printer =
        Printer { format, quiet: cli.quiet, fields: cli.fields.clone(), count_only: cli.count };

    let mut stdout = std::io::stdout().lock();
    let mut stderr = std::io::stderr().lock();

    match run(&cli) {
        Ok(mut out) => {
            let verdict = out.exit;
            let then = out.then.take();
            printer.print(out, &mut stdout, &mut stderr);
            let _ = stdout.flush();
            if let Some(code) = verdict {
                std::process::exit(code as i32);
            }
            if let Some(then) = then {
                // The locks are held for the life of `main`; a continuation
                // that spawns threads must not inherit a terminal nobody else
                // can write to.
                drop(stdout);
                drop(stderr);
                then();
            }
        }
        Err(e) => {
            let code = e.code;
            printer.print_error(&e, &mut stdout, &mut stderr);
            let _ = stdout.flush();
            std::process::exit(code as i32);
        }
    }
}

fn run(cli: &Cli) -> Result<Output, CliError> {
    // Set before any edit, and process-wide: every id minted this run has to
    // come from the same decision.
    amcli_model::ids::set_seed(cli.id_seed.clone().or_else(|| std::env::var("AMCLI_ID_SEED").ok()));

    // Neither of these is about a model that already exists, so both run before
    // amcli goes looking for one.
    match &cli.command {
        Command::Skill(c) => return skill::run(c),
        Command::Init { name, out, force } => {
            return init::run(&write_opts(cli), name, out.as_deref(), *force);
        }
        _ => {}
    }

    let path = find_model(cli.model.as_deref())?;
    let mut model = Model::open(&path).map_err(|e| {
        CliError::new(Code::Io, "io", e.to_string())
            .hint("check the path, or pass -m to point at the model")
    })?;

    let ctx = read::Ctx { limit: cli.limit, path: path.clone() };

    // Reads borrow the model; writes need it mutably, so they are dispatched
    // separately rather than threading a borrow through both.
    match &cli.command {
        Command::Element(_) | Command::Relation(_) | Command::Folder(_) | Command::Prop(_) => {
            return write::run(cli_write_opts(cli), &mut model, &cli.command_write());
        }
        Command::View(c) => {
            let out = view::run(&write_opts(cli), &mut model, c)?;
            return Ok(read::carry(out, &model, cli.fields.as_ref()));
        }
        Command::Apply { file } => {
            return apply::run(&write_opts(cli), &mut model, Some(file.as_str()));
        }
        Command::Validate { level, fix, strict } => {
            return read::validate(&mut model, level, *fix, *strict, &write_opts(cli));
        }
        Command::Web { port, bind, allow_host, no_open } => {
            return web::run(model, path, *port, bind, allow_host, *no_open);
        }
        _ => {}
    }

    let graph = Graph::build(&model);
    // One place where a projection can reach back into the model for what the
    // record did not print: every read answers through here.
    let out = match &cli.command {
        Command::Get { selector, full } => read::get(&graph, &ctx, selector, *full),
        Command::Search { query, r#type } => read::search(&graph, &ctx, query, r#type.as_deref()),
        Command::List { r#type, folder } => {
            read::list(&graph, &ctx, r#type.as_deref(), folder.as_deref())
        }
        Command::Query { expr } => read::query(&graph, &ctx, expr),
        Command::Neighbors { selector, direction, rel, r#type } => {
            read::neighbors(&graph, &ctx, selector, direction, rel.as_deref(), r#type.as_deref())
        }
        Command::Trace { selector, direction, depth, rel, r#type } => read::trace(
            &graph,
            &ctx,
            selector,
            direction,
            *depth,
            rel.as_deref(),
            r#type.as_deref(),
        ),
        Command::Path { from, to, direction, all, depth } => {
            read::path(&graph, &ctx, from, to, direction, *all, *depth)
        }
        Command::Impact { selector, direction, depth, r#type } => {
            read::impact(&graph, &ctx, selector, direction, *depth, r#type.as_deref())
        }
        Command::Ancestors { selector } => read::containment(&graph, &ctx, selector, true),
        Command::Descendants { selector } => read::containment(&graph, &ctx, selector, false),
        Command::Cycles { rel } => read::cycles(&graph, &ctx, rel.as_deref()),
        Command::Stats => read::stats(&graph, &ctx),
        Command::Info => read::info(&graph, &ctx),
        Command::Export { to, out } => export::run(&graph, to, out.as_deref()),
        Command::Element(_)
        | Command::Relation(_)
        | Command::Folder(_)
        | Command::Prop(_)
        | Command::View(_)
        | Command::Apply { .. }
        | Command::Skill(_)
        | Command::Init { .. }
        | Command::Web { .. }
        | Command::Validate { .. } => unreachable!("dispatched above"),
    }?;
    Ok(read::carry(out, &model, cli.fields.as_ref()))
}

impl Cli {
    fn command_write(&self) -> write::WriteCmd {
        match &self.command {
            Command::Element(c) => write::WriteCmd::Element(c.clone()),
            Command::Relation(c) => write::WriteCmd::Relation(c.clone()),
            Command::Folder(c) => write::WriteCmd::Folder(c.clone()),
            Command::Prop(c) => write::WriteCmd::Prop(c.clone()),
            _ => unreachable!("only write commands reach here"),
        }
    }
}

/// `--count` is documented as printing how many results there would be and
/// nothing else, so on a write it has to mean the same thing `--dry-run` does.
///
/// It did not, and `view auto --count` created the view while reporting a count
/// — leaving duplicate views behind on a command whose whole point was to avoid
/// changing anything. Both flags are answered in one place so a future write
/// path cannot forget one of them.
fn write_opts(cli: &Cli) -> write::Opts {
    write::Opts {
        dry_run: cli.dry_run || cli.count,
        yes: cli.yes,
        expect_checksum: cli.expect_checksum.clone(),
    }
}

fn cli_write_opts(cli: &Cli) -> write::Opts {
    write_opts(cli)
}

/// Explicit flag, then the environment, then the nearest model walking up. An
/// ambiguous directory is reported rather than guessed at.
fn find_model(explicit: Option<&Path>) -> Result<PathBuf, CliError> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }
    if let Ok(p) = std::env::var("AMCLI_MODEL") {
        return Ok(PathBuf::from(p));
    }

    let mut dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    loop {
        let mut found: Vec<PathBuf> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().and_then(|s| s.to_str()) == Some("archimate") {
                    found.push(p);
                }
            }
        }
        found.sort();
        match found.len() {
            1 => return Ok(found.remove(0)),
            0 => {}
            _ => {
                return Err(CliError::new(
                    Code::Ambiguous,
                    "ambiguous",
                    format!("{} models in `{}`", found.len(), dir.display()),
                )
                .hint("pass -m to choose one")
                .rows(
                    found
                        .iter()
                        .map(|p| output::Row::new().s("path", p.display().to_string()))
                        .collect(),
                ));
            }
        }
        if !dir.pop() {
            break;
        }
    }
    Err(CliError::new(Code::NotFound, "not_found", "no *.archimate file found")
        .hint("pass -m PATH, set AMCLI_MODEL, or run from a directory containing a model"))
}
