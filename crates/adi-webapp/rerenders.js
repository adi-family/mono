// Rerender highlighting — a debugging overlay that flashes every part of the page the DOM has
// just changed, the way React DevTools' "highlight updates" does for React.
//
// Leptos has no component re-render to hook: a component runs once, and what reruns afterwards
// is whichever reactive closure a signal woke, patching the DOM directly. So this watches the DOM
// itself with a MutationObserver and knows nothing about Leptos — which is also what makes it
// honest about the web components under `design/elements`, which patch the page on their own.
//
// It tells the two kinds of change apart, because in a fine-grained app they mean different things:
//
//   * a REBUILD (solid box, faint fill) — nodes were inserted or removed. That is a `move ||`
//     closure throwing away a subtree and drawing it again, and is the one worth asking about
//     when it happens on every poll.
//   * a PATCH (dashed box) — a text node or an attribute was written in place. That is the
//     reactive system working as intended; it is shown so a patch that fires far more often
//     than its value changes is visible too.
//
// The colour is heat: how many times the same element flashed while its last flash was still
// fading. Blue is once, green a few times, yellow often, red continuously; past one, the count is
// written in the box's corner.
//
// Loaded by index.html as a plain script, before the wasm bundle, so that when it was left on it
// is already watching when the app mounts. `window.__adiRerenders` is the bridge the Rust side
// (src/rerenders.rs) drives from the ⌘K menu; it works from the console just as well:
//
//   __adiRerenders.toggle()
//
// The switch is kept in localStorage (`adi-rerenders`), per browser: it is a question about how
// this page behaves on this screen, and the store has no business holding it.
(function () {
  "use strict";

  var KEY = "adi-rerenders";
  // How long a flash takes to fade out, in milliseconds. Also the window a second change to the
  // same element has to land in to count as "again" rather than as a fresh first flash.
  var FADE = 900;
  // The most boxes drawn in one frame. A route swap rebuilds a whole page at once, and past a few
  // hundred boxes the overlay costs more than it shows.
  var MAX = 400;
  // Heat, by how many times an element flashed in a row: 1, 2–3, 4–7, 8+.
  var HEAT = ["#3b82f6", "#22c55e", "#eab308", "#ef4444"];

  var bridge = {
    // Whether the overlay is showing.
    on: false,
    // Set by the wasm app; called whenever `on` changes.
    onchange: null,
  };
  window.__adiRerenders = bridge;

  var layer = null; // the fixed, pointer-events:none container holding the canvas and the badge
  var canvas = null;
  var ctx = null;
  var observer = null;
  var frame = 0; // the pending requestAnimationFrame, or 0 when the loop is idle
  // Elements the observer has reported since the last frame, mapped to whether any of the reports
  // was a rebuild. Measured in the frame rather than in the observer callback, so a burst of
  // mutations inside one task costs one layout instead of one per record.
  var pending = new Map();
  // Element -> { rect, at, count, rebuild } for every flash still on screen.
  var flashes = new Map();

  function heat(count) {
    return HEAT[count >= 8 ? 3 : count >= 4 ? 2 : count >= 2 ? 1 : 0];
  }

  function build() {
    layer = document.createElement("div");
    layer.setAttribute("aria-hidden", "true");
    layer.style.cssText =
      "position:fixed;inset:0;pointer-events:none;z-index:2147483647;contain:strict;";
    canvas = document.createElement("canvas");
    canvas.style.cssText = "position:absolute;inset:0;width:100%;height:100%;";
    ctx = canvas.getContext("2d");
    // Says the overlay is on, and how to turn it off: the switch outlives a reload, so somebody
    // who forgot they flipped it needs to be told what the boxes are.
    var badge = document.createElement("div");
    badge.textContent = "Showing rerenders · solid = rebuilt, dashed = patched · ⌘K to hide";
    badge.style.cssText =
      "position:absolute;left:8px;bottom:36px;padding:3px 8px;border-radius:4px;" +
      "background:rgba(16,16,16,.85);color:#e5e5e5;font:11px/1.4 system-ui,sans-serif;" +
      "border:1px solid #3b82f6;";
    layer.appendChild(canvas);
    layer.appendChild(badge);
  }

  function size() {
    var dpr = window.devicePixelRatio || 1;
    canvas.width = Math.round(window.innerWidth * dpr);
    canvas.height = Math.round(window.innerHeight * dpr);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  }

  // Queue an element to flash, and whether it was a rebuild. Two are never the box:
  //
  //   * the page itself (<html>, <body>). A change directly under it is a dialog opening or
  //     closing, or a framework marker going in, and outlining the whole screen for it says
  //     nothing — a dialog that was inserted still flashes as itself.
  //   * anything inside the overlay, whose drawing would feed the observer its own output.
  function note(el, rebuild) {
    if (!el || el.nodeType !== 1 || el === document.body || el === document.documentElement) return;
    if (layer && layer.contains(el)) return;
    pending.set(el, pending.get(el) || rebuild);
  }

  // Elements and text are on screen; the comments Leptos leaves as placeholders for a `<Show>` or
  // a `move ||` block are not, and flashing their parent for them would be flashing nothing.
  function seen(n) {
    return n.nodeType === 1 || n.nodeType === 3;
  }

  function record(r) {
    if (r.type === "characterData") {
      note(r.target.parentElement, false);
    } else if (r.type === "attributes") {
      note(r.target, false);
    } else {
      var added = false;
      for (var i = 0; i < r.addedNodes.length; i++) {
        var n = r.addedNodes[i];
        if (!seen(n)) continue;
        // An inserted element is the part that was drawn again; inserted text is drawn in its
        // parent, which is the box it shows up in.
        note(n.nodeType === 1 ? n : r.target, true);
        added = true;
      }
      // A removal with nothing put back shows up as its parent changing.
      if (!added && Array.prototype.some.call(r.removedNodes, seen)) note(r.target, true);
    }
  }

  function observe(records) {
    for (var i = 0; i < records.length; i++) record(records[i]);
    if (pending.size && !frame) frame = requestAnimationFrame(draw);
  }

  function draw(now) {
    frame = 0;
    pending.forEach(function (rebuild, el) {
      var rect = el.getBoundingClientRect();
      if (!rect.width && !rect.height) return; // detached again, or display:none
      var prev = flashes.get(el);
      var again = prev && now - prev.at < FADE;
      flashes.set(el, {
        rect: rect,
        at: now,
        count: again ? prev.count + 1 : 1,
        rebuild: rebuild || (again && prev.rebuild),
      });
    });
    pending.clear();

    ctx.clearRect(0, 0, window.innerWidth, window.innerHeight);
    var drawn = 0;
    flashes.forEach(function (f, el) {
      var age = now - f.at;
      if (age >= FADE) {
        flashes.delete(el);
        return;
      }
      if (drawn++ >= MAX) return;
      // Re-measured while it is still in the page, so a flash follows its element through a scroll
      // instead of staying where the element used to be.
      if (el.isConnected) f.rect = el.getBoundingClientRect();
      var r = f.rect;
      var colour = heat(f.count);
      ctx.globalAlpha = 1 - age / FADE;
      if (f.rebuild) {
        ctx.fillStyle = colour + "1f"; // ~12% — the tint marks a rebuild without hiding what's under it
        ctx.fillRect(r.left, r.top, r.width, r.height);
        ctx.setLineDash([]);
        ctx.lineWidth = 2;
      } else {
        ctx.setLineDash([4, 3]);
        ctx.lineWidth = 1;
      }
      ctx.strokeStyle = colour;
      ctx.strokeRect(r.left + 0.5, r.top + 0.5, Math.max(r.width - 1, 0), Math.max(r.height - 1, 0));
      if (f.count > 1) {
        var label = "×" + f.count;
        ctx.font = "10px system-ui, sans-serif";
        var w = ctx.measureText(label).width + 6;
        ctx.fillStyle = colour;
        ctx.fillRect(r.left, r.top, w, 13);
        ctx.fillStyle = "#101010";
        ctx.fillText(label, r.left + 3, r.top + 10);
      }
    });
    ctx.globalAlpha = 1;
    if (flashes.size) frame = requestAnimationFrame(draw);
  }

  function changed() {
    try {
      if (typeof bridge.onchange === "function") bridge.onchange();
    } catch (e) {
      console.warn("adi: rerenders onchange handler failed", e);
    }
  }

  function remember(on) {
    try {
      if (on) window.localStorage.setItem(KEY, "1");
      else window.localStorage.removeItem(KEY);
    } catch (e) {
      // Private mode or a disabled origin: the switch still works, it just won't outlive the tab.
    }
  }

  // Turn the overlay on or off, and remember which.
  bridge.set = function (on) {
    on = !!on;
    if (on === bridge.on) return;
    bridge.on = on;
    remember(on);
    if (on) {
      if (!layer) build();
      // Appended to <html> rather than <body>: it can go in before <body> exists, and it stays
      // out of the element the app mounts into. Appended *before* observing, so its own arrival
      // is not the first thing flashed.
      document.documentElement.appendChild(layer);
      size();
      window.addEventListener("resize", size);
      observer = new MutationObserver(observe);
      observer.observe(document.documentElement, {
        subtree: true,
        childList: true,
        attributes: true,
        characterData: true,
      });
    } else {
      observer.disconnect();
      observer = null;
      window.removeEventListener("resize", size);
      if (frame) cancelAnimationFrame(frame);
      frame = 0;
      pending.clear();
      flashes.clear();
      layer.remove();
    }
    changed();
  };

  bridge.toggle = function () {
    bridge.set(!bridge.on);
  };

  var saved = false;
  try {
    saved = window.localStorage.getItem(KEY) === "1";
  } catch (e) {}
  if (saved) bridge.set(true);
})();
