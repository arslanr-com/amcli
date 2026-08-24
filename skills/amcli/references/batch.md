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
    {"op":"relation.add","type":"Serving","source":"ref:x","target":"Y","access":"rw","doc":"…","ref":"r","if_absent":true}
    {"op":"element.rename","target":"ref:x","name":"New name"}
    {"op":"element.doc","target":"id:abc","text":"…"}
    {"op":"element.delete","target":"id:abc","if_present":true}
    {"op":"relation.delete","target":"id:abc","if_present":true}
    {"op":"prop.set","target":"ref:x","key":"owner","value":"team-a"}
    {"op":"prop.unset","target":"ref:x","key":"owner"}
    {"op":"folder.add","parent":"/Application","name":"Payments"}
    {"op":"folder.delete","path":"/Application/Payments"}

`access` is Access relationships only: `read`, `write`, `rw`, `unspecified`.

In every op but `relation.add`, `target` is the thing operated on. In
`relation.add` — and only there — `source` and `target` are the two ends.

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
    {"op":"view.auto","name":"Around X","from":"ref:x","depth":2,"direction":"both","layout":"auto","viewpoint":"application_cooperation","folder":"/Views/Payments","replace":true}
    {"op":"view.layout","view":"Payments","algorithm":"auto","relayout_all":true}
    {"op":"view.rename","view":"Payments","name":"Payments and Checkout"}
    {"op":"view.move","view":"Payments","folder":"/Views/Programme"}
    {"op":"view.viewpoint","view":"Payments","viewpoint":"application_cooperation"}
    {"op":"view.doc","view":"Payments","text":"What this drawing is for. Empty clears it."}
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
