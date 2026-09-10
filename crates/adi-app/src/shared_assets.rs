//! Point the served shell at the shared-assets CDN: an immutable-per-version prefix on Cloudflare
//! R2 holding every browser asset Trunk built (`apps/shared-assets` publishes it — see that
//! crate's directory for the URL scheme and the publisher). The setting itself
//! (`GET`/`POST /api/settings/shared-assets`) lives in [`adi_webapp_api::handlers::shared_assets`];
//! this module does the actual rewriting, because that operates on the built `index.html`, which
//! this crate already owns serving (see [`crate::serve_embedded`]).
//!
//! Same URL for every instance on the same version is the whole point — a browser that already
//! loaded the bundle for one instance has it cached for the next — so the prefix carries the
//! version and nothing else: no hostname, no node id, no per-install token.
//!
//! `index.html` itself is never served from anywhere but this instance; only what it points at
//! moves. And it only moves when there is no [`crate::DIST_ENV`] override — a developer serving
//! their own edits out of a local `dist/` gets exactly those edits, never the CDN's cached build.

use adi_webapp_api::handlers;

/// The CDN base URL and version a shell is pointed at, once the setting is confirmed on.
pub struct SharedAssets<'a> {
    base_url: String,
    version: &'a str,
}

impl<'a> SharedAssets<'a> {
    /// The active configuration for this request, or `None` when the setting is off.
    ///
    /// `version` is this build's own [`crate::VERSION`] — every instance on the same release
    /// resolves to the same prefix, and one on an older build never asks for a newer bundle.
    #[must_use]
    pub fn active(version: &'a str) -> Option<Self> {
        handlers::enabled().then(|| Self {
            base_url: handlers::base_url(),
            version,
        })
    }

    /// `rel`'s URL under this version's prefix — `<base>/webapp/<version>/<rel>`, `rel` taken
    /// either root-absolute (as it appears in `index.html`) or bare.
    fn cdn(&self, rel: &str) -> String {
        format!(
            "{}/webapp/{}/{}",
            self.base_url.trim_end_matches('/'),
            self.version,
            rel.trim_start_matches('/'),
        )
    }
}

/// The service worker registration path, which must never be rewritten: a cross-origin service
/// worker registration is refused by the browser outright, and this is the one `.js` reference in
/// the shell that isn't a Trunk-hashed bundle file.
const SW: &str = "/sw.js";

/// Point `html` (the built shell, byte-identical to what this instance would otherwise serve) at
/// `shared`'s CDN for every hashed bundle file it loads — the wasm, its JS glue, both
/// stylesheets, and the `modulepreload`/`preload` hints that name them — each with a same-origin
/// fallback, so a CDN that is offline, down, or simply hasn't been sent this version yet degrades
/// to exactly what an instance with the setting off already serves, rather than a blank page.
#[must_use]
pub fn rewrite(html: &str, shared: &SharedAssets<'_>) -> String {
    let (Some(js), Some(wasm)) = (
        between(html, "from '", "'"),
        between(html, "module_or_path: '", "'"),
    ) else {
        // Trunk's output no longer matches what this expects — serve it unmodified rather than
        // guess; the setting then behaves as if it were off for this one shell.
        return html.to_string();
    };

    let mut out = match span(html, "<script type=\"module\">", "</script>") {
        Some((start, end)) => {
            let mut s = String::with_capacity(html.len() + 512);
            s.push_str(&html[..start]);
            s.push_str(&boot_script(shared, js, wasm));
            s.push_str(&html[end..]);
            s
        }
        None => html.to_string(),
    };

    // Every `modulepreload`/`preload`/`stylesheet` hint naming a hashed bundle file: point it at
    // the same CDN URL the boot script above now asks for, so the preload actually warms it.
    // Scanned off the original `html`, not `out` — the new boot script never contains `href="`.
    for path in href_paths(html) {
        let cdn = shared.cdn(&path);
        let from = format!("href=\"{path}\"");
        let to = if has_ext(&path, "css") {
            // Cross-origin now, so `crossorigin` is what makes the browser run the `integrity`
            // check at all — and unlike the module script, a `<link>` has no surrounding code to
            // wrap in `try`, so the same-origin fallback is its own `onerror` handler instead.
            format!(
                "href=\"{cdn}\" crossorigin=\"anonymous\" \
                 onerror=\"this.onerror=null;this.href='{path}'\""
            )
        } else {
            format!("href=\"{cdn}\"")
        };
        out = out.replace(&from, &to);
    }

    out
}

/// The boot script: a static `import … from` can't be wrapped in `try`, so the CDN attempt
/// becomes a dynamic `import()` that falls back to this instance's own copy of `js`/`wasm` on any
/// failure. `{js:?}`/`{wasm:?}` (and the CDN URLs) rely on `Debug` for `str` producing a
/// JS-safe double-quoted literal — true for the plain, hash-named ASCII paths and URLs these
/// always are.
fn boot_script(shared: &SharedAssets<'_>, js: &str, wasm: &str) -> String {
    let cdn_js = shared.cdn(js);
    let cdn_wasm = shared.cdn(wasm);
    format!(
        "<script type=\"module\">\n\
async function adiBoot(js, wasm) {{\n\
const bindings = await import(js);\n\
const started = await bindings.default({{ module_or_path: wasm }});\n\
window.wasmBindings = bindings;\n\
dispatchEvent(new CustomEvent(\"TrunkApplicationStarted\", {{detail: {{wasm: started}}}}));\n\
}}\n\
try {{\n\
await adiBoot({cdn_js:?}, {cdn_wasm:?});\n\
}} catch (e) {{\n\
console.warn(\"adi: shared-assets bundle failed to load, falling back to the embedded copy\", e);\n\
await adiBoot({js:?}, {wasm:?});\n\
}}\n\
</script>"
    )
}

/// Every `href="/…"` in `html` naming a hashed bundle file (`.js`, `.mjs`, `.wasm`, `.css`) —
/// the `modulepreload`/`preload` hints plus the two stylesheet links. Excludes [`SW`], the one
/// same-origin-only `.js` reference among them.
fn href_paths(html: &str) -> Vec<String> {
    const MARKER: &str = "href=\"";
    let mut paths = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find(MARKER) {
        let after = &rest[i + MARKER.len()..];
        let Some(end) = after.find('"') else { break };
        let path = &after[..end];
        if path.starts_with('/')
            && path != SW
            && ["js", "mjs", "wasm", "css"]
                .into_iter()
                .any(|ext| has_ext(path, ext))
        {
            paths.push(path.to_string());
        }
        rest = &after[end..];
    }
    paths
}

/// Whether `path`'s extension is `ext`, case-insensitively — Trunk's own output is always
/// lowercase, but a hand-fed shell shouldn't silently mismatch on case alone.
fn has_ext(path: &str, ext: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

/// The text strictly between the first `before` and the following `after`.
fn between<'a>(html: &'a str, before: &str, after: &str) -> Option<&'a str> {
    let start = html.find(before)? + before.len();
    let rest = &html[start..];
    let end = rest.find(after)?;
    Some(&rest[..end])
}

/// The byte range `[start, end)` of the first `open` through the `close` that follows it,
/// `close` included.
fn span(html: &str, open: &str, close: &str) -> Option<(usize, usize)> {
    let start = html.find(open)?;
    let close_at = html[start..].find(close)? + start;
    Some((start, close_at + close.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // A trimmed but structurally faithful copy of Trunk's real output (`crates/adi-webapp/dist`
    // is a gitignored build artifact — `trunk build` may never have run here — so this is a fixed
    // fixture rather than an `include_str!` of it). Every anchor `rewrite` parses is present:
    // the boot script's static import and `module_or_path`, both stylesheet links with SRI, the
    // `modulepreload`/`preload` hints, the manifest/icon links, and the `sw.js` registration.
    const SHELL: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<link rel="manifest" href="/manifest.webmanifest">
<link rel="icon" href="/assets/favicon.png" sizes="32x32" type="image/png">
<link rel="apple-touch-icon" href="/assets/apple-touch-icon.png">

<script type="module">
import init, * as bindings from '/adi-webapp-63ff10a37a11d15e.js';
const wasm = await init({ module_or_path: '/adi-webapp-63ff10a37a11d15e_bg.wasm' });


window.wasmBindings = bindings;


dispatchEvent(new CustomEvent("TrunkApplicationStarted", {detail: {wasm}}));

</script>
<link rel="stylesheet" href="/tailwind-49e025338a7a490f.css" integrity="sha384-99ucztOPuE/VvErlWUfQ4CMVTpBPzL5/J9wpPpnAJSqXGjH1cm6EBoSkgpgu8dPy"/>
<link rel="stylesheet" href="/main-7fcffd4507da8448.css" integrity="sha384-46wcLA4OUYGOPHYSK+pAjrsv2wfStuCT6sXEiunqkNfrfEbz6QqpYCHN2HzBUWNB"/>

<script>
if ("serviceWorker" in navigator) {
  navigator.serviceWorker.register("/sw.js", { scope: "/" });
}
</script>
<link rel="modulepreload" href="/adi-webapp-63ff10a37a11d15e.js" crossorigin="anonymous" integrity="sha384-ZZOHLJZAPd314E+IWSQCx61Cab7CmvnmEY2MMRzqaO/ya+i+4UtNC98dyeFLtw8o"><link rel="modulepreload" href="/snippets/adi-webapp-723bd312eb4d940f/inline0.js" crossorigin="anonymous" integrity="sha384-Q7UViCZTZ572JQm/ZARZ5X28mGEi2/FY4fJqGxa8mQqBMll+6FYsvWSm/dbiMq75"><link rel="preload" href="/adi-webapp-63ff10a37a11d15e_bg.wasm" crossorigin="anonymous" integrity="sha384-J7Ixkft4GC3CJnWDy57G5P1JHnehwfjEwPpWDGqfaRRGyFW3SA/BcmtmpClCjeh6" as="fetch" type="application/wasm"></head>
<body>
</body>
</html>
"#;

    fn shared() -> SharedAssets<'static> {
        SharedAssets {
            base_url: "https://cdn.withadi.dev".to_string(),
            version: "1.2.3",
        }
    }

    /// Whether the real dist output this repo ships matches every anchor this module leans on —
    /// if Trunk's template ever changes shape, this fails loudly instead of `rewrite` silently
    /// falling back to serving the shell unmodified.
    #[test]
    fn the_checked_in_shell_still_has_every_anchor_this_module_parses() {
        assert!(SHELL.contains("from '"), "the boot script's static import");
        assert!(SHELL.contains("module_or_path: '"), "the wasm init call");
        assert!(SHELL.contains("<script type=\"module\">") && SHELL.contains("</script>"));
        assert!(SHELL.contains("rel=\"stylesheet\""));
    }

    #[test]
    fn every_hashed_reference_moves_to_the_cdn_under_the_version_prefix() {
        let out = rewrite(SHELL, &shared());
        assert!(
            !out.contains("from '/"),
            "the boot script's import must no longer be root-relative"
        );
        assert!(out.contains("https://cdn.withadi.dev/webapp/1.2.3/"));
        // The two stylesheet links, by content: both css files referenced in the checked-in
        // shell must have moved.
        for css in href_paths(SHELL)
            .into_iter()
            .filter(|p| p.ends_with(".css"))
        {
            assert!(
                out.contains(&format!("https://cdn.withadi.dev/webapp/1.2.3{css}")),
                "{css} did not move to the CDN"
            );
        }
    }

    #[test]
    fn the_service_worker_registration_never_moves() {
        let out = rewrite(SHELL, &shared());
        assert!(
            out.contains("register(\"/sw.js\""),
            "a cross-origin service worker registration is refused by the browser"
        );
    }

    #[test]
    fn the_manifest_and_icons_stay_same_origin() {
        let out = rewrite(SHELL, &shared());
        assert!(out.contains("href=\"/manifest.webmanifest\""));
        assert!(out.contains("href=\"/assets/favicon.png\""));
    }

    #[test]
    fn the_boot_script_falls_back_to_the_local_paths_it_replaced() {
        let js = between(SHELL, "from '", "'").expect("js path");
        let wasm = between(SHELL, "module_or_path: '", "'").expect("wasm path");
        let out = rewrite(SHELL, &shared());
        assert!(
            out.contains(&format!("{js:?}")),
            "local js path kept as the fallback"
        );
        assert!(
            out.contains(&format!("{wasm:?}")),
            "local wasm path kept as the fallback"
        );
        assert!(out.contains("catch (e)"));
    }

    #[test]
    fn stylesheet_links_get_a_same_origin_onerror_fallback() {
        let out = rewrite(SHELL, &shared());
        for css in href_paths(SHELL)
            .into_iter()
            .filter(|p| p.ends_with(".css"))
        {
            assert!(
                out.contains(&format!("onerror=\"this.onerror=null;this.href='{css}'\"")),
                "{css} has no same-origin fallback"
            );
            assert!(out.contains("crossorigin=\"anonymous\""));
        }
    }

    /// A shell that doesn't match the anchors this parses (a future Trunk output, or a hand-fed
    /// fixture in a test) is served unmodified rather than partially rewritten.
    #[test]
    fn an_unrecognized_shell_is_returned_unchanged() {
        let html = "<html><body>not a trunk shell</body></html>";
        assert_eq!(rewrite(html, &shared()), html);
    }

    #[test]
    fn cdn_urls_join_cleanly_regardless_of_a_trailing_slash_on_the_base() {
        let with_slash = SharedAssets {
            base_url: "https://cdn.withadi.dev/".to_string(),
            version: "1.2.3",
        };
        assert_eq!(
            with_slash.cdn("/main.css"),
            "https://cdn.withadi.dev/webapp/1.2.3/main.css"
        );
    }
}
