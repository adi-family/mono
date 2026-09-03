//! The pages adi-hive serves in place of an upstream: the `404` for a `Host` that matches no
//! configured route, the mesh page for a `*.n.adi` host this machine has no way to reach, and
//! the [`shell`] every other front-door and gateway page is drawn into. Fully self-contained
//! (inline CSS, no script, no external requests) — a front door that fetched an asset to explain
//! that nothing is reachable would be explaining it to nobody.
//!
//! They are deliberately different pages. "Nothing serves this name", "the app is here but its
//! port is dead" and "this name is a *remote machine* you have not paired with" call for three
//! different next actions, and a single generic error would leave the reader guessing which one
//! they are in.
//!
//! The look is `design/DESIGN.md`: the palette is `design/tokens.css`, inlined at compile time
//! so a page can never restate a colour; the mark is the flat monochrome build of §10; the type
//! is Geist where the reader has it and the token file's fallback where they do not, because
//! these pages cannot load a font.

/// Every colour, face and size the pages use, straight from the file the whole product reads.
const TOKENS: &str = include_str!("../../../design/tokens.css");

/// The mark at bar size — three hexagons, one in front, `currentColor` at 52%, 74% and 100%,
/// with the hairline gaps the `Cut` build keeps (`crates/adi-ui/src/mark.rs`). A macro rather
/// than a `const`, so the literal can also be spliced into [`PAGE`] by `concat!`.
///
/// `trefoil_geometry_matches_the_spec` re-derives every path from the five numbers all copies
/// of the mark are built from, so this drawing cannot drift from the others silently.
macro_rules! mark {
    () => {
        concat!(
            r##"<svg viewBox="0 0 200 200" fill="none" aria-hidden="true"><defs>"##,
            r##"<mask id="mk0" maskUnits="userSpaceOnUse" x="0" y="0" width="200" height="200"><rect width="200" height="200" fill="#fff"/>"##,
            r##"<path fill="#000" d="M127.71 65.00 L178.81 94.50 L178.81 153.50 L127.71 183.00 L76.62 153.50 L76.62 94.50 Z"/>"##,
            r##"<path fill="#000" d="M100.00 17.00 L151.10 46.50 L151.10 105.50 L100.00 135.00 L48.90 105.50 L48.90 46.50 Z"/></mask>"##,
            r##"<mask id="mk1" maskUnits="userSpaceOnUse" x="0" y="0" width="200" height="200"><rect width="200" height="200" fill="#fff"/>"##,
            r##"<path fill="#000" d="M100.00 17.00 L151.10 46.50 L151.10 105.50 L100.00 135.00 L48.90 105.50 L48.90 46.50 Z"/></mask></defs>"##,
            r##"<path fill="currentColor" fill-opacity="0.52" mask="url(#mk0)" d="M72.29 68.00 L120.78 96.00 L120.78 152.00 L72.29 180.00 L23.79 152.00 L23.79 96.00 Z"/>"##,
            r##"<path fill="currentColor" fill-opacity="0.74" mask="url(#mk1)" d="M127.71 68.00 L176.21 96.00 L176.21 152.00 L127.71 180.00 L79.22 152.00 L79.22 96.00 Z"/>"##,
            r##"<path fill="currentColor" d="M100.00 20.00 L148.50 48.00 L148.50 104.00 L100.00 132.00 L51.50 104.00 L51.50 48.00 Z"/></svg>"##,
        )
    };
}

/// The rules on top of the tokens: a bar with the mark and wordmark, then one column of prose
/// at 64ch. Everything is a token; the numbers are the spacing scale.
macro_rules! css {
    () => {
        concat!(
            ":root{color-scheme:dark}",
            "*{box-sizing:border-box}",
            "body{margin:0;min-height:100vh;padding:32px 24px 64px;font-size:var(--fs-body);line-height:1.6}",
            ".brand{display:flex;align-items:center;gap:8px;margin:0 0 48px;font-size:15px;font-weight:600;color:var(--ink)}",
            ".brand svg{width:18px;height:18px;display:block}",
            "main{max-width:64ch}",
            ".label{margin:0 0 12px;font-size:var(--fs-label);color:var(--ink-3)}",
            "h1{margin:0 0 16px;font-size:var(--fs-title);font-weight:600;line-height:1.25;letter-spacing:-.01em;color:var(--ink)}",
            "p{margin:0 0 16px;color:var(--ink-2)}",
            "b{font-weight:500;color:var(--ink)}",
            ".host{margin:0 0 16px;font-family:var(--mono);font-size:13px;color:var(--code);word-break:break-all}",
            ".glyph{display:block;width:24px;height:24px;margin:0 0 16px;color:var(--ink-3)}",
            "ol{margin:24px 0 0;padding:0;list-style:none;border-top:1px solid var(--line)}",
            "li{display:flex;gap:12px;padding:12px 0;border-bottom:1px solid var(--line);color:var(--ink-2)}",
            "li .n{flex:none;width:20px;color:var(--ink-3);font-variant-numeric:tabular-nums}",
            ".facts{margin-top:24px;border-top:1px solid var(--line);font-size:var(--fs-ui-sm)}",
            ".row{display:flex;gap:16px;padding:9px 0;border-bottom:1px solid var(--line)}",
            ".row .k{flex:0 0 128px;color:var(--ink-3)}",
            ".row code{word-break:break-all}",
            "a{color:var(--ink-2);text-decoration:underline;text-underline-offset:3px;text-decoration-color:var(--ink-3)}",
            "a:hover{color:var(--ink)}",
        )
    };
}

/// The standalone `404` — `Host` matched no configured route. A `const`, because it is the same
/// bytes for every request and the hot path writes it without a format.
pub const PAGE: &str = concat!(
    "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n",
    "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n",
    "<meta name=\"color-scheme\" content=\"dark\">\n",
    "<title>Not found</title>\n<style>\n",
    include_str!("../../../design/tokens.css"),
    css!(),
    "\n</style>\n</head>\n<body>\n<div class=\"brand\">",
    mark!(),
    "adi</div>\n<main>\n",
    "<p class=\"label\">404 \u{b7} not found</p>\n",
    "<h1>Nothing is served at this name</h1>\n",
    "<p>No service on this machine answers to the hostname you opened. Check the spelling, or \
     open the control panel at <a href=\"//app.adi/\">app.adi</a> and look under \
     <b>Services</b> for the name it is actually listening on.</p>\n",
    "<p>A service that was here and is now gone shows a different page \u{2014} one that says \
     its port is not answering. This one means the front door has never heard of the name.</p>\n",
    "</main>\n</body>\n</html>\n",
);

/// One document for every page the front door and the mesh gateway serve: the mark and
/// wordmark, then a label (`502 · node unreachable`), a title, and whatever `body` says. `title`,
/// `label` and `heading` are text and are escaped here; `body` is markup the caller built from
/// escaped pieces.
///
/// Public because the mesh gateway (`crates/adi-mesh/src/gateway.rs`) draws its five pages into
/// it, and a shell that exists twice is one that drifts.
#[must_use]
pub fn shell(title: &str, label: &str, heading: &str, body: &str) -> String {
    let title = escape(title);
    let label = escape(label);
    let heading = escape(heading);
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta name=\"color-scheme\" content=\"dark\">\n\
         <title>{title}</title>\n<style>\n{TOKENS}{css}\n</style>\n</head>\n<body>\n\
         <div class=\"brand\">{mark}adi</div>\n<main>\n\
         <p class=\"label\">{label}</p>\n<h1>{heading}</h1>\n{body}\n</main>\n</body>\n</html>\n",
        css = css!(),
        mark = mark!(),
    )
}

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
    // Lucide `unplug` (crates/adi-ui/icons/unplug.svg), at the 24px an empty state gets.
    let body = format!(
        r#"<svg class="glyph" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="m19 5 3-3"/><path d="m2 22 3-3"/><path d="M6.3 20.3a2.4 2.4 0 0 0 3.4 0L12 18l-6-6-2.3 2.3a2.4 2.4 0 0 0 0 3.4Z"/><path d="M7.5 13.5 10 11"/><path d="M10.5 16.5 13 14"/><path d="m12 6 6 6 2.3-2.3a2.4 2.4 0 0 0 0-3.4l-2.6-2.6a2.4 2.4 0 0 0-3.4 0Z"/></svg>
<p class="host">{host}</p>
<p>This hostname addresses a service on <b>{node}</b> — another adi machine, not this one. Reaching it needs a local mesh gateway, and none is running here.</p>
<ol>
<li><span class="n">1</span><span>Start the mesh on this machine, so the front door has a gateway to hand <code>*.n.adi</code> to.</span></li>
<li><span class="n">2</span><span>Pair with <b>{node}</b>. Its administrator authorizes this machine's key; a name alone grants nothing.</span></li>
<li><span class="n">3</span><span>Reload. Every node reachable from here answers under <code>&lt;service&gt;.&lt;node&gt;.n.adi</code>.</span></li>
</ol>"#
    );
    shell(
        &format!("{host} — node unreachable"),
        "502 · node unreachable",
        "That machine is not reachable from here",
        &body,
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
    use super::{PAGE, escape, mesh_unavailable, shell};

    /// The mark's path data is written into the page as literals, and the same geometry is
    /// drawn independently in `apps/macos/Sources/Trefoil.swift` and `crates/adi-ui/src/mark.rs`.
    /// Re-derive it from the numbers all of them are built from, so a drift is a failing test
    /// rather than two logos that quietly stop matching.
    #[test]
    fn trefoil_geometry_matches_the_spec() {
        const RADIUS: f64 = 56.0;
        const OFFSET: f64 = 32.0;
        const NUDGE: f64 = 8.0;
        // A lobe in front cuts a slightly larger hexagon out of what is behind it.
        const CUT_BLEED: f64 = 3.0;
        // back, mid, front — paint order, and the order the paths appear in the page.
        const ANGLES: [f64; 3] = [150.0, 30.0, -90.0];

        let hexagon = |angle: f64, radius: f64| {
            let (cx, cy) = (
                100.0 + OFFSET * angle.to_radians().cos(),
                100.0 + OFFSET * angle.to_radians().sin() + NUDGE,
            );
            let corners: Vec<String> = (0..6)
                .map(|i| {
                    let a = (f64::from(i) * 60.0 - 90.0).to_radians();
                    format!("{:.2} {:.2}", cx + radius * a.cos(), cy + radius * a.sin())
                })
                .collect();
            format!("M{} Z", corners.join(" L"))
        };

        for angle in ANGLES {
            let path = hexagon(angle, RADIUS);
            assert!(
                PAGE.contains(&path),
                "the page is missing the lobe at {angle}°: expected {path}"
            );
        }
        for angle in [ANGLES[1], ANGLES[2]] {
            let cutter = hexagon(angle, RADIUS + CUT_BLEED);
            assert!(PAGE.contains(&cutter), "the cutter at {angle}°: {cutter}");
        }
    }

    /// The lobe in front must be the strongest. An earlier mark had it the other way round and
    /// laid a wash of the palest shape over everything.
    #[test]
    fn the_front_lobe_is_the_strongest() {
        let back = PAGE.find("fill-opacity=\"0.52\"").expect("back lobe");
        let mid = PAGE.find("fill-opacity=\"0.74\"").expect("middle lobe");
        let front = PAGE
            .rfind("<path fill=\"currentColor\" d=")
            .expect("front lobe");
        assert!(
            back < mid && mid < front,
            "lobes must be declared back to front"
        );
    }

    /// `design/DESIGN.md` §8 and §10, checked on the bytes: no gradient, no glow, no motion,
    /// no uppercase label, and no colour the token file does not name.
    #[test]
    fn the_pages_are_flat_and_quiet() {
        for page in [
            PAGE.to_string(),
            mesh_unavailable("nosh.laptop-b.n.adi", Some("laptop-b")),
            shell("t", "l", "h", "<p>b</p>"),
        ] {
            for banned in [
                "Gradient",
                "@keyframes",
                "animation",
                "blur(",
                "box-shadow",
                "uppercase",
            ] {
                assert!(!page.contains(banned), "{banned} in a page");
            }
            assert!(page.contains("--bg-side:"), "the token file is inlined");
            assert!(page.contains("color-scheme: dark") || page.contains("color-scheme:dark"));
        }
    }

    #[test]
    fn page_is_a_self_contained_document() {
        let page = PAGE;
        assert!(page.starts_with("<!doctype html>"), "is a full document");
        assert!(
            page.contains("class=\"brand\""),
            "carries the mark and wordmark"
        );
        assert!(page.contains("404"), "carries its status");
        assert!(!page.contains("<script"), "no script");
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
        assert!(
            !page.contains("<script>alert"),
            "the host must not be markup"
        );
        assert!(page.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert_eq!(escape("a&b\"c'<d>"), "a&amp;b&quot;c&#39;&lt;d&gt;");
    }

    #[test]
    fn the_shell_escapes_its_text_and_keeps_its_body() {
        let page = shell("<t>", "<l>", "<h>", "<p>kept</p>");
        assert!(page.contains("<title>&lt;t&gt;</title>"));
        assert!(page.contains("<p class=\"label\">&lt;l&gt;</p>"));
        assert!(page.contains("<h1>&lt;h&gt;</h1>"));
        assert!(page.contains("<p>kept</p>"));
    }
}
