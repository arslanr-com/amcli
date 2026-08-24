// What the reader left behind, kept for the visit.
//
// Two things a page cannot rebuild from the file: how it was narrowed, and
// where its camera was pointing. Both are the reader's own work, and stepping
// to another page used to throw both away — a look at the graph and back, and
// a table narrowed to six rows was two hundred and seventy-two again from the
// top, a drawing zoomed into one corner was the whole sheet again.
//
// A page's filters are in the URL already, which is why a deep link works and
// why "back to the list" lands on the rows it was opened from. What has no URL
// to come from is the nav: `#/elements` written fresh carries nothing. So every
// page records the query it was last left under, and the nav asks here at the
// moment it is clicked — not when the link was built, because a href baked at
// build time is one filter out of date by the second letter typed.
//
// A section is more than its list, so it also records *which* of its pages the
// reader was on. Views holds a list and eighty-six drawings, and someone who
// opened one, zoomed into a corner of it and stepped away to look something up
// was reading that drawing — the top of the table is not where they were. The
// list stays one click away, on the drawing's own back button and under the
// wordmark.
//
// The camera stays out of the URL on purpose: it moves on every wheel notch,
// and a hash rewriting itself through a pan would bury the model under history
// entries. It is filed under the picture it was pointing at — a drawing's id,
// or the graph's centre, hops and direction — so coming back to the same
// picture comes back to the same corner of it, while asking for a different
// one is fitted afresh.
//
// The tab is the boundary, so the tab's own storage is where all three go. A
// reload is the same reader in the same place — and a reload is exactly where
// the loss showed: a page's filters are in the URL and come back on their own,
// while the camera is not, so F5 on a drawing zoomed into one corner used to
// hand back the whole sheet. A new tab still starts blank, which is what a new
// tab is, and nothing here outlives the tab; what is worth keeping longer is
// in localStorage, as the pins, the pane widths and the theme are. Storage can
// be denied outright, and then the three maps simply last the visit in memory.

const KEY = "amcli-kept";
// The payload's shape is this file's; a tab still holding an older one — or one
// written by a version whose camera meant something else — is dropped whole
// rather than half-read into a place the reader never was.
const SHAPE = 2;

const params = new Map(); // page → the query it was last left under
const places = new Map(); // section → the page within it the reader was on
const cameras = new Map(); // picture → {cx, cy, scale}

try {
  const held = JSON.parse(sessionStorage.getItem(KEY) || "null");
  if (held?.shape !== SHAPE) throw new Error("a shape this file does not know");
  for (const [map, rows] of [[params, held?.params], [places, held?.places], [cameras, held?.cameras]]) {
    for (const [k, v] of rows || []) map.set(k, v);
  }
} catch { /* nothing kept */ }

let queued = null;

function write() {
  queued = null;
  try {
    sessionStorage.setItem(KEY, JSON.stringify({
      shape: SHAPE,
      params: [...params], places: [...places], cameras: [...cameras],
    }));
  } catch { /* not persisted */ }
}

// The camera moves on every wheel notch and every pointer move, so the write
// is coalesced rather than run per frame; `pagehide` flushes the last one,
// which is the one a reload would otherwise lose.
function note() {
  if (queued === null) queued = setTimeout(write, 250);
}
addEventListener("pagehide", () => { if (queued !== null) { clearTimeout(queued); write(); } });

export function keepParams(page, p) {
  params.set(page, p);
  note();
}

export function lastParams(page) {
  return params.get(page) || {};
}

// Null is the section's own list, which is where a section starts and what
// arriving at the list means: it clears a drawing that is no longer where the
// reader is.
export function keepPlace(section, at) {
  places.set(section, at);
  note();
}

export function lastPlace(section) {
  return places.get(section) || null;
}

// A camera is a centre and a scale, never a viewBox: the pane is not the same
// width on the way back — an inspector dragged wider in between — and a
// viewBox replayed into a narrower pane is a different zoom.
export function keepCamera(picture, seat) {
  cameras.set(picture, seat);
  note();
}

export function lastCamera(picture) {
  return cameras.get(picture) || null;
}
