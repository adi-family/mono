//! Rewrite embedded-shell asset URLs to the versioned CDN published by `apps/shared-assets`.
//!
//! The prefix contains only the build version so instances share browser cache entries.
//! The shell stays local, and a [`crate::DIST_ENV`] override bypasses CDN rewriting.

use adi_webapp_api::handlers;
use adi_webapp_api::types::SharedAssetsMode;

pub struct SharedAssets<'a> {
    base_url: String,
    version: &'a str,
}

impl<'a> SharedAssets<'a> {
    /// Select CDN settings for this request. `version` must be this build's [`crate::VERSION`]
    /// and `host` the request's Host header.
    #[must_use]
    pub fn active(version: &'a str, host: Option<&str>) -> Option<Self> {
        let ask_cdn = match handlers::mode() {
            SharedAssetsMode::LocalAlways => false,
            SharedAssetsMode::CdnWhenRemote => !crate::origin::looks_local(host),
            SharedAssetsMode::CdnAlways => true,
        };
        ask_cdn.then(|| Self {
            base_url: handlers::base_url(),
            version,
        })
    }

    fn cdn(&self, rel: &str) -> String {
        format!(
            "{}/monoapp/{}/{}",
            self.base_url.trim_end_matches('/'),
            self.version,
            rel.trim_start_matches('/'),
        )
    }
}

/// Service workers must remain same-origin.
const SW: &str = "/sw.js";

/// Rewrite bundle URLs and preload hints, retaining same-origin fallbacks.
#[must_use]
pub fn rewrite(html: &str, shared: &SharedAssets<'_>) -> String {
    let (Some(js), Some(wasm)) = (
        between(html, "from '", "'"),
        between(html, "module_or_path: '", "'"),
    ) else {
        // Unknown Trunk output must remain usable without CDN rewriting.
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

    for path in href_paths(html) {
        let cdn = shared.cdn(&path);
        let from = format!("href=\"{path}\"");
        let to = if has_ext(&path, "css") {
            // Cross-origin SRI needs CORS; stylesheets recover through their own onerror handler.
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

/// Dynamic import allows CDN failures to fall back to local JS and wasm.
/// Debug quoting assumes these asset paths and URLs are plain ASCII.
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

/// Collect root-relative bundle references, excluding the same-origin service worker.
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

fn has_ext(path: &str, ext: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

fn between<'a>(html: &'a str, before: &str, after: &str) -> Option<&'a str> {
    let start = html.find(before)? + before.len();
    let rest = &html[start..];
    let end = rest.find(after)?;
    Some(&rest[..end])
}

/// Return the byte range including both delimiters.
fn span(html: &str, open: &str, close: &str) -> Option<(usize, usize)> {
    let start = html.find(open)?;
    let close_at = html[start..].find(close)? + start;
    Some((start, close_at + close.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixed Trunk fixture: the generated dist directory may not exist in a fresh checkout.
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
        assert!(out.contains("https://cdn.withadi.dev/monoapp/1.2.3/"));
        for css in href_paths(SHELL)
            .into_iter()
            .filter(|p| p.ends_with(".css"))
        {
            assert!(
                out.contains(&format!("https://cdn.withadi.dev/monoapp/1.2.3{css}")),
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
            "https://cdn.withadi.dev/monoapp/1.2.3/main.css"
        );
    }
}
