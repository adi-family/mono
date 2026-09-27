//! The settings window, at `/settings`: a macOS-style window floating over the screen, so the
//! wallpaper it edits stays in view and changes as it is picked. Dragged by its title bar, and
//! put back where it was left — per device, like everything else the new UI remembers.
//!
//! Background is its only section for now.

use adi_ui::{Icon, IconSize, Lucide};
use leptos::{ev, html, prelude::*};
use wasm_bindgen::JsCast;

use crate::ui;

/// Where the window was last dropped, as `x,y` in CSS pixels from the viewport's top left.
const POS_KEY: &str = "adi-new-ui-settings-window";

/// How much of the window must stay on screen, so a drag can never lose it: this much of its
/// width, and all of its title bar.
const KEEP_VISIBLE: f64 = 96.0;
const TITLEBAR: f64 = 40.0;

use super::background::{Appearance, Kind, MAX_BLUR, Preset, Wallpaper, is_hex};

/// The custom kinds, in the order the segmented control shows them.
const CUSTOM: [(Kind, &str); 3] = [
    (Kind::Color, "Colour"),
    (Kind::Gradient, "Gradient"),
    (Kind::Image, "Image"),
];

#[component]
pub(super) fn Window(
    wall: Wallpaper,
    #[prop(into)] close: Callback<()>,
    /// Draw in the light token set, to sit on a light wallpaper.
    #[prop(into)]
    light: Signal<bool>,
) -> impl IntoView {
    // Which custom editor is open. Not the same as what is showing: opening Image before there
    // is one must not swap the wallpaper for nothing.
    let tab = RwSignal::new({
        let kind = wall.choice.get_untracked().kind;
        (kind != Kind::Preset).then_some(kind)
    });

    // Escape closes the window — unless it is closing the ⌘K palette above it, a modal dialog
    // that listens for the same key.
    let keys = window_event_listener(ev::keydown, move |ev| {
        let dialog_open = document()
            .query_selector("[role=dialog][aria-modal=true]")
            .ok()
            .flatten()
            .is_some();
        if ev.key() == "Escape" && !dialog_open {
            close.run(());
        }
    });
    on_cleanup(move || keys.remove());

    let pick_tab = move |kind: Kind| {
        tab.set(Some(kind));
        if kind != Kind::Image || wall.image.get_untracked().is_some() {
            wall.set(|c| c.kind = kind);
        }
    };

    // `None` until it is first dragged: the stylesheet centres it, which stays centred through
    // a resize in a way a stored pixel position would not.
    let pos = RwSignal::new(load_pos());
    // Where in the window the pointer took hold, while a drag is on.
    let grip = StoredValue::new(None::<(f64, f64)>);
    let win: NodeRef<html::Div> = NodeRef::new();

    let on_down = move |ev: ev::PointerEvent| {
        // The traffic lights sit in the title bar; pressing one is a click, not a drag.
        let on_button = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .and_then(|t| t.closest("button").ok().flatten())
            .is_some();
        if on_button || ev.button() != 0 {
            return;
        }
        let (Some(win), Some(bar)) = (
            win.get(),
            ev.current_target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok()),
        ) else {
            return;
        };
        let r = win.get_bounding_client_rect();
        grip.set_value(Some((
            f64::from(ev.client_x()) - r.left(),
            f64::from(ev.client_y()) - r.top(),
        )));
        pos.set(Some((r.left(), r.top())));
        // Captured, so a fast drag that outruns the bar keeps moving the window.
        let _ = bar.set_pointer_capture(ev.pointer_id());
        ev.prevent_default();
    };
    let on_move = move |ev: ev::PointerEvent| {
        let (Some((gx, gy)), Some(win)) = (grip.get_value(), win.get()) else {
            return;
        };
        let x = f64::from(ev.client_x()) - gx;
        let y = f64::from(ev.client_y()) - gy;
        pos.set(Some(clamp(x, y, win.offset_width().into())));
    };
    let on_up = move |_: ev::PointerEvent| {
        if grip.get_value().is_some() {
            grip.set_value(None);
            if let (Some(s), Some((x, y))) = (ui::storage(), pos.get_untracked()) {
                let _ = s.set_item(POS_KEY, &format!("{x:.0},{y:.0}"));
            }
        }
    };

    view! {
        <div
            node_ref=win
            class="adi-new-win"
            class:light=move || light.get()
            class:is-placed=move || pos.get().is_some()
            style=move || {
                pos.get().map(|(x, y)| format!("left: {x:.0}px; top: {y:.0}px")).unwrap_or_default()
            }
            role="dialog"
            aria-label="Settings"
        >
            <header
                class="adi-new-win__bar"
                on:pointerdown=on_down
                on:pointermove=on_move
                on:pointerup=on_up
                on:pointercancel=on_up
            >
                // macOS's three lights. Only close does anything here — a settings window has
                // nothing to minimise or zoom — so the other two are drawn disabled, the way
                // macOS draws them on a window that cannot.
                <div class="adi-new-win__lights">
                    <button
                        class="adi-new-win__light adi-new-win__light--close"
                        type="button"
                        aria-label="Close settings"
                        title="Close"
                        on:click=move |_| close.run(())
                    ></button>
                    <span class="adi-new-win__light" aria-hidden="true"></span>
                    <span class="adi-new-win__light" aria-hidden="true"></span>
                </div>
                <h1 class="adi-new-win__title">"Settings"</h1>
            </header>
            // Only this scrolls: the title bar, and the way out in it, stays put however tall
            // the section below grows.
            <div class="adi-new-win__body">
                <section class="adi-new-win__section">
                    <h2 class="adi-new-win__label">"Background"</h2>
                    <div class="adi-segmented" role="group" aria-label="Appearance">
                        {Appearance::ALL
                            .into_iter()
                            .map(|(a, label)| view! {
                                <button
                                    class="adi-segmented__option"
                                    type="button"
                                    aria-pressed=move || {
                                        (wall.choice.get().appearance == a).to_string()
                                    }
                                    on:click=move |_| wall.set(|c| c.appearance = a)
                                >
                                    {label}
                                </button>
                            })
                            .collect_view()}
                    </div>
                    <div class="adi-new-walls">
                        {Preset::ALL
                            .into_iter()
                            .map(|p| {
                                let on = move || {
                                    let c = wall.choice.get();
                                    c.kind == Kind::Preset && c.preset == p
                                };
                                view! {
                                    <button
                                        class="adi-new-wall"
                                        type="button"
                                        aria-pressed=move || on().to_string()
                                        on:click=move |_| {
                                            tab.set(None);
                                            wall.set(|c| {
                                                c.kind = Kind::Preset;
                                                c.preset = p;
                                            });
                                        }
                                    >
                                        <span class="adi-new-wall__swatch" style=p.css()></span>
                                        <span class="adi-new-wall__name">{p.name()}</span>
                                    </button>
                                }
                            })
                            .collect_view()}
                    </div>
                </section>

                <section class="adi-new-win__section">
                    <h2 class="adi-new-win__label">"Custom"</h2>
                    <div class="adi-segmented" role="group" aria-label="Custom background">
                        {CUSTOM
                            .into_iter()
                            .map(|(kind, label)| view! {
                                <button
                                    class="adi-segmented__option"
                                    type="button"
                                    aria-pressed=move || (tab.get() == Some(kind)).to_string()
                                    on:click=move |_| pick_tab(kind)
                                >
                                    {label}
                                </button>
                            })
                            .collect_view()}
                    </div>
                    {move || match tab.get() {
                        Some(Kind::Color) => color_editor(wall).into_any(),
                        Some(Kind::Gradient) => gradient_editor(wall).into_any(),
                        Some(Kind::Image) => image_editor(wall).into_any(),
                        _ => ().into_any(),
                    }}
                </section>
            </div>
        </div>
    }
}

/// Keep a window of this width with enough of it on screen to take hold of again.
fn clamp(x: f64, y: f64, width: f64) -> (f64, f64) {
    let (vw, vh) = viewport();
    (
        x.clamp(KEEP_VISIBLE - width, (vw - KEEP_VISIBLE).max(0.0)),
        y.clamp(0.0, (vh - TITLEBAR).max(0.0)),
    )
}

fn viewport() -> (f64, f64) {
    let w = window();
    let px = |v: Result<wasm_bindgen::JsValue, _>| v.ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
    (px(w.inner_width()), px(w.inner_height()))
}

/// The saved position, pulled back on screen — the window may have been left on a larger one.
fn load_pos() -> Option<(f64, f64)> {
    let saved = ui::storage()?.get_item(POS_KEY).ok()??;
    let (x, y) = saved.split_once(',')?;
    let (x, y) = (x.parse().ok()?, y.parse().ok()?);
    // The width is not known before the window is drawn; the narrowest it is drawn at stands in.
    Some(clamp(x, y, KEEP_VISIBLE * 2.0))
}

/// One solid colour.
fn color_editor(wall: Wallpaper) -> impl IntoView {
    view! {
        {color_field(
            "Colour",
            Signal::derive(move || wall.choice.get().color),
            move |v| wall.set(|c| c.color = v),
        )}
    }
}

/// Two stops and an angle.
fn gradient_editor(wall: Wallpaper) -> impl IntoView {
    let angle = move || wall.choice.get().angle;
    view! {
        {color_field(
            "From",
            Signal::derive(move || wall.choice.get().from),
            move |v| wall.set(|c| c.from = v),
        )}
        {color_field(
            "To",
            Signal::derive(move || wall.choice.get().to),
            move |v| wall.set(|c| c.to = v),
        )}
        <label class="adi-new-field">
            <span class="adi-new-field__label">"Angle"</span>
            <span class="adi-new-field__value">
                <input
                    class="adi-new-range"
                    type="range"
                    min="0"
                    max="359"
                    prop:value=move || angle().to_string()
                    on:input=move |ev| {
                        if let Ok(a) = event_target_value(&ev).parse::<u16>() {
                            wall.set(|c| c.angle = a);
                        }
                    }
                />
                <span class="adi-new-field__num">{move || format!("{}°", angle())}</span>
            </span>
        </label>
    }
}

/// A labelled colour well with its hex beside it.
fn color_field(
    label: &'static str,
    value: Signal<String>,
    set: impl Fn(String) + 'static,
) -> impl IntoView {
    view! {
        <label class="adi-new-field">
            <span class="adi-new-field__label">{label}</span>
            <span class="adi-new-field__value">
                <input
                    class="adi-new-color"
                    type="color"
                    prop:value=move || value.get()
                    on:input=move |ev| {
                        let v = event_target_value(&ev);
                        if is_hex(&v) {
                            set(v);
                        }
                    }
                />
                <span class="mono">{move || value.get()}</span>
            </span>
        </label>
    }
}

/// How soft the image is. The preview above it stays sharp: it is the picture as chosen, and a
/// blur at thumbnail scale would not match what the screen shows anyway.
fn blur_field(wall: Wallpaper) -> impl IntoView {
    let blur = move || wall.choice.get().blur;
    view! {
        <label class="adi-new-field">
            <span class="adi-new-field__label">"Blur"</span>
            <span class="adi-new-field__value">
                <input
                    class="adi-new-range"
                    type="range"
                    min="0"
                    max=MAX_BLUR.to_string()
                    prop:value=move || blur().to_string()
                    on:input=move |ev| {
                        if let Ok(b) = event_target_value(&ev).parse::<u8>() {
                            wall.set(|c| c.blur = b.min(MAX_BLUR));
                        }
                    }
                />
                <span class="adi-new-field__num">
                    {move || match blur() {
                        0 => "Off".to_owned(),
                        b => format!("{b} px"),
                    }}
                </span>
            </span>
        </label>
    }
}

/// An image from this device, kept on this device.
fn image_editor(wall: Wallpaper) -> impl IntoView {
    let error = RwSignal::new(None::<&'static str>);
    let busy = RwSignal::new(false);

    let on_file = move |ev: ev::Event| {
        let Some(input) = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
        else {
            return;
        };
        let Some(file) = input.files().and_then(|f| f.get(0)) else {
            return;
        };
        // Cleared so choosing the same file again still fires `change`.
        input.set_value("");
        busy.set(true);
        error.set(None);
        wasm_bindgen_futures::spawn_local(async move {
            error.set(wall.keep_image(file).await.err());
            busy.set(false);
        });
    };

    view! {
        {move || wall.image.get().map(|url| view! {
            <span
                class="adi-new-win__preview"
                style=format!("background-image: url(\"{url}\")")
            ></span>
        })}
        {move || wall.image.get().is_some().then(|| blur_field(wall))}
        <div class="adi-new-win__actions">
            <label class="adi-btn" class:is-busy=move || busy.get()>
                <Icon icon=Lucide::Upload size=IconSize::Md/>
                {move || match (busy.get(), wall.image.get().is_some()) {
                    (true, _) => "Reading…",
                    (false, true) => "Replace image…",
                    (false, false) => "Choose image…",
                }}
                <input type="file" accept="image/*" hidden on:change=on_file/>
            </label>
            {move || wall.image.get().is_some().then(|| view! {
                <button
                    class="adi-btn adi-btn--quiet"
                    type="button"
                    on:click=move |_| {
                        error.set(None);
                        wall.remove_image();
                    }
                >
                    "Remove"
                </button>
            })}
        </div>
        {move || error.get().map(|e| view! { <p class="adi-new-win__note">{e}</p> })}
        <p class="adi-new-win__note">"Kept on this device only."</p>
    }
}
