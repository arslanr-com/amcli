// Pan and zoom by rewriting the SVG viewBox, so whatever the SVG contains —
// a rendered view straight from the server, or a graph we drew — is left
// untouched. Wheel zooms about the cursor; dragging the background pans.

import { keepCamera, lastCamera } from "./kept.js";

export function attachPanZoom(svg, container, opts = {}) {
  const state = { x: 0, y: 0, w: 100, h: 100, content: null };
  let dragging = null;

  // Which picture this camera is pointing at, for `kept.js` to file it under —
  // given as a function where the picture changes under a canvas that stays,
  // which is what recentring the graph does.
  const seat = () => (typeof opts.seat === "function" ? opts.seat() : opts.seat);

  // Until `fit` or `resume` runs, `state` is a placeholder box that has never
  // been pointed at anything, and a camera that is not looking anywhere is not
  // a camera to remember or to rescale. This is not a nicety: the graph fetches
  // its layout, so the ResizeObserver's first callback lands while the
  // placeholder is still standing, and recording *that* filed a scale of "the
  // pane over a hundred units" under the very picture the reader was coming
  // back to — which `resume` then sat in, as a hard zoom into the corner.
  let placed = false;

  const apply = () => {
    svg.setAttribute("viewBox", `${state.x} ${state.y} ${state.w} ${state.h}`);
    // Every move is remembered, because the reader never says when they have
    // finished looking. A pane with no width yet is not a place to remember.
    const cw = container.clientWidth;
    if (opts.seat && placed && cw > 0 && state.w > 0) {
      keepCamera(seat(), { cx: state.x + state.w / 2, cy: state.y + state.h / 2, scale: cw / state.w });
    }
  };

  // Fit `box` ({x,y,w,h}) into the container with a margin.
  //
  // `minFitScale` is the floor: a drawing that is a hundred times wider than
  // the pane would otherwise be shrunk until every box is a fraction of a
  // pixel and the canvas looks empty. Below the floor the fit gives up on
  // showing the whole thing and shows the middle of it at a size that can be
  // seen, leaving the rest to panning.
  const fit = (box, pad = 24) => {
    if (!box || box.w <= 0 || box.h <= 0) return;
    state.content = box;
    placed = true;
    const cw = container.clientWidth || 800, ch = container.clientHeight || 600;
    const scale = Math.max(
      Math.min(cw / (box.w + 2 * pad), ch / (box.h + 2 * pad), opts.maxFitScale || 1.5),
      opts.minFitScale || 0,
    );
    state.w = cw / scale;
    state.h = ch / scale;
    state.x = box.x + box.w / 2 - state.w / 2;
    state.y = box.y + box.h / 2 - state.h / 2;
    apply();
  };

  // Take the seat this picture was last left in — the answer is whether there
  // was one, so a page can fit instead when there was not. A seat pointing at
  // nothing is refused: the drawing behind it may have been laid out afresh or
  // changed on disk while the reader was elsewhere, and a blank sheet is worse
  // than a moved one.
  const resume = (box) => {
    const was = opts.seat ? lastCamera(seat()) : null;
    if (!was || !(was.scale > 0)) return false;
    const cw = container.clientWidth || 800, ch = container.clientHeight || 600;
    const at = { w: cw / was.scale, h: ch / was.scale };
    at.x = was.cx - at.w / 2;
    at.y = was.cy - at.h / 2;
    if (box && !meets(at, box)) return false;
    placed = true;
    Object.assign(state, at);
    if (box) state.content = box;
    apply();
    return true;
  };

  const actual = () => {
    placed = true;
    const cw = container.clientWidth, ch = container.clientHeight;
    const cx = state.x + state.w / 2, cy = state.y + state.h / 2;
    state.w = cw; state.h = ch;
    state.x = cx - cw / 2; state.y = cy - ch / 2;
    apply();
  };

  const zoomAt = (clientX, clientY, factor) => {
    const r = container.getBoundingClientRect();
    const px = state.x + ((clientX - r.left) / r.width) * state.w;
    const py = state.y + ((clientY - r.top) / r.height) * state.h;
    const nw = Math.min(Math.max(state.w * factor, 40), 200000);
    const nh = state.h * (nw / state.w);
    state.x = px - ((clientX - r.left) / r.width) * nw;
    state.y = py - ((clientY - r.top) / r.height) * nh;
    state.w = nw; state.h = nh;
    apply();
  };

  const onWheel = (e) => {
    e.preventDefault();
    const factor = Math.exp((e.deltaMode === 1 ? e.deltaY * 20 : e.deltaY) * 0.0015);
    zoomAt(e.clientX, e.clientY, factor);
  };
  const onDown = (e) => {
    if (e.button !== 0) return;
    if (opts.isNodeTarget && opts.isNodeTarget(e.target)) return;
    e.preventDefault();
    dragging = { sx: e.clientX, sy: e.clientY, x: state.x, y: state.y, moved: false, id: e.pointerId };
    container.classList.add("is-dragging");
  };
  const onMove = (e) => {
    if (!dragging) return;
    const r = container.getBoundingClientRect();
    const dx = ((e.clientX - dragging.sx) / r.width) * state.w;
    const dy = ((e.clientY - dragging.sy) / r.height) * state.h;
    if (!dragging.moved && Math.abs(e.clientX - dragging.sx) + Math.abs(e.clientY - dragging.sy) > 3) {
      dragging.moved = true;
      // Capture only once this is a drag: capturing on the way down would
      // swallow the click a figure is waiting for.
      container.setPointerCapture?.(dragging.id);
      document.body.classList.add("is-dragging");
    }
    if (!dragging.moved) return;
    state.x = dragging.x - dx;
    state.y = dragging.y - dy;
    apply();
  };
  const onUp = () => {
    if (dragging?.moved) container.dataset.justDragged = "1";
    else delete container.dataset.justDragged;
    dragging = null;
    container.classList.remove("is-dragging");
    document.body.classList.remove("is-dragging");
  };
  // When the pane changes size, keep the scale and the centre: a panel
  // opening beside the drawing must not throw away the zoom the reader chose.
  let lastSize = { w: container.clientWidth, h: container.clientHeight };
  const onResize = () => {
    const cw = container.clientWidth, ch = container.clientHeight;
    if (!placed || !cw || !ch || !lastSize.w || !lastSize.h) { lastSize = { w: cw, h: ch }; return; }
    const scale = lastSize.w / state.w;
    const cx = state.x + state.w / 2, cy = state.y + state.h / 2;
    state.w = cw / scale; state.h = ch / scale;
    state.x = cx - state.w / 2; state.y = cy - state.h / 2;
    lastSize = { w: cw, h: ch };
    apply();
  };

  container.addEventListener("wheel", onWheel, { passive: false });
  container.addEventListener("pointerdown", onDown);
  container.addEventListener("pointermove", onMove);
  container.addEventListener("pointerup", onUp);
  container.addEventListener("pointercancel", onUp);
  const ro = new ResizeObserver(onResize);
  ro.observe(container);

  // Client → SVG user coordinates, for the graph's node dragging.
  const toSvg = (clientX, clientY) => {
    const r = container.getBoundingClientRect();
    return { x: state.x + ((clientX - r.left) / r.width) * state.w, y: state.y + ((clientY - r.top) / r.height) * state.h };
  };

  return {
    fit, actual, resume, toSvg, apply,
    zoomIn: () => { const r = container.getBoundingClientRect(); zoomAt(r.left + r.width / 2, r.top + r.height / 2, 0.8); },
    zoomOut: () => { const r = container.getBoundingClientRect(); zoomAt(r.left + r.width / 2, r.top + r.height / 2, 1.25); },
    get viewBox() { return { ...state }; },
    destroy() {
      container.removeEventListener("wheel", onWheel);
      container.removeEventListener("pointerdown", onDown);
      container.removeEventListener("pointermove", onMove);
      container.removeEventListener("pointerup", onUp);
      container.removeEventListener("pointercancel", onUp);
      ro.disconnect();
    },
  };
}

// Do two boxes have any pixel in common? The camera against the drawing: a
// camera that meets nothing is pointing at a blank sheet.
export const meets = (a, b) =>
  a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
