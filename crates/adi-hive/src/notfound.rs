//! The animated `4XX` fallback page adi-hive serves when a request's `Host` matches
//! no configured route — i.e. you reached the `.adi` front door but no app answers to
//! that name. Fully self-contained (inline CSS + JS, no external requests). Ported
//! from adi-dns's former landing server so the whole `.adi` zone shares one page.

/// The standalone fallback page. Self-contained (inline CSS + JS), no external requests.
pub const PAGE: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>4XX error</title>
<style>
  :root { --bg: #ffffff; --fg: #14181d; --muted: #7b828c; --accent: #d64545; }
  * { box-sizing: border-box; }
  html, body { height: 100%; }
  body {
    margin: 0; min-height: 100vh; display: flex; flex-direction: column;
    align-items: center; justify-content: center; gap: 8px; padding: 40px 24px;
    background: var(--bg); color: var(--fg);
    font: 15px/1.55 -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
    text-align: center;
  }

  /* animated ADI mark (spinning 3D box) */
  .adi-mark {
    width: min(230px, 48vw); height: min(230px, 48vw); display: block;
    color: var(--fg); overflow: visible;
  }
  .adi-mark path, .adi-mark line { stroke-linecap: round; stroke-linejoin: round; }
  .mk-outer { stroke-dasharray: 1; animation: nfDraw .9s ease .15s both; }
  .mk-node { transform-box: fill-box; transform-origin: center; animation: nfPop .5s 1.2s both; }
  .mk-core { transform-box: fill-box; transform-origin: center;
    animation: nfPop .55s 1.75s cubic-bezier(.2,1.5,.35,1) both; }
  .mk-halo { transform-box: fill-box; transform-origin: center;
    animation: nfHalo 2.8s 2.0s ease-in-out infinite both; }
  @keyframes nfDraw { from { stroke-dashoffset: 1; } }
  @keyframes nfPop { from { transform: scale(0); opacity: 0; } }
  @keyframes nfHalo {
    0%, 100% { transform: scale(1); opacity: .5; }
    50% { transform: scale(1.2); opacity: 1; }
  }

  /* big 4XX error — shown immediately, no appear delay */
  .err { margin-top: 22px; }
  .err-code {
    display: block;
    font: 800 clamp(72px, 17vw, 132px)/.92 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    letter-spacing: 2px; color: var(--fg);
  }
  .err-code b { color: var(--accent); animation: nfFlicker 5s 3.0s steps(1, end) infinite; }
  .err-code b:nth-of-type(2) { animation-delay: 3.6s; }
  .err-word {
    display: block; margin-top: 10px;
    font-size: clamp(16px, 3.4vw, 22px); font-weight: 600; letter-spacing: .42em;
    text-transform: uppercase; color: var(--muted); padding-left: .42em;
  }
  @keyframes nfFlicker { 0%, 96%, 100% { opacity: 1; } 97% { opacity: .25; } 98% { opacity: 1; } 99% { opacity: .5; } }
</style>
</head>
<body>
  <svg class="adi-mark" viewBox="0 0 200 200" fill="none" role="img" aria-label="adi">
    <!-- STATIC outer hexagon cage -->
    <path class="mk-outer" pathLength="1" d="M98.25 2.74219C99.3329 2.11705 100.667 2.11705 101.75 2.74219L183.353 49.8555C184.435 50.4807 185.103 51.6363 185.103 52.8867V147.113C185.103 148.364 184.435 149.519 183.353 150.145L101.75 197.258C100.667 197.883 99.3329 197.883 98.25 197.258L16.6475 150.145C15.5646 149.519 14.8975 148.364 14.8975 147.113V52.8867C14.8975 51.6363 15.5646 50.4807 16.6475 49.8555L98.25 2.74219Z" stroke="currentColor" stroke-width="3"/>
    <!-- rotating inner hexagon + the 3 "cube" edges (outer hex stays put) -->
    <g id="inner">
      <path d="M167.258 98.25C167.883 99.3329 167.883 100.667 167.258 101.75L135.145 157.372C134.519 158.455 133.364 159.122 132.113 159.122L67.8867 159.122C66.6363 159.122 65.4807 158.455 64.8555 157.372L32.7422 101.75C32.117 100.667 32.117 99.3328 32.7422 98.25L64.8555 42.6279C65.4807 41.5451 66.6364 40.8779 67.8867 40.8779L132.113 40.8779C133.364 40.8779 134.519 41.5451 135.145 42.6279L167.258 98.25Z" stroke="currentColor" stroke-width="3"/>
      <!-- 3 edges from the centre to alternating inner corners: the isometric 3D-box look (core disc hides where they meet) -->
      <line x1="100" y1="100" x2="167.9" y2="100"   stroke="currentColor" stroke-width="2"/>
      <line x1="100" y1="100" x2="66.4"  y2="158.2" stroke="currentColor" stroke-width="2"/>
      <line x1="100" y1="100" x2="66.4"  y2="41.8"  stroke="currentColor" stroke-width="2"/>
    </g>
    <!-- spokes: each fixed outer point -> its 3 closest inner corners; opacity fades with distance (JS, per frame) -->
    <line class="spoke" id="s0" stroke="currentColor" stroke-width="2"/>
    <line class="spoke" id="s1" stroke="currentColor" stroke-width="2"/>
    <line class="spoke" id="s2" stroke="currentColor" stroke-width="2"/>
    <line class="spoke" id="s3" stroke="currentColor" stroke-width="2"/>
    <line class="spoke" id="s4" stroke="currentColor" stroke-width="2"/>
    <line class="spoke" id="s5" stroke="currentColor" stroke-width="2"/>
    <line class="spoke" id="s6" stroke="currentColor" stroke-width="2"/>
    <line class="spoke" id="s7" stroke="currentColor" stroke-width="2"/>
    <line class="spoke" id="s8" stroke="currentColor" stroke-width="2"/>
    <!-- STATIC core -->
    <circle class="mk-halo" cx="100" cy="100" r="30" fill="#c96422" fill-opacity="0.35"/>
    <circle class="mk-core" cx="100" cy="100" r="20" fill="#c96422"/>
    <!-- 3 fixed outer connection points: never move, always shown -->
    <circle class="mk-node" cx="100"   cy="2"   r="6" fill="currentColor"/>
    <circle class="mk-node" cx="185.1" cy="149" r="6" fill="currentColor"/>
    <circle class="mk-node" cx="14.9"  cy="149" r="6" fill="currentColor"/>
  </svg>

  <div class="err">
    <span class="err-code">4<b>X</b><b>X</b></span>
    <span class="err-word">error</span>
  </div>

  <script>
  (function () {
    var cx = 100, cy = 100, period = 14000; // ms per full turn of the inner mechanism
    // 3 fixed outer connection points (never move)
    var outer = [[100, 2], [185.1, 149], [14.9, 149]];
    // all 6 inner-hexagon corners are connectable
    var innerBase = [[167.9, 100], [133.6, 158.2], [66.4, 158.2], [32.1, 100], [66.4, 41.8], [133.6, 41.8]];
    var inner = document.getElementById('inner');
    var spokes = [0, 1, 2, 3, 4, 5, 6, 7, 8].map(function (i) { return document.getElementById('s' + i); });

    function place(th) {
      inner.setAttribute('transform', 'rotate(' + (th * 180 / Math.PI) + ' ' + cx + ' ' + cy + ')');
      var cos = Math.cos(th), sin = Math.sin(th);
      // current positions of the 6 rotating inner corners
      var iv = innerBase.map(function (p) {
        return [cx + (p[0] - cx) * cos - (p[1] - cy) * sin, cy + (p[0] - cx) * sin + (p[1] - cy) * cos];
      });
      var s = 0;
      for (var o = 0; o < outer.length; o++) {
        var ox = outer[o][0], oy = outer[o][1];
        // rank the 6 inner corners by distance to this fixed outer point
        var ds = iv.map(function (p) { return Math.sqrt((p[0] - ox) * (p[0] - ox) + (p[1] - oy) * (p[1] - oy)); });
        var order = [0, 1, 2, 3, 4, 5].sort(function (a, b) { return ds[a] - ds[b]; });
        // fade opacity across [closest .. the 4th, which is the drop-out cutoff]
        var dNear = ds[order[0]], dCut = ds[order[3]], span = (dCut - dNear) || 1;
        for (var j = 0; j < 3; j++) {
          var p = iv[order[j]];
          var op = (dCut - ds[order[j]]) / span;      // 1 at the closest, -> 0 as it nears drop-out
          if (op < 0) op = 0; else if (op > 1) op = 1;
          spokes[s].setAttribute('x1', ox); spokes[s].setAttribute('y1', oy);
          spokes[s].setAttribute('x2', p[0]); spokes[s].setAttribute('y2', p[1]);
          spokes[s].setAttribute('opacity', op.toFixed(3));
          s++;
        }
      }
    }

    // ---- "lag" glitch: the spin hitches while the logo and 4XX jitter + flicker together ----
    var mark = document.querySelector('.adi-mark');
    var xx = document.querySelector('.err-code');
    var GC = 4200, GD = 240;                     // a glitch burst every GC ms, lasting GD ms
    var JX = [-3, 3, -2, 4, -1, 2, 0];           // horizontal jitter, px
    var OP = [0.5, 1, 0.3, 0.7, 0.4, 1, 0.85];   // opacity flicker
    function glitch(on, ph) {
      if (!on) { mark.style.transform = ''; mark.style.opacity = ''; xx.style.transform = ''; xx.style.opacity = ''; return; }
      var k = Math.floor(ph / 34) % JX.length;   // step every ~34ms for a stuttery feel
      mark.style.transform = 'translateX(' + JX[k] + 'px)'; mark.style.opacity = OP[k];
      xx.style.transform = 'translateX(' + (-JX[k]) + 'px)'; xx.style.opacity = OP[k];
    }

    var t0 = null, lastTs = null, spinMs = 0;
    function frame(ts) {
      if (t0 === null) { t0 = ts; lastTs = ts; }
      var el = ts - t0, dt = ts - lastTs; lastTs = ts;
      var ph = el % GC, on = ph < GD;
      if (!on) spinMs += dt;                      // rotation advances only between glitches -> it lags
      place((spinMs / period) * 2 * Math.PI);
      glitch(on, ph);
      requestAnimationFrame(frame);
    }
    requestAnimationFrame(frame);
  })();
  </script>
</body>
</html>
"##;

#[cfg(test)]
mod tests {
    use super::PAGE;

    #[test]
    fn page_is_a_self_contained_document() {
        let page = PAGE;
        assert!(page.starts_with("<!doctype html>"), "is a full document");
        assert!(
            page.contains("class=\"adi-mark\""),
            "includes the animated mark"
        );
        assert!(page.contains("err-code"), "includes the 4XX headline");
        assert!(page.contains(">error<"), "includes the 'error' word");
        // fully self-contained: no external asset references
        assert!(!page.contains("http://"), "no external http refs");
        assert!(!page.contains("https://"), "no external https refs");
    }
}
