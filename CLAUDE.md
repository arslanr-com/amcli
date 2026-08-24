# amcli — notes for agents working on this repo

`amcli` is a CLI over ArchiMate model files. Rust workspace, one static binary,
no runtime dependencies.

## The rule that overrides everything else

**A parse-then-write round trip must be byte-identical.** If a change makes
`tests/corpus` fail the identity test, the change is wrong — not the test. The
whole product proposition is that editing a model produces a diff a human can
review, and that only holds if untouched bytes stay untouched.

Practically: never re-serialize what you did not edit. `amcli-xml` blits
untouched subtrees straight out of the source buffer, and any new code that
touches the document must preserve that property.

## Layout

| Crate | Job |
|---|---|
| `crates/amcli-xml` | Format-preserving XML tree with byte spans. Knows nothing about ArchiMate. |
| `crates/amcli-model` | The ArchiMate IR: types, folders, concepts, views, containers (plain XML / ZIP / grafico). |
| `crates/amcli-view` | View geometry, layout, notation — and `icons.rs`, the type icons. |
| `crates/amcli-render` | A compiled view to SVG, or to PNG through `resvg` (pure Rust, so the binary stays static). |
| `crates/amcli-cli` | The binary; `src/web/` is `amcli web`, the read-only viewer. |
| `xtask` | Codegen from the vendored Archi assets. |

## `skills/amcli/` is shipped, not documentation

`npx skills add arslan-gg/amcli` copies that directory out of the default
branch verbatim, and `crates/amcli-cli/src/skill.rs` embeds the same files with
`include_str!` for the binary-first route. **Both routes must produce identical
bytes** — `both_install_routes_ship_the_same_bytes` walks the directory and
fails if a file is there but not in `FILES`.

Consequences worth knowing before you edit it:

- **Nothing may be generated into it.** A file written only by `skill install`
  makes the two routes differ; a committed generated file goes stale on the
  next release. That is why there is no `references/commands.md` and why
  `amcli skill commands` exists instead.
- **The executable bit does not survive.** The skills CLI has two install
  paths, and its blob fast path writes every file 0644. Invoke scripts as
  `sh scripts/install.sh`, never `./scripts/install.sh`.
- **The skill ships from the branch, the binary from the newest tag**, so the
  skill is normally the *newer* of the two. Never tell an agent to reconcile
  that by reinstalling the skill — it would downgrade itself. The reconciliation
  lives in `parse_or_hint` in `main.rs`.
- **Every release updates the skill, and the version line is only half of it.**
  "written for **amcli X.Y.Z**" in `SKILL.md` is pinned to the workspace
  version by a test, so a release commit bumps `Cargo.toml`, `Cargo.lock` *and*
  that line or `cargo test` is red. The half no test can check is the prose:
  a command or flag the release adds is invisible to an agent until `SKILL.md`
  describes it, because that file — not `--help` — is what it reads. So before
  tagging, diff the release against `amcli skill commands` and ask of each new
  subcommand, flag and batch op: is it in `SKILL.md`, or in
  `references/batch.md` if it only matters inside a batch? Shipping a feature
  nobody is told about is the same as not shipping it.
- **The skill's Setup runs the installer every session.** That is only fine
  because `install.sh` / `install.ps1` short-circuit when the newest release is
  already installed and keep the installed binary when there is no network.
  Keep those two properties if you touch the installers.
- Files beginning with a dot are never copied, so no `.gitattributes` or
  `.version` inside the skill folder.

## `amcli web` and `crates/amcli-cli/src/web/`

The viewer is a hand-rolled HTTP/1.1 GET server on `std::net` plus a page of
plain ES modules, all compiled into the binary with `include_str!`. There is
no build step, no bundler and no CDN, and that is a property to keep: the
binary is the whole product, and it works offline.

- **Every file under `src/web/assets/` must be in `ASSETS` in `api.rs`.** A
  test starts the binary and fetches each file on disk; a file that is there
  but not listed is unreachable and fails the test.
- **Server threads never print.** `main` holds the stdout/stderr locks until
  it hands over to the server via `Output::then`, and after that the terminal
  belongs to the person, not to a request log.
- **The URL is printed before the server serves.** That is the whole
  contract for an agent: read one line, keep the process running.
- **Nothing on the page writes.** The only HTTP verb is GET; the viewer is
  read-only by construction, not by policy.
- **The page draws with the same notation as the renderer.** `/api/model`
  carries the fills, figures, icons and line ends from `amcli-view`'s
  `notation.rs` and `icons.rs`, so the graph cannot drift from a rendered view.
- **The graph is laid out by `amcli-view`, not by the browser.** The page
  decides *what* to draw — it has the whole model, so a filter costs nothing —
  and `GET /api/layout?e=<index ranges>` answers *where*, from the same
  `layout::place` that `view auto` runs. There is no second layout engine and
  no force simulation; a graph is a view that was not saved. The one
  difference is [`Lanes::Free`] (below), and the indices in `e` are positions
  in `/api/model`'s arrays, which is why `indexed()` is the only place that
  decides that order.
- **`replaceParams` deliberately does not tell the router.** Every page
  redraws the part of itself that changed; notifying would tear the page down
  and rebuild it, losing what the URL does not hold and asking the server
  again for the same answer.
- **Type icons are hand-ported code**, in `crates/amcli-view/src/icons.rs`,
  one entry per Archi figure class named in a comment. They are not
  `assets/archi` inputs and `xtask` does not touch them. Path data must stay
  within the 16×16 box and never contain `-0` — the render byte-stability
  test forbids it. The *chrome's* icons are a separate set, in
  `assets/icons.js`, and no interface element may be a typed character:
  `the_chrome_has_no_text_icons` fails on `▣ ▶ ▼ ✕ ↗ ⤡ ◐ ‹ ↔ ▾`.

### The design system, and the five tests that hold it up

The page came apart once — nine font sizes, twenty spacings, four radius
idioms, thirty-seven inline styles, three copies of the sortable table header
with three different sort defaults, and the same layer named two ways on two
pages. Care at the call site is what failed, so the rules are tests now. Read
`docs/web-ux.md` for the diagnosis; these are the standing constraints:

- **`tokens.css` is the only file allowed to name a colour or a length.**
  `tokens_are_the_only_literals` fails on a hex, an `rgb()` or any `px` other
  than `0` and `1` anywhere in `app.css`. Add the value to `tokens.css` and
  give it a name instead.
- **A page module may compute a length but never decide one.**
  `page_modules_decide_no_lengths` fails on a quoted `12px` in any asset. A
  bar's width from its value, a tree row's indent from its depth and an
  ArchiMate fill from `/api/model` are all fine; `style: { width: "220px" }`
  is not — that is how one search box came to have three widths.
- **Every foreground on every ground clears WCAG AA.**
  `every_token_pair_clears_wcag_aa` reads the hexes out of `tokens.css` and
  checks the pairs in both themes. The count inside a selected chip used to
  sit at 2.46:1 because `.muted` beat the chip's inverted colour; that is what
  `--invert-subtle` is for.
- **Searching is one matcher**, `assets/fuzzy.js`, and every box on the page
  goes through it. `one_matcher_behind_every_search` fails on a
  `toLowerCase().includes(` anywhere else, which is the shape each of the four
  boxes had written for itself. The matcher ranks in one currency — whole
  name, prefix, substring, then the letters in order — and the fourth tier
  only accepts what a reader could have meant: a match that starts a word and
  then continues, jumps to another word start, or drops a letter or two
  without leaving the word it is in. Without that last rule `data` matched
  thirty-three names on a real model where five contain the word.
- **A widget is written once**, in `assets/ui.js` — `toolbar`, `filterBar`,
  `dataTable`, `tree`, `popover`, `button`, `chip`, `badge`, `card`,
  `barChart`, `emptyState`. A second copy is a primitive that has not been
  extracted yet. Nothing in `ui.js` knows what ArchiMate is; a page hands in
  render functions.

### The shape of the page, and why

- **Three columns, always all three: rail, middle, inspector.** Neither side
  pane opens or closes — each only narrows, so the layout never jumps and
  nothing has to be found again. `--rail-w` and `--inspector-w` are tokens and
  the inspector's width is dragged and remembered.
- **The rail is where you narrow; the middle is what you got.** Navigation,
  the folder tree and the filters all live in the rail, on every page, through
  `railContext()` — which `render()` clears on each route. Nothing filters
  from a band across the top any more.
- **One page-header anatomy**: title · meta · controls · trailing. It neither
  wraps nor scrolls sideways — what does not fit moves into the toolbar's
  overflow menu, because a control scrolled out of sight is a control you do
  not have. **That reflow must move nothing when nothing has changed**:
  moving a node blurs whatever is focused inside it, and the bar is resized by
  its own contents — the count beside the title is narrower at "6 of 272" than
  at "272 of 272" — so a reflow that re-inserted every control unconditionally
  took the filter box away from under the reader after every letter typed. Nothing in the viewer scrolls horizontally; tables are
  `table-layout: fixed` with a `colgroup`, and cells ellipsize.
- **The camera on the graph belongs to the reader.** A redraw they did not ask
  to move must not move: pinning one box adds one node, and refitting the whole
  graph for it threw away the scale and the corner they had navigated to.
  Narrowing — a pin, a filter — passes `keep` to `build`; asking for a
  different neighbourhood — another centre, more hops, the other direction —
  fits, because that is a different picture. The exception is a camera left
  pointing at nothing, since the server lays the whole graph out afresh every
  time and a blank sheet is worse than a moved one.
- **Leaving a page is not asking it to forget.** `kept.js` holds, for the
  visit, three things no page can rebuild from the file: the query each page
  was last left under, which page of a section the reader was on, and where
  each picture's camera was pointing. A nav entry resolves its destination *on
  the click*, never into its href, which would be one filter out of date by the
  second letter typed — and it leads to the page they were on in that section,
  so Views comes back to the drawing they had opened and zoomed into rather
  than to the top of a table they were not reading. Arriving at a section's
  list clears that; the drawing's own back button and the wordmark are the two
  ways to the list. A camera is filed under the picture it
  was pointing at — `view:<id>`, or the graph's centre, hops and direction, the
  three things that make it a different picture — and taken again by
  `pz.resume(box)` on the first drawing after a mount, which refuses a seat
  that no longer meets the drawing for the same reason `keep` does. It is a
  centre and a scale, never a viewBox: the pane is not the same width on the
  way back. **Nothing is recorded, and nothing is rescaled on a resize, until
  the camera has been pointed at something** — `placed` in `panzoom.js`. The
  graph fetches its layout, so the ResizeObserver's first callback arrives
  while the placeholder 100×100 box is still standing, and filing *that* wrote
  a scale of "the pane over a hundred units" under the picture the reader was
  coming back to, which `resume` then sat in: a hard zoom into the corner. A
  drawing never showed it, because there `resume` runs in the same breath as
  the SVG being inserted.
  All three maps live in sessionStorage, because the tab is the boundary and a
  reload is the same reader in the same place — a page's filters come back from
  the URL on their own, and the camera has nowhere else to come from. The
  payload carries the shape it was written with, and a tab holding an older one
  is dropped whole. Nothing here outlives the tab; what is worth keeping longer
  is in localStorage, as the pins and the pane widths are.
- **One tree, not a tree beside a list.** `folderNodes` in `collection.js`
  builds it once and both rails draw it: the Views page narrows its table with
  it, and a drawing navigates with it. Three things it must keep doing —
  (1) a single root that holds the whole collection is not drawn, because "All
  views 86" and, indented under it, "Views 86" is the same list twice, and the
  folders that divide anything were a level further down; Elements, with seven
  top-level folders, is unaffected. (2) Where a spec declares `leaves`, what is
  filed in a folder hangs off it — the drawing page used to carry a folder tree
  above a flat list of the drawings in the chosen folder, which is eighty-six
  names twice and a fold to keep in step between them. (3) A leaf is picked the
  way its page picks things: on the Views list a click selects and a
  double-click opens, because that is what its rows do; on a drawing a click
  opens, because that rail is the way from one drawing to the next.
- **One selection, one place for it.** A single click anywhere — a row, a
  figure, a graph node — sets the current concept and fills the inspector;
  nothing navigates on a single click. `#/element/ID` opens the collection it
  belongs to with it selected, not a second rendering of the same lists.
  Surfaces stay in step through the `amcli:select` event.
- **Every popover is `position: fixed` and placed by `anchorTo`.** Each
  container that holds a trigger clips its own overflow — that is how the old
  search results were sliced 32px short of their right edge, and how a focus
  ring drawn outside a search field came to be a bracket down its right-hand
  side and nothing on the other three edges. A ring inside a clipped rail is
  drawn inward, `outline-offset: calc(-1 * var(--focus-w))`, exactly as a
  full-bleed table row's is; the rail itself is stretched to the bar's height
  so that nothing else in it is clipped top and bottom.
- **The panes fold in the order of what a reader can do without**: the details
  of one concept first, then navigation. The filters go last, because a table
  you cannot narrow is a table you cannot use. The breakpoints are the
  arithmetic of keeping the middle above ~520px.

`assets/archi/` holds MIT-licensed files vendored from `archimatetool/archi`.
They are **generated inputs, not hand-edited** — `assets/archi/PROVENANCE.toml`
records the upstream tag and checksums, and updating them is a deliberate,
reviewable change. `cargo xtask verify` enforces both halves: the assets
against the recorded checksums, and the generated tables against the assets.
The refresh procedure is in the header of PROVENANCE.toml itself.

## Two things about the layout that are easy to undo by accident

- **The crossing counts read one precomputed thing.** `Layered::sides` gathers,
  per row, where each slot's neighbours stand in the rows above and below;
  `pair_crossings` and friends read only that. Sifting scores a group's
  positions with prefix sums (`sift_scores`) instead of walking the row once
  per position. Together these took the whole 272-element model from 91
  seconds to 1.2 with byte-identical output — putting either back makes
  `view auto` unusable on a real model and the web graph impossible. If you
  change them, check identity against a known layout, not just the tests.
- **`Lanes` is the one place a graph differs from a view.** `Reserved` — the
  default, and what every `view` command uses — gives every line crossing a
  row a corridor, so no line is drawn through a box. It is also where the
  width of a dense drawing comes from: on the whole 272-element model, 504
  edges reserving lanes made it 170,328 px wide with a median line of 15,806.
  `Lanes::Free` drops the corridors: 14,184 px wide, median line 1,063, at the
  cost of about seventy per cent more crossings. `/api/layout` uses `Free`
  because nobody saves a graph; do not make it the default anywhere a diagram
  is written to the file.

## Format traps worth knowing before you touch the model layer

- Elements *and* relationships both serialize as `<element xsi:type="archimate:X">`.
  They are told apart by the type, not the tag.
- Five types are renamed on the way out: `DiagramModelArchimateObject` →
  `DiagramObject`, `DiagramModelArchimateConnection` → `Connection`,
  `DiagramModelGroup` → `Group`, `DiagramModelNote` → `Note`, and the root
  `ArchimateModel` → `model`.
- `documentation` and `purpose` are child *elements*, not attributes.
- `AccessRelationship/@accessType`: **0 is Write**, 1 Read, 2 Unspecified,
  3 Read/Write. The obvious guess is wrong.
- `<bounds>` is a child element. `x`/`y` are parent-relative and may be negative;
  `width`/`height` of `-1` mean "the figure's default size" (120×55 for elements).
- Bendpoints are relative deltas from the source and target anchors, not points.
- A `.archimate` file may be a **ZIP** containing `model.xml` plus images. Sniff
  before assuming XML.
- EMF omits attributes whose value equals the schema default, so writing one back
  explicitly breaks byte identity.

## Two process-wide things, and what they cost

- **`ids::set_seed` is a `OnceLock`.** `--id-seed` switches new ids from random to
  derived-from-content, and `new_id` is called from deep inside `edit.rs`, so the
  seed is process-wide rather than threaded through every signature. The
  consequence for tests: within one test binary the first caller wins and every
  other test in that file sees the seed. That is why the seeded test lives in its
  own file (`crates/amcli-model/tests/seeded_ids.rs`) — cargo gives each file its
  own binary. Do not set a seed from a shared test file.
- **`--version` carries the commit**, from `crates/amcli-cli/build.rs`. It uses
  the commit *date*, never the build date: a wall clock would make every rebuild
  of the same source a different binary.

## Testing

```bash
cargo test          # identity, involution and property tests
cargo clippy --all-targets
cargo fmt
```

Property tests have already caught two real bugs that hand-written cases missed.
When you fix a bug they find, add the minimal case to `roundtrip.rs` as a named
regression test rather than relying on the random search to catch it again.
