//! The pages adi-hive serves in place of an upstream: the animated `4XX` fallback for a `Host`
//! that matches no configured route, and the mesh page for a `*.n.adi` host this machine has no
//! way to reach. Fully self-contained (inline CSS + JS, no external requests) — a front door that
//! fetched an asset to explain that nothing is reachable would be explaining it to nobody.
//!
//! They are deliberately three different pages. "Nothing serves this name", "the app is here but
//! its port is dead" and "this name is a *remote machine* you have not paired with" call for three
//! different next actions, and a single generic error would leave the reader guessing which one
//! they are in.

/// The standalone fallback page. Self-contained (inline CSS + JS), no external requests.
pub const PAGE: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>4XX error</title>
<style>
  /* Mirrors the adi design-system tokens; inlined because this page makes no external
     requests. Keep in sync with crates/adi-css/scss/_tokens.scss. */
  :root { --bg: #fafafb; --fg: #0d0f12; --muted: #6b7280; --accent: #dc2626; }
  @media (prefers-color-scheme: dark) {
    :root { --bg: #0a0b0d; --fg: #e9ecf1; --muted: #8b919c; --accent: #f87171; }
  }
  * { box-sizing: border-box; }
  html, body { height: 100%; }
  body {
    margin: 0; min-height: 100vh; display: flex; flex-direction: column;
    align-items: center; justify-content: center; gap: 8px; padding: 40px 24px;
    background: var(--bg); color: var(--fg);
    letter-spacing: -.006em; -webkit-font-smoothing: antialiased;
    font: 13.5px/1.45 ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
    text-align: center;
  }

  .adi-mark {
    width: min(168px, 38vw); height: min(168px, 38vw); display: block;
    color: var(--fg); overflow: visible;
  }
  /* The translucent build: the lobes mix where they overlap instead of stacking, which is
     richer than flat fills at this size and muddy below about 96px -- so it belongs here and
     not in the icon set. Painted back to front, weak to strong; that order is the design.

     Each lobe is a base fill plus a gloss pass over it -- a specular across the top and a
     shade under the bottom. The lighting is always laid *over* the lobe and never a ramp in
     its own alpha: grading the alpha turns the front lobe translucent at its foot and lets
     whatever is behind show through it. Same surface as apps/macos/Sources/Trefoil.swift. */
  .lobe { transform-box: fill-box; transform-origin: center; }
  .lobe .base { fill: currentColor; }
  .lobe .gl   { fill: url(#mkGloss); }
  .l-back  { opacity: .42; animation: nfRise .55s ease .10s both; }
  .l-mid   { opacity: .82; animation: nfRise .55s ease .24s both; }
  .l-front { opacity: .88; animation: nfRise .55s ease .38s both; }
  .l-mid .base { fill: url(#mkAccent); }
  @keyframes nfRise { from { transform: scale(.86); opacity: 0; } }

  .err { margin-top: 18px; }
  .err-code {
    display: block;
    font: 600 clamp(38px, 8vw, 56px)/1 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    letter-spacing: -.02em; color: var(--fg);
  }
  .err-code b { color: var(--accent); animation: nfFlicker 5s 3.0s steps(1, end) infinite; }
  .err-code b:nth-of-type(2) { animation-delay: 3.6s; }
  .err-word {
    display: block; margin-top: 8px;
    font-size: 12px; font-weight: 600; letter-spacing: .06em;
    text-transform: uppercase; color: var(--muted); padding-left: .06em;
  }
  @keyframes nfFlicker { 0%, 96%, 100% { opacity: 1; } 97% { opacity: .25; } 98% { opacity: 1; } 99% { opacity: .5; } }

  /* Hold the mark in its finished state for anyone who has asked for less motion. */
  @media (prefers-reduced-motion: reduce) {
    .lobe, .err-code b { animation: none !important; }
  }
</style>
</head>
<body>
  <!-- Trefoil: three hexagons of radius 56, centres 32 from the middle at 150/30/-90 degrees,
       shifted 8 down so the drawn shape is centred rather than the lobe centres. Generated
       from those numbers, and `trefoil_geometry_matches_the_spec` re-derives them and fails if
       these literals drift. The same geometry is in apps/macos/Sources/Trefoil.swift. -->
  <svg class="adi-mark" viewBox="0 0 200 200" fill="none" role="img" aria-label="adi">
    <defs>
      <linearGradient id="mkAccent" x1="0" y1="0" x2="0" y2="1">
        <stop offset="0" stop-color="#FF8A4A"/>
        <stop offset=".55" stop-color="#FA5019"/>
        <stop offset="1" stop-color="#D8380A"/>
      </linearGradient>
      <!-- Specular then shade, handing over at the same point so there is no band between. -->
      <linearGradient id="mkGloss" x1="0" y1="0" x2="0" y2="1">
        <stop offset="0" stop-color="#ffffff" stop-opacity=".38"/>
        <stop offset=".55" stop-color="#ffffff" stop-opacity="0"/>
        <stop offset=".55" stop-color="#000000" stop-opacity="0"/>
        <stop offset="1" stop-color="#000000" stop-opacity=".28"/>
      </linearGradient>
    </defs>
    <g id="lobes">
      <g class="lobe l-back">
        <path class="base" d="M72.29 68.00 L120.78 96.00 L120.78 152.00 L72.29 180.00 L23.79 152.00 L23.79 96.00 Z"/>
        <path class="gl" d="M72.29 68.00 L120.78 96.00 L120.78 152.00 L72.29 180.00 L23.79 152.00 L23.79 96.00 Z"/>
      </g>
      <g class="lobe l-mid">
        <path class="base" d="M127.71 68.00 L176.21 96.00 L176.21 152.00 L127.71 180.00 L79.22 152.00 L79.22 96.00 Z"/>
        <path class="gl" d="M127.71 68.00 L176.21 96.00 L176.21 152.00 L127.71 180.00 L79.22 152.00 L79.22 96.00 Z"/>
      </g>
      <g class="lobe l-front">
        <path class="base" d="M100.00 20.00 L148.50 48.00 L148.50 104.00 L100.00 132.00 L51.50 104.00 L51.50 48.00 Z"/>
        <path class="gl" d="M100.00 20.00 L148.50 48.00 L148.50 104.00 L100.00 132.00 L51.50 104.00 L51.50 48.00 Z"/>
      </g>
    </g>
  </svg>

  <div class="err">
    <span class="err-code">4<b>X</b><b>X</b></span>
    <span class="err-word">error</span>
  </div>

  <script>
  (function () {
    var period = 14000;                       // ms per full turn of the mechanism
    var lobes = document.getElementById('lobes');

    // The whole trefoil turns about the centre of the box. The lobes keep their paint order as
    // they go, so the mark stays legible at every angle -- rotating the tones instead would put
    // the faintest lobe in front, which is the exact fault this mark was redrawn to fix.
    function place(th) {
      lobes.setAttribute('transform', 'rotate(' + (th * 180 / Math.PI) + ' 100 100)');
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

    // Reduced motion: draw the mechanism once, at rest, and skip the spin and glitch.
    var still = window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    if (still) { place(0); return; }

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

/// The `502` page for a hostname in the reserved `n.adi` zone that this machine cannot reach:
/// either no local mesh gateway is configured, or the one that is refused the connection.
///
/// It exists because the two generic pages would both mislead here. A `404` would say "nothing
/// serves this name", when the name is perfectly good — it just names a machine somewhere else. A
/// bare `502` would say "the upstream is down", sending the reader to look for a local service
/// that was never part of this request. What is actually missing is a *pairing*, and that is the
/// one thing this page says.
///
/// `host` and `node` come straight off the wire (a `Host` header is whatever the client wrote), so
/// both are HTML-escaped before they reach the markup.
#[must_use]
pub fn mesh_unavailable(host: &str, node: Option<&str>) -> String {
    let host = escape(host);
    let node = node.map_or_else(|| "this node".to_string(), escape);
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{host} — node unreachable</title>
<style>
  /* Mirrors the adi design-system tokens; inlined because this page makes no external
     requests. Keep in sync with crates/adi-css/scss/_tokens.scss. */
  :root {{ --bg:#fafafb; --fg:#0d0f12; --muted:#6b7280; --line:#e5e7eb; --accent:#FA5019; }}
  @media (prefers-color-scheme: dark) {{
    :root {{ --bg:#0a0b0d; --fg:#e9ecf1; --muted:#8b919c; --line:#23262b; --accent:#FF7A4D; }}
  }}
  * {{ box-sizing: border-box; }}
  html, body {{ height: 100%; }}
  body {{
    margin:0; min-height:100vh; display:flex; align-items:center; justify-content:center;
    padding:40px 24px; background:var(--bg); color:var(--fg);
    letter-spacing:-.006em; -webkit-font-smoothing:antialiased;
    font:13.5px/1.5 ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
  }}
  .wrap {{ display:flex; flex-direction:column; align-items:center; gap:14px;
    text-align:center; max-width:36rem; }}
  .link {{ width:min(196px, 46vw); height:auto; color:var(--fg); overflow:visible; }}
  .link .near {{ opacity:1; }}
  .link .far {{ opacity:.32; }}
  .link .span {{ stroke-dasharray:6 7; animation:meshDrift 2.6s linear infinite; }}
  @keyframes meshDrift {{ to {{ stroke-dashoffset:-26; }} }}
  @media (prefers-reduced-motion: reduce) {{ .link .span {{ animation:none; }} }}
  .line {{ display:flex; align-items:center; margin-top:6px; }}
  .code {{ font-size:20px; font-weight:600; letter-spacing:-.02em;
    font-variant-numeric:tabular-nums; }}
  .reason {{ margin-left:14px; padding-left:14px; border-left:1px solid var(--line);
    color:var(--muted); }}
  .host {{ margin:0; font:600 15px/1.3 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    color:var(--accent); word-break:break-all; }}
  .msg {{ margin:0; color:var(--muted); }}
  ol {{ margin:6px 0 0; padding:0; list-style:none; display:flex; flex-direction:column; gap:8px;
    text-align:left; color:var(--muted); border-top:1px solid var(--line); padding-top:14px;
    width:100%; }}
  li {{ display:flex; gap:10px; align-items:baseline; }}
  li b {{ color:var(--fg); font-weight:600; }}
  .n {{ flex:0 0 auto; width:18px; height:18px; border:1px solid var(--line); border-radius:50%;
    display:inline-flex; align-items:center; justify-content:center; font-size:10px;
    font-variant-numeric:tabular-nums; color:var(--muted); }}
  code {{ font:12.5px/1 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; color:var(--fg); }}
</style>
</head>
<body>
  <div class="wrap">
    <svg class="link" viewBox="0 0 240 90" fill="none" role="img"
         aria-label="two machines, not connected">
      <g class="near">
        <path d="M24 45 L44 33 L64 45 L44 57 Z" stroke="currentColor" stroke-width="2"
              stroke-linejoin="round"/>
        <circle cx="44" cy="45" r="5" fill="var(--accent)"/>
      </g>
      <line class="span" x1="74" y1="45" x2="166" y2="45" stroke="currentColor" stroke-width="2"
            stroke-linecap="round" opacity=".45"/>
      <g class="far">
        <path d="M176 45 L196 33 L216 45 L196 57 Z" stroke="currentColor" stroke-width="2"
              stroke-linejoin="round"/>
        <circle cx="196" cy="45" r="5" fill="currentColor"/>
      </g>
    </svg>

    <div class="line">
      <span class="code">502</span>
      <span class="reason">node unreachable</span>
    </div>

    <p class="host">{host}</p>
    <p class="msg">
      This hostname addresses a service on <b>{node}</b> — another adi machine, not this one.
      Reaching it needs a local mesh gateway, and none is running here.
    </p>

    <ol>
      <li><span class="n">1</span><span>Start the mesh on this machine, so the front door has a
        gateway to hand <code>*.n.adi</code> to.</span></li>
      <li><span class="n">2</span><span>Pair with <b>{node}</b>. Its administrator authorizes this
        machine's key; a name alone grants nothing.</span></li>
      <li><span class="n">3</span><span>Reload. Every node reachable from here answers under
        <code>&lt;service&gt;.&lt;node&gt;.n.adi</code>.</span></li>
    </ol>
  </div>
</body>
</html>
"#
    )
}

/// Escape the five characters that could break out of the markup. Small and local on purpose:
/// pulling in an HTML-escaping crate for a handful of interpolations would be a dependency the
/// front door pays for on every build.
///
/// Public because the mesh gateway interpolates into the same page shell, and an escaper that
/// exists twice is one that gets fixed once.
#[must_use]
pub fn escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    /// The mark's path data is written into the page as literals, and the same geometry is
    /// drawn independently in `apps/macos/Sources/Trefoil.swift`. Re-derive it from the numbers
    /// both are supposed to be built from, so a drift is a failing test rather than two logos
    /// that quietly stop matching.
    #[test]
    fn trefoil_geometry_matches_the_spec() {
        const RADIUS: f64 = 56.0;
        const OFFSET: f64 = 32.0;
        const NUDGE: f64 = 8.0;
        // back, mid, front — paint order, and the order the paths appear in the page.
        const ANGLES: [f64; 3] = [150.0, 30.0, -90.0];

        for angle in ANGLES {
            let (cx, cy) = (
                100.0 + OFFSET * angle.to_radians().cos(),
                100.0 + OFFSET * angle.to_radians().sin() + NUDGE,
            );
            let corners: Vec<String> = (0..6)
                .map(|i| {
                    let a = (f64::from(i) * 60.0 - 90.0).to_radians();
                    format!("{:.2} {:.2}", cx + RADIUS * a.cos(), cy + RADIUS * a.sin())
                })
                .collect();
            let path = format!("M{} Z", corners.join(" L"));
            assert!(
                PAGE.contains(&path),
                "the page is missing the lobe at {angle}°: expected {path}"
            );
        }
    }

    /// The lobe in front must be the strongest. An earlier mark had it the other way round and
    /// laid a wash of the palest shape over everything, which is invisible on a dark ground and
    /// ruins it on a light one.
    #[test]
    fn the_front_lobe_is_the_strongest() {
        let front = PAGE.find("opacity: .88").expect("front lobe opacity");
        let back = PAGE.find("opacity: .42").expect("back lobe opacity");
        assert!(back < front, "lobes must be declared back to front");
    }

    use super::{PAGE, escape, mesh_unavailable};

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
        assert!(!page.contains("http://"), "no external http refs");
        assert!(!page.contains("https://"), "no external https refs");
    }

    #[test]
    fn the_mesh_page_names_the_node_and_says_what_is_missing() {
        let page = mesh_unavailable("nosh.laptop-b.n.adi", Some("laptop-b"));
        assert!(page.starts_with("<!doctype html>"), "is a full document");
        assert!(page.contains("nosh.laptop-b.n.adi"), "names the hostname");
        assert!(page.contains("laptop-b"), "names the node");
        assert!(page.contains("502"), "carries its status");
        // The three things that make this page different from the 404 and the generic 502.
        assert!(page.contains("mesh gateway"), "says the gateway is missing");
        assert!(page.contains("Pair"), "says the node must be paired");
        assert!(!page.contains("http://"), "no external http refs");
        assert!(!page.contains("https://"), "no external https refs");
    }

    #[test]
    fn the_mesh_page_falls_back_when_the_node_cannot_be_read_from_the_host() {
        // The zone apex names no node; the page still has to say something sensible.
        let page = mesh_unavailable("n.adi", None);
        assert!(page.contains("this node"));
        assert!(page.contains("n.adi"));
    }

    #[test]
    fn the_mesh_page_escapes_the_host_it_was_handed() {
        // A `Host` header is whatever the client wrote — it must never reach the markup raw.
        let page = mesh_unavailable("<script>alert(1)</script>.n.adi", None);
        assert!(!page.contains("<script>alert"), "the host must not be markup");
        assert!(page.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert_eq!(escape("a&b\"c'<d>"), "a&amp;b&quot;c&#39;&lt;d&gt;");
    }
}
