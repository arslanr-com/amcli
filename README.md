<h1 align="center">amcli</h1>

<p align="center">
  <b>ArchiMate models from the command line — and in your browser.</b><br>
  One static binary that reads, edits, validates, draws and serves <code>.archimate</code> files directly.<br>
  No Archi, no JVM, no daemon. Built for AI agents, pleasant for humans.
</p>

<p align="center">
  <a href="https://github.com/arslan-gg/amcli/actions/workflows/ci.yml"><img src="https://github.com/arslan-gg/amcli/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/arslan-gg/amcli/releases/latest"><img src="https://img.shields.io/github/v/release/arslan-gg/amcli?label=release" alt="Latest release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-blue.svg" alt="Apache-2.0"></a>
  <a href="https://agentskills.io"><img src="https://img.shields.io/badge/agent%20skill-included-8A2BE2" alt="Agent skill included"></a>
</p>

```console
$ amcli search payment -l 3
id-a47e7ccb…  ApplicationComponent  Payment API      /Application  3  3  1  name
id-96b26f68…  ApplicationComponent  Payment Gateway  /Application  0  1  1  name
id-9a5a25ad…  ApplicationService    Payment Service  /Application  1  2  1  name

$ amcli impact "Payment API" -D in            # what breaks if this changes?
$ amcli relation add Serving "Fraud Check" "Payment API"
$ amcli view auto "Payments" --from "Payment API" -n 3
$ amcli view render "Payments" -o payments.png     # or .svg
$ amcli web                                   # and now look at all of it
http://127.0.0.1:52341/
```

<p align="center">
  <img src="docs/web.gif" alt="amcli web: the views in their folder tree, one of them drawn as Archi draws it, the details of a figure on it, ⌘K finding a name that was typed wrong, the element table narrowed by the same misspelling, and the graph two hops out from what was picked, in light and dark">
  <br>
  <sub>The agent works on the model; <code>amcli web</code> shows you what it did — every view, every element, the graph between them — while it keeps working.</sub>
</p>

## Why

- **Archi is a GUI.** Its command line (ACLI) loads, saves, imports and reports.
  It cannot search a model, walk its graph, edit one element, validate anything
  or export an image. The headless route is jArchi scripting — a full Archi
  install plus a JRE, and a fresh JavaScript file per question.
- **Nothing permissively licensed reads *and* writes the Archi format** with
  full fidelity. amcli does, and it is Apache-2.0.
- **Agents need a tool, not an app.** Tab-separated records, exit codes to
  branch on, atomic batches, and a skill that installs the binary itself.

## Highlights

- **The agent edits, you watch — `amcli web`.** One command serves the model
  read-only to your browser on a free local port and opens it. Every view is
  drawn as Archi draws it — layer colours, figures, type icons — and a click on
  any figure opens the element: documentation, properties, every relationship
  in and out, every view it is on. Every element and relationship sits in a
  table you can filter by layer, type, folder and name. The **graph** starts
  from any element, walks out to a chosen depth, filters by layer and
  relationship type, and recentres on whatever you double-click — laid out by
  the same code `view auto` runs, so it is the drawing amcli would file.
  Search (⌘K), light and dark, SVG and PNG a click away.
  The page **follows the file**: an agent editing with amcli and a person
  watching in a browser see the same model, batch by batch, without a
  reload. Nothing on the page writes; the only verb it knows is GET.
- **Diffs a human can review.** Untouched bytes stay untouched — comments,
  whitespace, attribute order, all of it. Renaming one element changes one line:

  ```diff
  -    <element xsi:type="archimate:ApplicationComponent" name="Payment API" id="id-a47e7ccb…"/>
  +    <element xsi:type="archimate:ApplicationComponent" name="Payments API" id="id-a47e7ccb…"/>
  ```

- **Writes that cannot corrupt the model.** Every write is checked against
  Archi's own 62×62 relationship matrix and refused if the standard forbids it
  — naming what *is* allowed:

  ```console
  $ amcli relation add Composition "Transaction" "Checkout"
  error: ArchiMate does not permit Composition from DataObject to BusinessProcess
         — permitted here: Association
  ```

  Deletes cascade to every diagram object that referenced the concept, saves
  are atomic, and `--expect-checksum` refuses to write over a file that moved
  since you read it. Whatever amcli writes, Archi still opens.
- **One question, one call.** `search`, `get`, `trace`, `path`, `impact`,
  `cycles`, `query 'layer=Application and deg>10'`. Data on stdout, context on
  stderr, exit codes an agent can branch on: `3` not found, `4` ambiguous —
  both come back with something to retry.
- **Atomic batches.** `amcli apply` takes JSONL, resolves forward references
  between lines and writes once. If any line fails, the file is byte-identical.
- **Reproducible rebuilds.** Keep the batches in git and pass `--id-seed`: ids
  derive from what they name, so regenerating an unchanged model produces an
  unchanged file. `amcli export views` goes the other way — it derives the
  batch that rebuilds every view from the model, so a drawing gets a
  declarative form to review without a second source of truth to keep in step.
  Export, apply, and the file is byte-identical.
- **Views drawn to be read.** Layout works from the graph alone, tries several
  layerings and keeps the least tangled: every edge one straight line, kept off
  the boxes, boxes sized to their labels, and — wherever the graph allows it —
  nothing crossing. [How it works →](docs/layout.md) Past a dozen views, file
  them: `-f /Views/<name>` on `create` and `auto`, `view move` for the rest,
  and a view keeps its id when it moves.
- **Agent-ready.** Ships an [Agent Skill](https://agentskills.io) that teaches
  Claude Code, Codex and friends the workflow — and keeps the binary current
  every session.

## Install

**For an agent** — the skill installs and updates the binary itself:

```bash
npx skills add arslan-gg/amcli
```

**Just the binary** — checked against the release's SHA256SUMS before it is
unpacked, with no flag that skips that, into `~/.local/bin`, no `sudo`, no
shell config edited:

```bash
curl -fsSL https://raw.githubusercontent.com/arslan-gg/amcli/main/skills/amcli/scripts/install.sh | sh
```

```powershell
irm https://raw.githubusercontent.com/arslan-gg/amcli/main/skills/amcli/scripts/install.ps1 | iex
```

**From source** (Rust 1.90+; the only route for platforms without a prebuilt
binary — the installers fall back to it on their own):

```bash
cargo install --git https://github.com/arslan-gg/amcli --locked amcli-cli
```

Add `--tag vX.Y.Z` to build a release rather than the branch, which is what
the installers do when they take this route themselves.

Prebuilt for macOS (Apple silicon, Intel), Linux (x86_64, aarch64, static
musl) and Windows x64. `amcli skill install` writes the skill from the binary
if you went binary-first.

### Updating

**The binary updates itself.** The skill runs the installer at the start of
every session; it stops as soon as it sees that the newest release is already
installed, and keeps the binary you have when there is no network. So an agent
that has the skill is on the current binary without being asked.

**The skill is updated by the tool that installed it:**

```bash
npx skills update amcli      # this skill, from the repository's default branch
npx skills update            # or every skill you have
npx skills list              # what is installed, and where each came from
```

It asks whether you mean the project's skills or your global ones; `-y` takes
the obvious answer, `-g` and `-p` say which outright — that is the form for a
script.

If you installed binary-first, `amcli skill install` writes the skill the
binary carries — re-run it after upgrading the binary, and pass `--force` to
overwrite a copy you have edited. It refuses outright when `npx skills` owns
the directory, because that install has its own lock file and would overwrite
the change on its next update; `npx skills update amcli` is the way in there.

One thing worth knowing when the two disagree: the skill ships from the default
branch and the binary from the newest tag, so the skill is normally the *newer*
of the two. Never "fix" that by reinstalling the skill from an older binary —
it would talk you down a version. `amcli` reconciles it itself, and says so
when a command it is asked for does not exist yet.

## A tour

```bash
amcli stats                                   # how big, and of what
amcli get "Payment API"                       # the concept and everything it touches
amcli trace "Payment API" -n 2                # the neighbourhood
amcli path "Web App" "Customer Database"      # how are these connected?
amcli query 'kind=element and view=0'         # modelled but drawn nowhere

amcli element  add ApplicationComponent "Refund Service" -f /Application
amcli relation add Access "Refund Service" "Refund Record" --access rw
amcli prop set "Refund Service" owner team-payments
amcli element  delete "Refund Service" -y     # cascades, and says to what

amcli apply batch.jsonl                       # many edits, one write, all or nothing
amcli validate                                # rules, with a `fix` per finding
amcli view auto "Refunds" --from "Refund Service" -n 2
amcli view move "Refunds" -f /Views/Payments  # file it; the id does not change
amcli export views                            # the batch that rebuilds every view
amcli export mermaid                          # a quick diagram for a chat window
amcli view render "Refunds" -o refunds.png     # or .svg; --scale 2 for a slide
amcli web                                     # look at all of it in a browser, read-only
amcli web --no-open                           # just the URL — a container, or an agent handing it to you
```

`amcli --help` lists everything; `amcli <command> --help` goes deep. Reads are
bare verbs, writes are noun-verb, `--dry-run` and `--count` never write.

## Development

```bash
cargo test           # byte-identity over real Archi files, property tests, layout sweep
cargo xtask verify   # generated tables still match the vendored Archi assets
```

`tests/corpus/` is real Archi output; the identity test asserts that parsing and
writing every file is a byte-for-byte no-op. `assets/archi/` is vendored from
[archimatetool/archi](https://github.com/archimatetool/archi) (MIT) and turned
into the type tables and relationship matrix by `cargo xtask codegen`.

## Status

Read, write, validate, views, SVG and PNG, and the web viewer all work and are
tested against real Archi files. A `.archimate` file is read whether it is
plain XML or the ZIP an embedded image makes of it.

## Licence and trademarks

Apache-2.0. See [NOTICE](NOTICE) for the vendored Archi assets and their MIT
licence.

ArchiMate® is a registered trademark of The Open Group. Archi® is a trademark
of Phillip Beauvoir. This project is independent and is not affiliated with,
endorsed by, or certified by either; it reads and writes their file formats for
interoperability.
