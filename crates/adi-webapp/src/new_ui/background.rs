//! The wallpaper behind the new UI, and where it is kept.
//!
//! A setting of *this device* — the browser, or the Mac app's web view — and never of the stack
//! it talks to, so it lives in `localStorage` beside the switch in [`super`] and no endpoint
//! stands behind it. That is the operator's rule, not a shortcut: the same panel is reached from
//! several machines, and each keeps its own wallpaper.
//!
//! The one surface in the app where DESIGN.md §8's "no gradients" gives way, by the operator's
//! decision for the new UI. The presets' values are still tokens (`--wall-*` in
//! `design/tokens.css`), so this file names them and never spells one out.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use crate::ui;

/// The choice, minus the image.
const KEY: &str = "adi-new-ui-background";

/// The image, on its own key: it runs to a megabyte or more, and [`KEY`] is rewritten on every
/// tick of a colour picker.
const IMAGE_KEY: &str = "adi-new-ui-background-image";

/// The encodings tried for an uploaded image, largest first, until one fits in storage — the
/// long edge in pixels and the JPEG quality. 2560 covers a laptop screen at 2x without the file
/// outgrowing the ~5 MB a browser gives one origin.
const ENCODINGS: [(f64, f64); 3] = [(2560.0, 0.85), (2560.0, 0.7), (1920.0, 0.7)];

/// The wallpapers offered ready-made.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Preset {
    Night,
    Dusk,
    Ocean,
    Forest,
    Aurora,
    Ember,
    Blush,
    Graphite,
}

impl Preset {
    pub(super) const ALL: [Self; 8] = [
        Self::Night,
        Self::Dusk,
        Self::Ocean,
        Self::Forest,
        Self::Aurora,
        Self::Ember,
        Self::Blush,
        Self::Graphite,
    ];

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Night => "Night",
            Self::Dusk => "Dusk",
            Self::Ocean => "Ocean",
            Self::Forest => "Forest",
            Self::Aurora => "Aurora",
            Self::Ember => "Ember",
            Self::Blush => "Blush",
            Self::Graphite => "Graphite",
        }
    }

    /// The inline style that paints it — on the screen and on its own tile in the sheet.
    pub(super) fn css(self) -> String {
        let token = match self {
            Self::Night => "night",
            Self::Dusk => "dusk",
            Self::Ocean => "ocean",
            Self::Forest => "forest",
            Self::Aurora => "aurora",
            Self::Ember => "ember",
            Self::Blush => "blush",
            Self::Graphite => "graphite",
        };
        format!("background: var(--wall-{token})")
    }
}

/// Which kind of wallpaper is showing.
#[derive(Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Kind {
    #[default]
    Preset,
    Color,
    Gradient,
    Image,
}

/// Everything but the image. Every kind's values are kept, not just the showing one's, so
/// trying a preset and coming back finds the custom colour where it was left.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Choice {
    pub(super) kind: Kind,
    pub(super) preset: Preset,
    pub(super) color: String,
    pub(super) from: String,
    pub(super) to: String,
    /// Degrees, as CSS reads a `linear-gradient` angle: 0 runs bottom to top.
    pub(super) angle: u16,
}

impl Default for Choice {
    fn default() -> Self {
        // Hexes, not tokens: these are where the pickers start, and `<input type="color">` takes
        // nothing but `#rrggbb`. They are the Night and Dusk presets' own colours.
        Self {
            kind: Kind::Preset,
            preset: Preset::Night,
            color: "#23315C".into(),
            from: "#2A1B47".into(),
            to: "#B85A7E".into(),
            angle: 160,
        }
    }
}

/// The wallpaper's live state, shared by the screen and the settings sheet.
#[derive(Clone, Copy)]
pub(super) struct Wallpaper {
    pub(super) choice: RwSignal<Choice>,
    /// The uploaded image as a `data:` URL, whichever kind is showing.
    pub(super) image: RwSignal<Option<String>>,
}

impl Wallpaper {
    /// What this device saved; the default for anything missing or unreadable.
    pub(super) fn load() -> Self {
        let storage = ui::storage();
        let read = |key| {
            storage
                .as_ref()
                .and_then(|s| s.get_item(key).ok().flatten())
        };
        let choice = read(KEY)
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        Self {
            choice: RwSignal::new(choice),
            image: RwSignal::new(read(IMAGE_KEY).filter(|url| is_image_url(url))),
        }
    }

    /// Change the choice and save it.
    pub(super) fn set(self, change: impl FnOnce(&mut Choice)) {
        self.choice.update(change);
        if let (Some(s), Ok(json)) = (
            ui::storage(),
            serde_json::to_string(&self.choice.get_untracked()),
        ) {
            let _ = s.set_item(KEY, &json);
        }
    }

    /// Forget the image and fall back to the preset.
    pub(super) fn remove_image(self) {
        if let Some(s) = ui::storage() {
            let _ = s.remove_item(IMAGE_KEY);
        }
        self.image.set(None);
        if self.choice.get_untracked().kind == Kind::Image {
            self.set(|c| c.kind = Kind::Preset);
        }
    }

    /// Shrink an uploaded file to something storage will hold, keep it, and show it.
    pub(super) async fn keep_image(self, file: web_sys::File) -> Result<(), &'static str> {
        let image = decode(&file)
            .await
            .ok_or("That file couldn't be read as an image.")?;
        let storage = ui::storage().ok_or("This browser won't keep anything for this page.")?;
        for (edge, quality) in ENCODINGS {
            let Some(url) = encode(&image, edge, quality) else {
                return Err("That file couldn't be read as an image.");
            };
            // A full origin answers `set_item` with a QuotaExceededError, which is the signal
            // to try the next, smaller encoding.
            if storage.set_item(IMAGE_KEY, &url).is_ok() {
                self.image.set(Some(url));
                self.set(|c| c.kind = Kind::Image);
                return Ok(());
            }
        }
        Err("That image is too large to keep on this device.")
    }

    /// The inline style that paints the screen. A custom value that fails its check — storage
    /// is editable by anyone with devtools — paints the preset instead.
    pub(super) fn css(self) -> String {
        let c = self.choice.get();
        match c.kind {
            Kind::Color if is_hex(&c.color) => format!("background: {}", c.color),
            Kind::Gradient if is_hex(&c.from) && is_hex(&c.to) => format!(
                "background: linear-gradient({}deg, {}, {})",
                c.angle % 360,
                c.from,
                c.to
            ),
            Kind::Image => match self.image.get() {
                Some(url) => {
                    format!("background: url(\"{url}\") center / cover no-repeat, var(--bg)")
                }
                None => c.preset.css(),
            },
            _ => c.preset.css(),
        }
    }
}

/// `#rrggbb` and nothing else — the only shape `<input type="color">` writes.
pub(super) fn is_hex(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// Only what [`encode`] writes, so a stored value can never close the `url("…")` it is put in.
fn is_image_url(s: &str) -> bool {
    s.starts_with("data:image/") && !s.contains('"')
}

/// Load a file into an image element that has finished decoding.
async fn decode(file: &web_sys::File) -> Option<web_sys::HtmlImageElement> {
    let url = web_sys::Url::create_object_url_with_blob(file).ok()?;
    let image = web_sys::HtmlImageElement::new().ok()?;
    image.set_src(&url);
    let decoded = JsFuture::from(image.decode()).await;
    web_sys::Url::revoke_object_url(&url).ok();
    decoded.ok()?;
    Some(image)
}

/// Draw the image no larger than `edge` on its long side and read it back as a JPEG `data:` URL.
fn encode(image: &web_sys::HtmlImageElement, edge: f64, quality: f64) -> Option<String> {
    let (w, h) = (
        f64::from(image.natural_width()),
        f64::from(image.natural_height()),
    );
    if w == 0.0 || h == 0.0 {
        return None;
    }
    let scale = (edge / w.max(h)).min(1.0);
    let (w, h) = ((w * scale).round(), (h * scale).round());
    let canvas: web_sys::HtmlCanvasElement =
        document().create_element("canvas").ok()?.dyn_into().ok()?;
    canvas.set_width(w as u32);
    canvas.set_height(h as u32);
    let ctx: web_sys::CanvasRenderingContext2d = canvas.get_context("2d").ok()??.dyn_into().ok()?;
    ctx.draw_image_with_html_image_element_and_dw_and_dh(image, 0.0, 0.0, w, h)
        .ok()?;
    canvas
        .to_data_url_with_type_and_encoder_options("image/jpeg", &JsValue::from_f64(quality))
        .ok()
}
