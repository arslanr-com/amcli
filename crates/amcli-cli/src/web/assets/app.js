// The shell: rail, working surface, inspector — plus the routing, the theme,
// the keyboard and the poll that keeps all three in step with the file.
//
// The shell owns *where* things go. What goes in the middle is a page module,
// and what goes in the inspector is always `renderConcept`, whoever selected
// it. There is one current concept and one place it is shown.

import { h, clear, fmt } from "./dom.js";
import { store, load, subscribe, startPolling } from "./store.js";
import { parse, onRoute, href } from "./router.js";
import { icon } from "./icons.js";
import { iconButton, emptyState } from "./ui.js";
import { openPalette, closePalette, paletteIsOpen } from "./palette.js";
import { lastParams, lastPlace, keepPlace } from "./kept.js";
import { renderConcept } from "./pages/detail.js";
import * as collection from "./pages/collection.js";
import * as viewPage from "./pages/view.js";
import * as graph from "./pages/graph.js";

const NAV = [
  { page: "views", label: "Views", iconName: "view", count: (d) => d.views.length },
  { page: "elements", label: "Elements", iconName: "elements", count: (d) => d.elements.length },
  { page: "relations", label: "Relationships", iconName: "relations", count: (d) => d.relations.length },
  { page: "graph", label: "Graph", iconName: "graph" },
];

// Which nav entry a route lights up. A concept's deep link belongs to the
// collection it came from, because that is what the page behind it shows.
const OWNER = { view: "views", element: "elements", relation: "relations" };

const app = document.getElementById("app");
const main = document.getElementById("main");
const inspector = document.getElementById("inspector");
const inspectorBody = document.getElementById("inspector-body");
const inspectorActions = document.getElementById("inspector-actions");
const statusEl = document.getElementById("status");
const railContextEl = document.getElementById("rail-context");

// Where a page puts what narrows it — the folder tree, the filters, the pins.
// One place, on every page, so "how do I see less of this" has one answer.
export function railContext() { return railContextEl; }

let unmount = () => {};
let currentId = null;

/* ---- preferences ----------------------------------------------------------
   Storage can be denied outright; the viewer then simply keeps its defaults
   for the visit rather than failing to start. */
const prefs = {
  get(k, fallback) { try { return localStorage.getItem(k) ?? fallback; } catch { return fallback; } },
  set(k, v) { try { localStorage.setItem(k, v); } catch { /* not persisted */ } },
};

/* ---- how the page opens ----------------------------------------------------
   A window too small for three columns opens with a pane folded — as a state,
   the same one ⌘B and ⌘I toggle, never as a media rule. A media rule folded
   the rail at every width, which left the filters filtering with nothing on
   screen that could have caused it and no way to get it back. The widths are
   `--fold-inspector` and `--fold-rail` in tokens.css; the order is what a
   reader can do without: the details of one concept first, then navigation.
   A remembered choice always wins, and an automatic fold is not remembered. */
const token = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();
const narrowerThan = (name) => {
  const w = token(name);
  return !!w && matchMedia(`(max-width: ${w})`).matches;
};

/* ---- theme ----------------------------------------------------------------
   Until the button is pressed there is no preference to remember, and the page
   simply is what the system is — which is also what tokens.css paints before
   this module runs, so the two never disagree and there is no white flash on
   the way to a dark page. Pressing the button is a choice, and a choice is
   kept. */
const systemDark = matchMedia("(prefers-color-scheme: dark)");
function applyTheme(t, remember = true) {
  document.documentElement.dataset.theme = t;
  if (remember) prefs.set("amcli-theme", t);
  themeBtn?.replaceChildren(icon("theme"));
  themeBtn?.setAttribute("title", t === "dark" ? "Switch to the light theme" : "Switch to the dark theme");
  themeBtn?.setAttribute("aria-label", themeBtn.title);
}
const themeBtn = iconButton("theme", "Switch theme", () =>
  applyTheme(document.documentElement.dataset.theme === "dark" ? "light" : "dark"), { variant: "quiet" });
const themePref = prefs.get("amcli-theme");
applyTheme(themePref || (systemDark.matches ? "dark" : "light"), themePref != null);
systemDark.addEventListener("change", (e) => {
  if (prefs.get("amcli-theme") == null) applyTheme(e.matches ? "dark" : "light", false);
});

/* ---- rail ------------------------------------------------------------------ */
const railToggle = document.getElementById("rail-toggle");
function applyRail(collapsed, remember = true) {
  app.classList.toggle("rail-collapsed", collapsed);
  if (remember) prefs.set("amcli-rail", collapsed ? "1" : "0");
  clear(railToggle).appendChild(icon("rail"));
  railToggle.title = collapsed ? "Expand the sidebar (⌘B)" : "Collapse the sidebar (⌘B)";
  railToggle.setAttribute("aria-label", railToggle.title);
}
railToggle.addEventListener("click", () => applyRail(!app.classList.contains("rail-collapsed")));
const railPref = prefs.get("amcli-rail");
applyRail(railPref == null ? narrowerThan("--fold-rail") : railPref === "1", railPref != null);

// Disabled in the markup and enabled when the model arrives: until then there
// is nothing to search, and the panel counts the model as it is built.
const paletteBtn = document.getElementById("open-palette");
paletteBtn.addEventListener("click", openPalette);

// The wordmark goes home, which is the list of views — the same place an empty
// hash lands on, and the one page that describes the model rather than a
// corner of it.
const brand = document.getElementById("brand");
brand.href = href("views");
brand.title = "Views — the front of the model";

// A nav entry goes back to the section as the reader left it: the page they
// were on in it — Views is a list and eighty-six drawings, and someone who
// opened one and zoomed into a corner of it was reading that drawing, not the
// top of the table — and, on a list, their folder, their search, the layers
// they hid, the centre the graph was on. That is what `kept.js` holds and what
// the bare route in the href does not.
//
// It is resolved on the click rather than written into the href, because a
// href built when the nav was would be one filter out of date by the second
// letter typed into a box, and because the link that is copied, opened in a
// tab or read off the status bar should be the plain page. The two ways back
// to a section whole are the drawing's own back button and the wordmark.
function whereTo(section) {
  // A model that reloaded without that view leaves the id pointing at nothing,
  // and "No such view" is not where anybody asked to go. Every deep page in a
  // section is named after the kind it shows, so this is the whole check.
  const at = lastPlace(section);
  if (at && store.byId.get(at.id)?.kind === at.page) return href(at.page, at.id);
  return href(section, null, lastParams(section));
}

const nav = document.getElementById("nav");
function buildNav() {
  clear(nav);
  for (const n of NAV) {
    nav.appendChild(h("a", {
      href: href(n.page), dataset: { page: n.page }, title: n.label,
      onclick: (e) => {
        if (e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
        e.preventDefault();
        location.hash = whereTo(n.page);
      },
    },
      icon(n.iconName),
      h("span", { class: "nav-label" }, n.label),
      n.count ? h("span", { class: "nav-n" }, fmt(n.count(store.data))) : null));
  }
}

/* ---- inspector -------------------------------------------------------------
   One current concept, one place it is shown. A click anywhere — a figure on a
   drawing, a node on the graph, a row in a table — selects; nothing navigates
   on a single click, so a row no longer has two destinations depending on
   which pixel was hit. */
export function select(id, opts = {}) {
  const found = store.byId.get(id);
  clear(inspectorActions);
  // A reload can delete what is selected. `renderConcept` says so; going on
  // describing a concept the file no longer has would not.
  if (!found) {
    currentId = null;
    renderConcept(inspectorBody, id);
    document.dispatchEvent(new CustomEvent("amcli:select", { detail: { id: null } }));
    return;
  }
  currentId = id;
  // Back to the list this came from, as the reader left it — their folder,
  // their search, their sort, the layers they hid. The bare route would land
  // on the same page having thrown all of that away. A view goes to the Views
  // list, not to its drawing: the drawing has its own button in the details.
  const where = { element: ["elements", "Elements"], relation: ["relations", "Relationships"], view: ["views", "Views"] }[found.kind];
  inspectorActions.append(
    where ? iconButton(where[0] === "views" ? "view" : where[0], `Find this in ${where[1]}`,
      () => { location.hash = href(where[0], null, lastParams(where[0])); }, { variant: "quiet" }) : null,
  );
  renderConcept(inspectorBody, id);
  if (opts.focus !== false) inspectorBody.scrollTop = 0;
  document.dispatchEvent(new CustomEvent("amcli:select", { detail: { id } }));
}

export function clearSelection() {
  currentId = null;
  clear(inspectorActions);
  clear(inspectorBody).appendChild(emptyState({
    iconName: "info", title: "Nothing selected",
    body: "Pick a row, a figure on a drawing or a box on the graph, and it will be described here.",
  }));
  document.dispatchEvent(new CustomEvent("amcli:select", { detail: { id: null } }));
}

function applyInspector(narrow, remember = true) {
  app.classList.toggle("inspector-narrow", narrow);
  if (remember) prefs.set("amcli-inspector-narrow", narrow ? "1" : "0");
  clear(inspectorToggle).appendChild(icon("inspector"));
  inspectorToggle.title = narrow ? "Widen the details panel (⌘I)" : "Narrow the details panel (⌘I)";
  inspectorToggle.setAttribute("aria-label", inspectorToggle.title);
}
const inspectorToggle = document.getElementById("inspector-toggle");
inspectorToggle.addEventListener("click", () => applyInspector(!app.classList.contains("inspector-narrow")));
const inspectorPref = prefs.get("amcli-inspector-narrow");
applyInspector(
  inspectorPref == null ? narrowerThan("--fold-inspector") : inspectorPref === "1",
  inspectorPref != null,
);

export function selectedId() { return currentId; }

// Drag the seam. Both panes, one implementation: a side pane is a side pane,
// and the only thing that differs is which way the pointer has to travel to
// make it wider. The width is remembered, because a reader who widened a pane
// to read something should not have to do it again on the next thing.
function makeResizable({ grip, pane, widthVar, minVar, maxVar, storeKey, growsWith }) {
  const num = (name) => parseInt(getComputedStyle(document.documentElement).getPropertyValue(name), 10) || 0;
  const clamp = (px) => Math.max(num(minVar), Math.min(num(maxVar), Math.round(px)));
  const width = () => pane.getBoundingClientRect().width;

  // A separator you can focus is a range widget, and one that never says where
  // it stands gives the keyboard no feedback at all — including at the two
  // ends, where the arrows stop moving anything.
  const report = (px) => {
    grip.setAttribute("aria-valuenow", String(px));
    grip.setAttribute("aria-valuetext", `${px} pixels`);
  };
  grip.setAttribute("aria-valuemin", String(num(minVar)));
  grip.setAttribute("aria-valuemax", String(num(maxVar)));

  const setWidth = (px) => {
    const w = clamp(px);
    app.style.setProperty(widthVar, `${w}px`);
    prefs.set(storeKey, String(w));
    report(w);
  };
  const stored = parseInt(prefs.get(storeKey, ""), 10);
  if (stored) setWidth(stored); else report(clamp(width()));

  let from = null;
  grip.addEventListener("pointerdown", (e) => {
    from = { x: e.clientX, w: width() };
    grip.setPointerCapture(e.pointerId);
    grip.classList.add("is-held");
    document.body.classList.add("is-resizing");
  });
  grip.addEventListener("pointermove", (e) => {
    if (from) setWidth(from.w + (e.clientX - from.x) * growsWith);
  });
  const stop = () => { from = null; grip.classList.remove("is-held"); document.body.classList.remove("is-resizing"); };
  grip.addEventListener("pointerup", stop);
  grip.addEventListener("pointercancel", stop);
  grip.addEventListener("keydown", (e) => {
    if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
    e.preventDefault();
    const step = (e.shiftKey ? num("--sp-12") : num("--sp-4")) * (e.key === "ArrowRight" ? 1 : -1);
    setWidth(width() + step * growsWith);
  });
}

// `growsWith` is which way the pointer makes the pane wider: the rail grows
// rightwards, the inspector leftwards.
makeResizable({
  grip: document.getElementById("rail-grip"), pane: document.getElementById("rail"),
  widthVar: "--rail-w", minVar: "--rail-min", maxVar: "--rail-max",
  storeKey: "amcli-rail-w", growsWith: 1,
});
makeResizable({
  grip: document.getElementById("inspector-grip"), pane: inspector,
  widthVar: "--inspector-w", minVar: "--inspector-min", maxVar: "--inspector-max",
  storeKey: "amcli-inspector-w", growsWith: -1,
});

/* ---- status ----------------------------------------------------------------- */
function setStatus(kind, text, title) {
  statusEl.className = `status ${kind}`;
  statusEl.querySelector(".status-text").textContent = text;
  statusEl.title = title || "";
}

/* ---- keyboard ----------------------------------------------------------------
   One place, so a shortcut cannot mean two things on two pages. */
document.addEventListener("keydown", (e) => {
  const mod = e.metaKey || e.ctrlKey;
  const typing = /^(INPUT|TEXTAREA|SELECT)$/.test(document.activeElement?.tagName || "");
  if (mod && e.key.toLowerCase() === "k") { e.preventDefault(); paletteIsOpen() ? closePalette() : openPalette(); return; }
  if (mod && e.key.toLowerCase() === "b") { e.preventDefault(); applyRail(!app.classList.contains("rail-collapsed")); return; }
  if (mod && e.key.toLowerCase() === "i") {
    e.preventDefault();
    applyInspector(!app.classList.contains("inspector-narrow"));
    return;
  }
  if (e.key === "Escape" && !paletteIsOpen() && currentId && !typing) { clearSelection(); return; }
  if (e.key === "/" && !typing && !paletteIsOpen()) {
    const box = main.querySelector(".field-input");
    if (box) { e.preventDefault(); box.focus(); box.select?.(); }
  }
});

/* ---- routing ------------------------------------------------------------------ */
function pageFor(route) {
  switch (route.page) {
    case "view": return viewPage;
    case "graph": return graph;
    default: return collection;
  }
}

function render(route) {
  if (!store.data) return;
  unmount();
  clear(main);
  clear(railContextEl);
  const owner = OWNER[route.page] || route.page;
  nav.querySelectorAll("a").forEach((a) => a.classList.toggle("is-current", a.dataset.page === owner));

  // A concept's deep link is the collection it belongs to, with the concept
  // selected — not a second, wider copy of the inspector.
  const deep = (route.page === "element" || route.page === "relation") && route.id;
  const effective = deep ? { ...route, page: owner, id: null } : route;

  // Where the reader now is in this section, for its nav entry to come back
  // to. A list is the section's own start, so it records nothing and clears
  // whatever drawing was there: arriving at the list *is* leaving the drawing.
  keepPlace(owner, effective.page === owner ? null : { page: effective.page, id: effective.id });

  try {
    unmount = pageFor(effective).mount(main, effective) || (() => {});
  } catch (err) {
    console.error(err);
    main.appendChild(emptyState({ iconName: "alert", title: "This page could not be drawn", body: err.message }));
    unmount = () => {};
  }
  if (deep) select(route.id);
}

function refreshShell() {
  const d = store.data;
  document.getElementById("model-name").textContent = d.model.name || "(unnamed model)";
  // The file's name, with the whole path on the hover: a path reversed to
  // put the ellipsis at the front read as `…chpad/demo/bank.archimate/`.
  const path = document.getElementById("model-path");
  path.textContent = d.model.path.split(/[\\/]/).pop();
  path.title = d.model.path;
  document.title = `${d.model.name || "amcli"} — amcli`;
  buildNav();
}

subscribe((event) => {
  if (event === "model") refreshShell();
  if (event === "changed") {
    setStatus("is-live is-changed", "updated", `Reloaded at ${new Date().toLocaleTimeString()}`);
    render(parse());
    if (currentId) select(currentId, { focus: false });
    setTimeout(() => setStatus("is-live", "live", `Watching ${store.data.model.path}`), 2500);
  } else if (event === "status") {
    if (store.error) setStatus("is-error", "file invalid", `The model file no longer parses; showing the last good version.\n${store.error}`);
    else setStatus("is-live", "live", `Watching ${store.data.model.path}`);
  } else if (event === "offline") {
    setStatus("is-error", "server gone", "amcli web is no longer answering — was it stopped?");
  }
});

document.querySelector(".rail-foot-row").append(h("span", { class: "spacer" }), themeBtn);

onRoute(render);

load()
  .then(() => {
    setStatus("is-live", "live", `Watching ${store.data.model.path}`);
    paletteBtn.disabled = false;
    if (!location.hash) location.hash = href("views");
    clearSelection();
    render(parse());
    startPolling(2000);
  })
  .catch((e) => {
    setStatus("is-error", "failed", e.message);
    main.appendChild(emptyState({ iconName: "alert", title: "Could not load the model", body: e.message }));
  });
