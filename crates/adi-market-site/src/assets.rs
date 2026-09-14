//! Everything the site serves that is not a page: one stylesheet, the faces it names, and the
//! favicon.
//!
//! All of it is compiled into this binary, so generating a site needs nothing on disk but the
//! manifest — the publisher building one is not necessarily standing in this checkout.
//!
//! The stylesheet is three files in one response: `design/tokens.css` (the values), the font sheet
//! (`crates/adi-ui/fonts/fonts.css`, whose `url()`s are relative and therefore resolve against
//! wherever this sheet is served from), then `assets/site.css` (the rules). One request, and a
//! page that cannot restate a token because it never sees a literal.

use crate::File;

/// The design system's values, from the file every adi surface reads.
const TOKENS: &str = include_str!("../../../design/tokens.css");

/// The `@font-face` sheet `scripts/fonts.sh` generates beside the faces themselves.
const FACES: &str = include_str!("../../adi-ui/fonts/fonts.css");

/// This site's own rules.
const SITE: &str = include_str!("../assets/site.css");

/// The one script: which of the two answers to "have you got adi" is in front. Nothing on any page
/// depends on it having run.
const SCRIPT: &str = include_str!("../assets/site.js");

/// The three faces of DESIGN.md §4, one woff2 per script. The list mirrors what
/// `scripts/fonts.sh` writes; `the_sheet_names_exactly_the_faces_that_ship` fails if the two
/// stop agreeing, which is the only way a page would ask for a font that is not there.
const WOFF2: [(&str, &[u8]); 11] = [
    ("geist-latin.woff2", include_bytes!("../../adi-ui/fonts/geist-latin.woff2")),
    ("geist-latin-ext.woff2", include_bytes!("../../adi-ui/fonts/geist-latin-ext.woff2")),
    ("geist-cyrillic.woff2", include_bytes!("../../adi-ui/fonts/geist-cyrillic.woff2")),
    ("geist-cyrillic-ext.woff2", include_bytes!("../../adi-ui/fonts/geist-cyrillic-ext.woff2")),
    ("geist-mono-latin.woff2", include_bytes!("../../adi-ui/fonts/geist-mono-latin.woff2")),
    ("geist-mono-latin-ext.woff2", include_bytes!("../../adi-ui/fonts/geist-mono-latin-ext.woff2")),
    ("geist-mono-cyrillic.woff2", include_bytes!("../../adi-ui/fonts/geist-mono-cyrillic.woff2")),
    (
        "geist-mono-cyrillic-ext.woff2",
        include_bytes!("../../adi-ui/fonts/geist-mono-cyrillic-ext.woff2"),
    ),
    ("geist-mono-symbols2.woff2", include_bytes!("../../adi-ui/fonts/geist-mono-symbols2.woff2")),
    ("bricolage-latin.woff2", include_bytes!("../../adi-ui/fonts/bricolage-latin.woff2")),
    ("bricolage-latin-ext.woff2", include_bytes!("../../adi-ui/fonts/bricolage-latin-ext.woff2")),
];

/// The tab icon: the coloured mark on its tile, which is the build DESIGN.md §10 keeps for an app
/// icon — and a favicon is one. The PNG beside it is for the browsers that still want one.
const FAVICON_SVG: &str = include_str!("../../adi-webapp/assets/mark.svg");
const FAVICON_PNG: &[u8] = include_bytes!("../../adi-webapp/assets/favicon.png");

/// The site's one stylesheet.
#[must_use]
pub fn stylesheet() -> String {
    format!("{TOKENS}\n{FACES}\n{SITE}")
}

/// Every file that is the same on every site this generator builds.
#[must_use]
pub fn files() -> Vec<File> {
    let mut files = vec![
        File::text("site.css", stylesheet()),
        File::text("site.js", SCRIPT.to_string()),
        File::text("favicon.svg", FAVICON_SVG.to_string()),
        File {
            path: "favicon.png".to_string(),
            bytes: FAVICON_PNG.to_vec(),
        },
    ];
    for (name, bytes) in WOFF2 {
        files.push(File {
            path: format!("fonts/{name}"),
            bytes: bytes.to_vec(),
        });
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sheet_names_exactly_the_faces_that_ship() {
        let asked: Vec<&str> = FACES
            .match_indices("url(\"fonts/")
            .map(|(at, _)| {
                let rest = &FACES[at + "url(\"fonts/".len()..];
                &rest[..rest.find('"').expect("a closing quote")]
            })
            .collect();
        assert!(!asked.is_empty(), "the sheet names some faces");
        for face in &asked {
            assert!(
                WOFF2.iter().any(|(name, _)| name == face),
                "the sheet asks for {face}, which this binary does not carry — \
                 add it to WOFF2 (scripts/fonts.sh wrote it)"
            );
        }
        for (name, _) in WOFF2 {
            assert!(asked.contains(&name), "{name} ships but nothing asks for it");
        }
    }

    #[test]
    fn the_stylesheet_carries_the_tokens_and_the_rules() {
        let css = stylesheet();
        assert!(css.contains("--accent:"), "the values");
        assert!(css.contains("@font-face"), "the faces");
        assert!(css.contains(".shelf"), "the rules");
    }

    #[test]
    fn the_fixed_files_are_all_there() {
        let paths: Vec<String> = files().into_iter().map(|f| f.path).collect();
        assert!(paths.contains(&"site.css".to_string()));
        assert!(paths.contains(&"favicon.svg".to_string()));
        assert!(paths.iter().filter(|p| p.starts_with("fonts/")).count() == WOFF2.len());
    }
}
