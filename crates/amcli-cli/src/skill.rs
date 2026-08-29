//! Installing the agent skill.
//!
//! `npx skills add arslanr-com/amcli` is the primary route and copies
//! `skills/amcli/` out of the repository verbatim. This command is the reverse
//! route, for someone who got the binary first. Both must produce the *same*
//! bytes, so the embedded copy is taken from that same directory and nothing
//! is generated into it — see `Commands` below for why the command reference
//! is not a file.
//!
//! Installation targets `~/.agents/skills/`, the documented cross-tool location
//! that Codex reads natively, and symlinks `~/.claude/skills/` at it, which is
//! exactly what `npx skills add` does.

use std::path::{Path, PathBuf};

use clap::{CommandFactory, Subcommand};

use crate::output::{CliError, Code, Output, Row};

/// The skill, compiled in. Editing these files and rebuilding is the only way
/// to change what gets installed.
///
/// `scripts/install.sh` is here too: the skill is what teaches an agent to
/// install the binary, so the installer has to travel with it.
const FILES: &[(&str, &str)] = &[
    ("SKILL.md", include_str!("../../../skills/amcli/SKILL.md")),
    ("references/types.md", include_str!("../../../skills/amcli/references/types.md")),
    ("references/batch.md", include_str!("../../../skills/amcli/references/batch.md")),
    ("scripts/install.sh", include_str!("../../../skills/amcli/scripts/install.sh")),
    ("scripts/install.ps1", include_str!("../../../skills/amcli/scripts/install.ps1")),
    ("agents/openai.yaml", include_str!("../../../skills/amcli/agents/openai.yaml")),
];

#[derive(Subcommand, Clone)]
pub enum SkillCmd {
    /// Write the skill where agents look for it.
    Install {
        /// Install into ./.agents/skills instead of the home directory.
        #[arg(long)]
        project: bool,
        /// Overwrite content that differs.
        #[arg(long)]
        force: bool,
        /// Copy rather than symlink, for filesystems without symlinks.
        #[arg(long)]
        copy: bool,
    },
    /// Remove the skill and the links this command created.
    Uninstall {
        #[arg(long)]
        project: bool,
    },
    /// Print the skill instead of writing it.
    Show,
    /// Every command and flag this binary has, in one page.
    Commands,
    /// Where the skill would go.
    Path {
        #[arg(long)]
        project: bool,
    },
}

pub fn run(cmd: &SkillCmd) -> Result<Output, CliError> {
    match cmd {
        SkillCmd::Show => {
            print!("{}", FILES[0].1);
            Ok(Output::empty())
        }
        SkillCmd::Commands => {
            print!("{}", command_reference());
            Ok(Output::empty())
        }
        SkillCmd::Path { project } => {
            let root = target(*project)?;
            Ok(Output::one(
                Row::new()
                    .s("skill", root.display().to_string())
                    .s("claude_link", claude_link(*project)?.display().to_string()),
            ))
        }
        SkillCmd::Install { project, force, copy } => install(*project, *force, *copy),
        SkillCmd::Uninstall { project } => uninstall(*project),
    }
}

fn home() -> Result<PathBuf, CliError> {
    // Native Windows shells set USERPROFILE and not HOME; without the fallback
    // every `amcli skill` command exits 7 there.
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| CliError::new(Code::Io, "io", "neither HOME nor USERPROFILE is set"))
}

/// True when `npx skills add` owns this skill.
///
/// Its lock file is the only record that it manages the directory, and the
/// hash it stores is the upstream git tree SHA, so it cannot notice that we
/// rewrote the folder — it would simply overwrite our version on the next
/// upstream change, or leave a dangling entry if we deleted it.
fn managed_by_skills_cli() -> bool {
    let Ok(home) = home() else { return false };
    let Ok(text) = std::fs::read_to_string(home.join(".agents/.skill-lock.json")) else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v.get("skills")?.get("amcli").cloned())
        .is_some()
}

fn target(project: bool) -> Result<PathBuf, CliError> {
    Ok(if project {
        std::env::current_dir()
            .map_err(|e| CliError::new(Code::Io, "io", e.to_string()))?
            .join(".agents/skills/amcli")
    } else {
        home()?.join(".agents/skills/amcli")
    })
}

fn claude_link(project: bool) -> Result<PathBuf, CliError> {
    Ok(if project {
        std::env::current_dir()
            .map_err(|e| CliError::new(Code::Io, "io", e.to_string()))?
            .join(".claude/skills/amcli")
    } else {
        home()?.join(".claude/skills/amcli")
    })
}

fn install(project: bool, force: bool, copy: bool) -> Result<Output, CliError> {
    let root = target(project)?;
    let io = |e: std::io::Error, p: &Path| {
        CliError::new(Code::Io, "io", format!("`{}`: {e}", p.display()))
    };

    if !force && !project && managed_by_skills_cli() {
        return Err(CliError::new(
            Code::Conflict,
            "conflict",
            format!("{} was installed by `npx skills add`", root.display()),
        )
        .hint("that install is already current; use `npx skills update amcli` to change it, or --force to overwrite it here"));
    }

    // Refuse to clobber content someone may have edited, unless told.
    if root.exists() && !force {
        let differs = FILES.iter().any(|(name, body)| {
            std::fs::read_to_string(root.join(name)).map(|c| c != *body).unwrap_or(true)
        });
        if differs {
            return Err(CliError::new(
                Code::Conflict,
                "conflict",
                format!("{} already holds different content", root.display()),
            )
            .hint("pass --force to overwrite"));
        }
    }

    let mut written = Vec::new();
    for (name, body) in FILES {
        let path = root.join(name);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| io(e, dir))?;
        }
        std::fs::write(&path, body).map_err(|e| io(e, &path))?;
        written.push(name.to_string());
    }

    // Nothing is generated into the directory. A committed command reference
    // would be one more thing that goes stale, and a generated one would make
    // this install differ from what `npx skills add` copies — which is exactly
    // the difference the check above is trying to detect. `amcli skill
    // commands` reads the tree out of the running binary instead, and so
    // cannot be out of date by construction.

    // One link, for Claude Code. Codex reads ~/.agents/skills natively, so a
    // second copy under ~/.codex would only be another thing to keep in sync.
    let link = claude_link(project)?;
    let mut linked = false;
    if let Some(parent) = link.parent()
        && (parent.exists() || project)
    {
        std::fs::create_dir_all(parent).map_err(|e| io(e, parent))?;
        if link.exists() || link.symlink_metadata().is_ok() {
            let _ = std::fs::remove_file(&link);
            let _ = std::fs::remove_dir_all(&link);
        }
        linked = if copy {
            copy_tree(&root, &link).is_ok()
        } else {
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(&root, &link).is_ok()
            }
            #[cfg(not(unix))]
            {
                copy_tree(&root, &link).is_ok()
            }
        };
    }

    let mut out = Output::one(
        Row::new()
            .s("skill", root.display().to_string())
            .n("files", written.len() as i64)
            .b("claude_link", linked),
    )
    .note(format!("installed {} files into {}", written.len(), root.display()));

    if linked {
        out = out.note(format!("linked {}", link.display()));
    } else {
        out = out.note(format!(
            "no Claude Code directory at {}; Codex and other tools read {} directly",
            link.parent().map(|p| p.display().to_string()).unwrap_or_default(),
            root.display()
        ));
    }
    Ok(out.note("start a new agent session to pick it up"))
}

fn uninstall(project: bool) -> Result<Output, CliError> {
    let root = target(project)?;
    let link = claude_link(project)?;
    let mut removed = Vec::new();

    // Deleting a directory `npx skills add` owns would leave its lock file
    // describing something that is no longer there, and nothing would repair
    // that but a manual edit.
    if !project && managed_by_skills_cli() {
        return Err(CliError::new(
            Code::Conflict,
            "conflict",
            format!("{} was installed by `npx skills add`", root.display()),
        )
        .hint("remove it the same way: `npx skills remove amcli`"));
    }

    // Only the link is removed, never whatever it pointed at if it was not ours.
    if link.symlink_metadata().is_ok() {
        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_dir_all(&link);
        removed.push(link.display().to_string());
    }
    if root.exists() {
        std::fs::remove_dir_all(&root)
            .map_err(|e| CliError::new(Code::Io, "io", format!("`{}`: {e}", root.display())))?;
        removed.push(root.display().to_string());
    }
    if removed.is_empty() {
        return Ok(Output::empty().note("nothing to remove"));
    }
    Ok(Output::rows(removed.into_iter().map(|p| Row::new().s("removed", p)).collect()))
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let dest = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_tree(&e.path(), &dest)?;
        } else {
            std::fs::copy(e.path(), dest)?;
        }
    }
    Ok(())
}

/// Render the whole command tree out of clap, so the reference is exactly what
/// this binary does rather than what some release of it once did.
///
/// This is why there is no `references/commands.md`: a file would have to be
/// either committed (and stale by the next release) or generated (and then
/// different between the two install routes).
fn command_reference() -> String {
    let mut out = String::from(
        "# amcli commands\n\n\
         Generated from the binary by `amcli skill install`. Do not edit — it is\n\
         overwritten on every install, which is what keeps it from going stale.\n\n",
    );
    let cmd = crate::Cli::command();
    out.push_str("## Global flags\n\n```\n");
    for a in cmd.get_arguments() {
        let long = a.get_long().map(|l| format!("--{l}")).unwrap_or_default();
        let short = a.get_short().map(|s| format!("-{s}, ")).unwrap_or_default();
        let help = a.get_help().map(|h| h.to_string()).unwrap_or_default();
        out.push_str(&format!("  {short}{long:<24} {}\n", help.replace('\n', " ")));
    }
    out.push_str("```\n\n## Commands\n\n");
    for sub in cmd.get_subcommands() {
        let about = sub.get_about().map(|a| a.to_string()).unwrap_or_default();
        out.push_str(&format!("### `{}`\n\n{}\n\n", sub.get_name(), about.replace('\n', " ")));
        let nested: Vec<_> = sub.get_subcommands().collect();
        if !nested.is_empty() {
            out.push_str("```\n");
            for n in nested {
                let a = n.get_about().map(|a| a.to_string()).unwrap_or_default();
                out.push_str(&format!(
                    "  amcli {} {:<16} {}\n",
                    sub.get_name(),
                    n.get_name(),
                    a.replace('\n', " ")
                ));
            }
            out.push_str("```\n\n");
        }
    }
    out
}
