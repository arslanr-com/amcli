---
name: amcli
description: >-
  Read, search, traverse, edit, validate and draw ArchiMate architecture models
  (.archimate files) from the command line with the `amcli` binary. Use when the
  user asks about an enterprise, solution or application architecture model;
  when they mention ArchiMate, Archi, coArchi, a .archimate file, application
  components, business processes, capabilities or an EA repository; or when a
  repository contains a *.archimate file. Also use for: finding which services
  depend on a component, tracing dependencies, assessing the blast radius of a
  change, listing what reads or writes a data object, adding or renaming
  elements and relationships, checking a model for rule violations, and
  producing a diagram. Prefer this over opening the model XML directly — these
  files run to megabytes, reading one wastes context, and hand-editing the XML
  corrupts models in ways Archi then refuses to open.
license: Apache-2.0
compatibility: >-
  Needs a shell and the `amcli` binary; if the binary is missing, the Setup
  section installs it and needs network for that one step. After that: no
  network, no daemon, and no Archi installation — amcli works directly on the
  model file. The one optional server is `amcli web`, a local read-only
  viewer that runs only while asked to and stops with Ctrl-C.
metadata:
  homepage: https://github.com/arslan-gg/amcli
  binary: amcli
  spec: ArchiMate 3.2
---

# amcli — ArchiMate models from the command line

A single binary that reads and edits ArchiMate model files directly. No GUI, no
JVM, no daemon — and, when a person wants to *look* at the model, `amcli web`
serves it read-only to their browser from the same binary.

**Never read a `.archimate` file with Read or cat.** They run to megabytes of
XML. Every question you have is one `amcli` command, and editing that XML by
hand corrupts models.

## Setup

Run this before anything else, **every session, without exception** — it
installs amcli if it is missing, updates it if a newer release exists, and
otherwise does nothing but print where the binary is. Never start work on a
binary you have not tried to update this session; an old amcli lays views out
differently and refuses flags this file describes:

    AMCLI=$(sh ~/.agents/skills/amcli/scripts/install.sh)
    $AMCLI --version

The installer sits next to this file, which is normally `~/.agents/skills/amcli`,
or `./.agents/skills/amcli` for a project install. It prints the absolute path
of the binary on stdout and nothing else. **Use `$AMCLI` for the rest of the
session.** A newly installed binary is usually not on the current shell's PATH
yet, so plain `amcli` will still report "command not found" even though the
install succeeded — that is the single most likely way this goes wrong.

Running it every time is cheap and safe: it always asks GitHub for the newest
release (one HTTP redirect); if that is what is installed it downloads
nothing; if it is newer it fetches it, checked against the release's SHA256SUMS
before it is unpacked and with no flag that skips the check; and only when
there is no network at all does it keep whatever is installed, and it says so
on stderr.

**Native Windows PowerShell**, where there is no `sh`, use the PowerShell one
instead. It follows the same contract:

    $AMCLI = & ~\.agents\skills\amcli\scripts\install.ps1

Either installer asks for nothing, never elevates, and never edits a shell
config. If no prebuilt binary matches the platform it builds one with cargo on
its own. Do not pipe either from a URL — they are already on disk.

This skill is written for **amcli 0.13.0**. The installer always gives you the
newest *release*, and this file ships from the repository's main branch, so
for a short while after a change lands the binary can be one release behind
what is described here. If a command or flag below is refused, the binary
says so and names the fix; carry on with what the binary you have does
support, and do not reinstall the skill to match the binary — that would only
take you backwards.

## Finding the model

amcli finds the model on its own: `-m PATH`, else `$AMCLI_MODEL`, else the
nearest `*.archimate` walking up from the working directory. If several are
found it exits 4 and lists them — pass `-m`.

## The loop

Start every architecture question here. Do not open source code first.

    amcli stats                       # how big is this thing, and of what
    amcli search <term>               # find the concept, get its id
    amcli get <id-or-name>            # what it is, and everything it touches
    amcli trace <id-or-name> -n 2     # the neighbourhood
    # only now read source code, and only the files the model pointed you at

## Flags mean one thing each

    -t concept type   -r relationship type   -f folder   -D direction(out|in|both)
    -n depth          -l limit               -m model    -F format   -o output file

Subjects are positional, never flags:

    amcli element  add ApplicationComponent "Refund Service" -f /Application
    amcli relation add Serving "Refund Service" "Checkout Service"

## Output and token economy

The default output is tab-separated records, one per line — cheap to read and
easy to `cut -f2`. Counts, hints and a `# id<TAB>name<TAB>…` column header go to
stderr, so read stderr to learn what the columns are. Add `-F json` only when you
need nested structure, such as the relationship ids inside `get`. JSON is always
the same shape — `{"ok":…,"data":[…],"meta":{…}}`, success or failure — so
`.data[]` is the path whether or not `-q` is there; `-q` only quietens stderr.

    amcli query 'layer=Application' --count    # ask "how many" FIRST, always
    amcli search auth -l 10 --fields id,name   # project down
    amcli list --fields -documentation         # or drop a field
    amcli query 'prop:reg-id=RG-14' --fields name,prop:reg-id   # print what you filtered on

**A field you can filter on is a field you can print.** Besides the columns a
command prints by default, `--fields` takes `doc`, `layer`, `kind` and any
`prop:KEY` — matched case-insensitively, as the filter matches it — and the
value is appended as a column, empty where the concept has none. That is the
route to one property: you do not need `-F json` to read it. It works on
`view list` too.

**A capped answer says so on stderr, and `-q` cannot silence it.** Every list
is cut to `-l` (50 by default) and prints `showing 50 of 83` when it cuts;
`-l 0` gives all of them, and `--count` gives the true total without the rows.
`-q` drops the header and the notes — never that line, and never the
`"truncated"` field in the JSON envelope. If you are counting, count with
`--count` or check the line.

`--count` and `--dry-run` never write, on any command.

Never run an unfiltered `list` on an unfamiliar model. Run `amcli stats` first.

## Addressing concepts

    id:5dde26f7                      an id — always unambiguous, always prefer it
    "Payment API"                    an exact name
    ApplicationComponent:"Payment"   a name qualified by type
    "*Payment*"                      a glob

Filter expressions, quoted as one argument:

    amcli query 'type=ApplicationComponent and name~payment'
    amcli query 'prop:owner=team-a and not folder^=/Technology'
    amcli query 'layer=Application and deg>10'
    amcli query 'out:Access~Customer'    # everything that accesses something matching
    amcli query 'kind=element and view=0'  # in the model but on no diagram
    amcli query 'kind=element and view~"Refunds"'   # what a view holds

Operators: `=` exact · `~` contains · `^=` prefix · `=~` regex · `!=` · `>` `<`
on `deg` and `view`. Fields: `id name type kind layer folder doc deg view
prop:KEY in:RelType out:RelType`.

Two of those are easy to get wrong:

- `kind` is `element` or `relation`. Without it, filters like `layer!=none`
  return relationships mixed in with elements, and relationships have no name.
  A relationship row carries `source`, `source_name`, `target` and `target_name`
  after the usual columns, so `amcli query 'kind=relation'` and `amcli get` on a
  relationship both say what it joins — read that before deleting one.
- `type` takes one ArchiMate type and accepts either spelling — `Triggering` and
  `TriggeringRelationship` are the same thing. So does `-t`. A type that does not
  exist is exit 2 with the model's own types listed, never an empty result.
- `view` compares as a **number** against how many views a concept is drawn on
  (`view=0`, `view>1`), and as a **name** otherwise (`view~"Payments"`). The
  `views` column on every concept row is that same count.

## Exit codes — branch on these, do not parse messages

    0 ok   2 usage   3 not found   4 ambiguous   5 invalid   6 conflict
    7 io   8 unsupported

On **3**, the response lists the nearest names — retry with one, do not run
another search. On **4**, it lists candidates, each with a ready-to-paste
`id:` selector. Never work around an ambiguity by guessing; re-run with the id.

## Graph questions

    amcli path "Web App" "Customer Database"   # how are these connected?
    amcli impact id:5dde26f7 -D in             # what breaks if this changes?
    amcli neighbors id:5dde26f7 -r Serving     # only Serving relationships
    amcli ancestors "Payment API"              # what composes or aggregates it
    amcli descendants "Payments Capability"    # the composition tree
    amcli cycles                               # dependency cycles

## Editing

    amcli init "Model Name" -o model.archimate   # a new, empty model
    amcli element  add ApplicationComponent "Refund Service" --doc "…"
    amcli element  rename id:c40a19b7 "Refunds Service"
    amcli relation add Access "Refunds Service" "Refund Record" --access rw
    amcli element  doc id:c40a19b7 "What it is for"
    amcli element  move id:c40a19b7 -f /Application/Payments
    amcli prop set id:c40a19b7 owner team-payments
    amcli element  delete id:c40a19b7 -y

Never hand-write the XML skeleton for a new model — `amcli init` writes it with
the nine folders Archi expects.

Every write is checked against the ArchiMate relationship matrix first and
refused (exit 5) if the standard forbids it — and the refusal names what *is*
permitted between those two types, so read it rather than guessing again.

Deleting refuses by default when it would take other things with it, and the
refusal is the impact report. Add `-y` once you have read it.

Use `--dry-run` when unsure. Use `--expect-checksum` when you read the model on
an earlier turn and are writing now:

    CS=$(amcli info -F json | jq -r '.data[0].checksum')
    amcli element rename id:x "New" --expect-checksum "$CS"    # exit 6 if it moved

**For more than two edits, use one atomic batch rather than a sequence.**

    amcli apply - <<'EOF'
    {"op":"element.add","type":"ApplicationComponent","name":"Refund Service","ref":"r","if_absent":true}
    {"op":"element.add","type":"DataObject","name":"Refund Record","ref":"rec","if_absent":true}
    {"op":"relation.add","type":"Access","source":"ref:r","target":"ref:rec","access":"rw","if_absent":true}
    EOF

`ref` names a line's result so a later line can point at it before its id
exists. `if_absent` makes the batch safe to re-run. If any line fails, nothing
is written and the file is byte-identical. View operations go in the same
batch — `view.create`, `view.add`, `view.auto`, `view.layout`, `view.rename`,
`view.move`, `view.delete`, same fields as the commands — so a view is built
and laid out with the concepts it shows, in one write; `references/batch.md`
has those and the folder ops.

Deletes go in a batch too — `element.delete`, `relation.delete`, `prop.unset` —
which is what lets a change that is really a *replacement* land as one write.
Retyping a relationship is the common one, and the model never passes through a
state where it says the wrong thing:

    amcli apply - <<'EOF'
    {"op":"relation.delete","target":"id:8f3c1a02","if_present":true}
    {"op":"relation.add","type":"Realization","source":"Payment API","target":"Payments","if_absent":true}
    EOF

`if_present` is `if_absent` for deletes: with it, the second run of that batch
finds nothing to do and writes a file identical to the one it read.

**If the model is regenerated from batches you keep in the repository**, pass the
same `--id-seed` every time (or set `$AMCLI_ID_SEED`). Ids are then derived from
what they name instead of drawn at random, so rebuilding an unchanged model
produces an unchanged file and the diff shows only what you meant to change.
Without a seed every rebuild reissues every id and the whole file appears to
change. Use one seed per model and do not change it afterwards.

## Before you finish any edit

    amcli validate

Exit 5 means the model has errors. Each finding names a line in the file and
carries a `fix` command. `amcli validate --fix` applies only the repairs that
are derived rather than chosen — orphaned diagram objects and stale view
mirrors — and never deletes anyone's modelling.

## Views and diagrams

    amcli view list                            # columns are named on stderr
    amcli view create "Refund Flow" -f /Views/Payments
    amcli view auto "Refund Flow" --from "Refund Service" -n 2
    amcli view add "Refund Flow" "Fraud Check" # drawn *and* wired to what is there
    amcli view layout "Refund Flow" --relayout-all
    amcli view rename "Refund Flow" "Refunds"
    amcli view move "Refunds" -f /Views/Payments   # re-file, id unchanged
    amcli view viewpoint "Refunds" application_cooperation   # or "" to clear
    amcli view doc "Refunds" "What this drawing is for."      # or "" to clear
    amcli view delete "Refunds"                # removes the drawing, no concept
    amcli view render "Refunds" -o refund.svg
    amcli view render "Refunds" -o refund.png --scale 2   # a raster, from the extension
    amcli export views                         # the batch that rebuilds every view
    amcli export mermaid                       # a quick inline diagram for chat

A view name is unique: creating a second view with a name already in use is exit
6. Pass `--replace` to overwrite the old one, which is what makes a
regenerate-everything script re-runnable:

    amcli view auto "Refund Flow" --from "Refund Service" --replace

A **viewpoint** narrows what a view is meant to say, and Archi shows it in the
properties. `create` and `auto` take `--viewpoint` (`"viewpoint"` in a batch);
`view viewpoint` sets or clears it on a drawing that already exists, which is
what you want when a view grew past the one it was filed under. The id must be
one of the 25 ArchiMate ones — a wrong one is exit 2 and the hint lists them
all — and `""` clears it. Nothing is enforced by it: putting a concept the
viewpoint does not cover on the view is a note on stderr, not a refusal.

A view carries **documentation** exactly as a concept does, and Archi shows it
in the properties. `view doc` writes it (`"op":"view.doc"` in a batch), `""`
clears it, and `view list --fields name,doc` reads it back — a drawing is the
one thing you hand to a person, so say on it what it is for.

**Past about a dozen views, file them in folders.** `create` and `auto` take
`-f /Views/<name>` (`"folder"` in a batch), `view move` re-files one already
drawn, and `view list` reports where each sits. The folder must exist first:
`amcli folder list` shows what does, and `amcli folder add /Views "Programme"`
makes one — asked twice it returns the folder already there rather than a
second one, so a script may simply declare the folders it needs. It must be
under `/Views`: Archi never shows a diagram filed anywhere else, so amcli
refuses (exit 5) rather than writing a model with a view you cannot open.
Re-filing never changes a view's id, so a regenerate-everything script keeps
producing the same diff. `folder delete` removes an empty folder and refuses a
full one; both folder commands take an `id:` as well as a path, which is the
only way to address two folders that ended up sharing one.

**A view has no declarative form of its own** — what it holds is only geometry
in the file, so a diff cannot answer "which fifteen elements are on this view".
`amcli export views` derives one: the batch of `folder.add`, `view.create`,
`view.add` and `view.layout` operations that rebuilds every view, readable and
reviewable, which `amcli apply` takes straight back:

    amcli export views -o views.jsonl     # read it, review it, edit it
    amcli apply views.jsonl               # and the model is byte-identical

The round trip is exact, and re-running it changes nothing, so this is the way
to regenerate views from something a human reviewed. Do **not** keep such a
file beside the model as the source of truth: it is derived, and a derived file
kept by hand goes stale. Generate it when you need to read or change a view,
apply it, and let the model stay the one record. Views drawn in Archi with
notes, groups or nested objects are the limit — `view.add` cannot put those
back, and the export says so in a comment rather than pretending.

`--layout` takes `auto` (the default), `layered` or `grid`; `view layout` spells
the same flag `--algorithm` and both commands accept both names. `auto` layers
the graph — folding a rank too wide to read onto several lines rather than
running it off the page — and only lays out a grid when that would be both
squarer and no more tangled, which in practice means an edgeless set. The row
reports which algorithm actually ran.

Layout places by the graph alone: it does not consult the ArchiMate layer,
and it does not insist that arrows point down. Several layerings are tried
and the least tangled drawing kept — fewest crossings and lines through boxes
— so a hub's fan sits half above it and half below, a value stream lies along
one row under the lifecycle that composes it, and two capabilities serving
the same crowd take opposite sides of it. Arrows point down only when that
costs nothing. Do not "fix" an upward arrow by moving boxes: the layout chose
that to keep a line off a box.

Layout sizes each box to the label it has to hold — measured against the room
Archi leaves inside a figure, which is the box less its margin and, when the
type icon shows, less that icon's width off *both* sides — and draws every
edge as one straight line, no bendpoints, ever, placing the boxes so the lines
stay off them. `view
layout` writes sizes back along with positions and straightens every
connection, so `--relayout-all` redraws the view, it does not just shuffle it.
Run it after a batch of edits rather than after each one — and over every view
when a new amcli lays out better, which needs no record of what each view
holds, because the model already has it:

    amcli view list -q --fields name |
      while IFS= read -r v; do amcli view layout "$v" --relayout-all -q; done

`view render` draws the geometry the model actually stores — SVG by default,
PNG when `-o` ends in `.png` or `--as png` says so (`--scale 2` doubles the
pixels; labels use the machine's fonts, so a container with none draws no
text and says so). `export mermaid` and `export dot` re-lay-out, so they are
for a quick look, not for reproducing someone's diagram.

## Showing the model to a person

    amcli web                     # local read-only viewer; prints the URL and opens the browser
    amcli web --no-open           # print the URL only — a container, SSH, or when the person opens it
    amcli web --port 8080         # a fixed port; the default is a free one the OS picks

`amcli web` serves the model on `127.0.0.1` only: the views in their folder
tree, each drawn as Archi draws it (colours, figures, type icons) with SVG and
PNG a click away; a table of every element and of every relationship; and a
graph laid out by the same code `view auto` runs — centred on an element to a
chosen depth, or the whole model when nothing is centred, with SVG and PNG of
whatever is on screen. What narrows a page — the folder tree, the filters, the
pinned elements — is in the sidebar on the left; whatever was last clicked is
described in the panel on the right; moving between pages keeps a reader's
place, so the filters they set and the corner of a drawing or of the graph
they had zoomed into are still there when they come back; and ⌘K searches the
whole model from anywhere — by the letters rather than the spelling, so
initials and a typo still find the thing, and every box on the page searches
the same way. Nothing on the page edits anything, and the process writes
nothing — it is the one command that keeps running after it has answered, so
**run it in the background** (`amcli web --no-open &`, or a second terminal)
and hand the URL to the person. Stdout is one tab-separated record — the URL,
then the model path — so the URL is `cut -f1`; `-q` drops the header and the
note on stderr but not the second field. `-F json` puts both in the usual
envelope. Ctrl-C stops it, and nothing needs cleaning up.

The page follows the file: keep editing the model with amcli in the same
session and the browser picks up each write within a couple of seconds, so a
person can watch a batch land view by view. When the file is mid-write or
invalid the page keeps showing the last good model and says so.

Opening the browser is the default and is what a person at the same machine
wants. In a container or over SSH there is no browser to open — pass
`--no-open`; the URL is printed either way, and a failed open is a note on
stderr, never an error.

## Going deeper

| Where | When |
|---|---|
| `amcli skill commands` | you need a subcommand or flag not shown above — it prints the whole tree, read out of the binary you are actually running, so it is never out of date |
| `amcli <command> --help` | you need one command's flags in detail |
| `references/types.md` | you need an exact ArchiMate 3.2 type name, or which relationships are legal between two types |
| `references/batch.md` | you are writing a batch of more than about ten operations |
