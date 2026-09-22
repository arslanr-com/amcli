# Batches

    amcli apply ops.jsonl
    amcli apply -            # from stdin

One JSON operation per line. Blank lines and lines starting with `#` or `//`
are ignored.

**All or nothing.** Every line is applied in memory, and the file is written
once at the end. If any line fails, the file is byte-identical to what it was
and the error names the line number. There is no partial application to clean
up, and no rollback step that could itself fail.

## Operations

    {"op":"element.add","type":"ApplicationComponent","name":"X","folder":"/Application","doc":"…","props":{"owner":"team"},"ref":"x","if_absent":true}
    {"op":"relation.add","type":"Serving","source":"ref:x","target":"Y","name":"…","access":"rw","doc":"…","ref":"r","if_absent":true,"no_draw":false}
    {"op":"element.rename","target":"ref:x","name":"New name"}
    {"op":"element.doc","target":"id:abc","text":"…"}
    {"op":"element.delete","target":"id:abc","if_present":true}
    {"op":"relation.delete","target":"id:abc","if_present":true}
    {"op":"prop.set","target":"ref:x","key":"owner","value":"team-a"}
    {"op":"prop.unset","target":"ref:x","key":"owner"}
    {"op":"folder.add","parent":"/Application","name":"Payments"}
    {"op":"folder.delete","path":"/Application/Payments"}
    {"op":"folder.rename","path":"/Application/Payments","name":"Payments and Refunds"}
    {"op":"folder.move","path":"/Application/Payments and Refunds","parent":"/Application/Core"}

`access` is Access relationships only: `read`, `write`, `rw`, `unspecified`.
`name` labels the line — Archi reads an Association as "related to" without
one and "owns" with it; most relationships have none.

In every op but `relation.add`, `target` is the thing operated on. In
`relation.add` — and only there — `source` and `target` are the two ends.

**A field an operation does not take fails the line**, exit 2, naming the
line and the fields the operation does take — nothing is written, and
`--dry-run` says the same. A `relation.add` with a `name` used to be accepted,
reported as applied, and written without one; a batch that silently does less
than it says is not atomic. A field the skill documents and the binary
refuses is an old binary: the hint says so, and how to upgrade.

**`relation.add` draws the relationship on every view that already shows
both ends**, exactly as the command does; the row's `views` counts them.
`"no_draw":true` adds it to the model only. A line skipped by `if_absent`
draws nothing and reports the views the existing relationship is on.

`folder.rename` changes one attribute — the folder keeps its id and
everything in it stays where it is — and `folder.move` re-files a folder
with its contents under another folder of the same tree; a folder never
leaves the top-level folder of its type, and the top-level folders
themselves cannot be renamed or moved.

`relation.delete` takes the relationship itself, which you address by id or by
a `ref:` from an earlier line; a relationship rarely has a name to call it by.
`amcli get` on either end lists the relationships it touches with their ids,
and `amcli query 'kind=relation'` carries `source`, `source_name`, `target`
and `target_name` on every row. It refuses anything that is not a
relationship: aimed at an element by accident it would take that element's
whole cascade with it.

Deleting cascades in a batch the same way it does at the prompt — a
relationship's diagram connections go with it, an element's relationships go
with it — but without the confirmation, since a batch is written before it is
run. `removed` in the report counts everything the line took, the concept
itself included. Use `--dry-run` first if you are not sure.

Views too — each mirrors the `view` subcommand of the same name, with the
same fields, and takes a `ref:` wherever it takes a concept:

    {"op":"view.create","name":"Payments","viewpoint":"application_cooperation","folder":"/Views/Payments","replace":true}
    {"op":"view.add","view":"Payments","target":"ref:x"}
    {"op":"view.add","view":"Payments","target":"Checkout","x":240,"y":0,"no_connect":true}
    {"op":"view.add","view":"Payments","target":"Fraud Check","into":"Checkout"}
    {"op":"view.add","view":"Payments","target":"Fraud Check","again":true,"x":600,"y":40,"width":200,"height":80,"ref":"fc2"}
    {"op":"view.group","view":"Payments","name":"Card rails","ref":"g1","into":"Checkout","x":0,"y":0,"width":400,"height":140}
    {"op":"view.add","view":"Payments","target":"Acquirer","into":"ref:g1"}
    {"op":"view.note","view":"Payments","text":"Sandbox only until go-live","ref":"n1","into":"ref:g1","x":0,"y":0,"width":300,"height":60}
    {"op":"view.style","view":"Payments","target":"ref:g1","fill":"#eef5fc","line":"#a4b4c5","line_width":"2","font_size":"20","font_face":"Arial","font_style":"bold","font":"","font_color":"#183047","text_align":"left","text_position":"top","border":"rectangle","alpha":"255","line_alpha":"255","label":"${name}\nap-south-1","icon":"hide"}
    {"op":"view.connect","view":"Payments","source":"ref:fc2","target":"Acquirer","relationship":"id:abc","ref":"c1"}
    {"op":"view.route","view":"Payments","target":"ref:c1","points":[[640,200],[900,200]]}
    {"op":"view.nest","view":"Payments","target":"Acquirer","into":"Checkout"}
    {"op":"view.sync","view":"Payments"}
    {"op":"view.doc","view":"Payments","text":"What this drawing is for. Empty clears it."}
    {"op":"view.auto","name":"Around X","from":"ref:x","depth":2,"direction":"both","layout":"auto","viewpoint":"application_cooperation","folder":"/Views/Payments","replace":true}
    {"op":"view.layout","view":"Payments","algorithm":"auto","relayout_all":true}
    {"op":"view.rename","view":"Payments","name":"Payments and Checkout"}
    {"op":"view.move","view":"Payments","folder":"/Views/Programme"}
    {"op":"view.viewpoint","view":"Payments","viewpoint":"application_cooperation"}
    {"op":"view.delete","view":"Old Sketch"}

A view built member by member — create it, add each element, lay it out —
is a dozen or a hundred lines that would otherwise be a dozen or a hundred
`amcli` invocations, each parsing and writing the whole file, and any one
of them able to fail and leave the view half drawn. In a batch they land
with the concept edits they belong to, once, or not at all; `--dry-run`
covers them; and with `replace` on the create and a seed set, re-running
the batch is a no-op in git. `view.rename` is the exception: like a second
`view rename` at the prompt it fails on the re-run, so keep it out of a
batch meant to be re-run.

**Nesting.** `view.add` with `into` draws the box inside another object:
a concept on the view, a group's name, an object id, or the `ref:` a
`view.group` line bound. The container grows to hold it, and no line is
drawn between the two, because a box inside a box is how Archi shows the
relationship between them — every relationship type, by its default
preferences. `view.group` puts a titled box that stands for no concept on
the view and binds its `ref` so objects can be nested in it; `view.note` puts
free text there. `view.nest` moves an object already on the view inside
another (or, with no `into`, back to the top), keeping its place on the
canvas and removing any line between the two, exactly as dragging a box
into another does in Archi. `view.sync` draws every relationship the model
holds between two members of the view that the view does not show yet,
skipping what the nesting already says. `view.layout` lays each container's
children out inside it and sizes the container to what it holds.

**Posters.** A drawing laid out by hand — regions as groups, boxes with
their own captions and colours, numbered flows routed around things — is
a batch too. `view.add` takes `x`, `y`, `width`, `height` and, for a
concept the view already shows, `"again":true` for a second box; every
`view.add`, `view.group` and `view.note` may bind a `ref` so later lines
can name that object. `view.style` sets what a person sets in Archi's
properties on an object or a line: `fill`, `line`, `line_width`, the font
as `font_size` / `font_face` / `font_style` (normal, bold, italic,
bold-italic) or whole as `font` (Archi's `1|Arial|19.0|1|COCOA|1|`),
`font_color`, `text_align` (left, center, right), `text_position` (top,
center, bottom; on a line source, middle, target), `border` (a group:
tabbed, rectangle; a note: dogear, rectangle, none), `alpha` and
`line_alpha` (0–255), `label` (a label expression: `${name}`,
`${documentation}`, `${type}`, `${property:KEY}` expand, anything else is
literal, `\n` breaks a line) and `icon` (show, hide); Archi's own codes
are accepted where the words are, and `""` clears a setting. Its target is
an object id, a group's name, a concept on the view, a `ref:`, a
connection id, or `rel:<selector>` for every line drawing that
relationship. `view.connect` draws one relationship between two objects —
the one line a poster wants where `view.sync` would draw them all — and
binds a `ref` for the line; `view.route` bends a line through absolute
canvas points (an empty list straightens it). `export views` writes a view
that carries any style this way — bounds on every object, one
`view.connect` per line, `view.style` and `view.route` where they apply,
and no `view.layout` — so a poster rebuilds as it was drawn; a plain view
is still its members and a layout.

`view.add` of a concept already on the view adds nothing — the row says
`added false` — but still draws any relationship that concept can newly
draw to what is there. That is what lets a rebuild batch be re-run without
leaving a second box for one element, and what lets a view catch up with
the model.

`view.create` with `replace` redraws the view and keeps what it said about
itself: the viewpoint unless the line sets one (`""` clears it), the
documentation unless a later `view.doc` line replaces it, and the
properties. `amcli export views` writes the `view.doc` line for every
documented view and the `viewpoint` on every `view.create`, so the batch it
produces rebuilds the views with their documentation, and applying it is
byte-identical.

## `ref`

A line names its result; later lines address it as `ref:name`. This is what
makes a batch composable: you cannot know the generated id in advance.

Refs resolve forwards only. A typo fails at the line that used it, rather than
silently deferring the problem.

## `if_absent`

Skip the operation if the thing already exists, and bind the `ref` to the
existing one. This is what makes a batch **re-runnable** — after a half-finished
attempt, or against a second model.

Without it, adding the same relationship twice is refused, because a duplicate
relationship of the same type between the same pair adds nothing to the model.

## `if_present`

The mirror of `if_absent`, on the two ops that delete: do nothing if the target
is not there, instead of failing the batch. A skipped line reports no id and
`removed` 0 — nothing else reports 0, because a delete that happens removes at
least the concept itself.

It is what makes a batch that *replaces* something re-runnable. Swapping an
Association for a Realization is two lines that have to land together, or the
model spends the gap saying something false:

    amcli apply - <<'EOF'
    {"op":"relation.delete","target":"id:8f3c1a02","if_present":true}
    {"op":"relation.add","type":"Realization","source":"Payment API","target":"Payments","if_absent":true}
    EOF

Run that twice and the second run finds the old relationship gone and the new
one already there, deletes nothing, adds nothing, and writes a file identical
to the one it read.

Two misses are never skipped. An ambiguous selector still fails — the thing is
there, and the batch has not said which one — and so does a `ref:`, which names
something an earlier line was supposed to produce, so a miss there is a typo.

`prop.unset` needs none of this: a key that is not set is already what it asks
for, and it says `removed false` when there was nothing to remove.

## Rebuilding a model from its batches

Keeping the batches in the repository and regenerating the model from them is a
good workflow, but by default every rebuild mints fresh random ids, so a model
that is semantically unchanged produces a whole-file diff.

Pass the same seed on every command that writes — `init`, `apply`, `view auto` —
or set it once in the environment:

    export AMCLI_ID_SEED=monetech
    amcli init "Monetech" -o model.archimate
    amcli apply 01-capabilities.jsonl
    amcli apply 02-applications.jsonl

Ids are then a function of what they name: an element's from its type and name, a
relationship's from its type and endpoints, a view's from its name. Rebuild
twice and the files are byte-identical, so the diff shows only what changed.

One seed per model, chosen once. Changing it reissues every id, and two models
sharing a seed will give the same id to two elements that share a type and name.

## Checking before writing

    amcli apply ops.jsonl --dry-run      # reports, writes nothing
    amcli apply ops.jsonl --expect-checksum "$CS"   # exit 6 if the file moved
