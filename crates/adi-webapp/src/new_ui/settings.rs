//! The settings sheet, at `/settings`: a side sheet over the screen, so the wallpaper it edits
//! stays in view and changes as it is picked.
//!
//! Background is its only section for now.

use adi_ui::{Icon, IconSize, Lucide};
use leptos::{ev, prelude::*};
use wasm_bindgen::JsCast;

use super::background::{Appearance, Kind, Preset, Wallpaper, is_hex};

/// The custom kinds, in the order the segmented control shows them.
const CUSTOM: [(Kind, &str); 3] = [
    (Kind::Color, "Colour"),
    (Kind::Gradient, "Gradient"),
    (Kind::Image, "Image"),
];

#[component]
pub(super) fn Sheet(wall: Wallpaper, #[prop(into)] close: Callback<()>) -> impl IntoView {
    // Which custom editor is open. Not the same as what is showing: opening Image before there
    // is one must not swap the wallpaper for nothing.
    let tab = RwSignal::new({
        let kind = wall.choice.get_untracked().kind;
        (kind != Kind::Preset).then_some(kind)
    });

    // Escape closes the sheet — unless it is closing the ⌘K menu above it (`adi_ui::Modal`, a
    // `role="dialog"` that listens for the same key on the same window).
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

    view! {
        <aside class="adi-new-sheet" aria-label="Settings">
            <header class="adi-new-sheet__head">
                <h1 class="adi-new-sheet__title">"Settings"</h1>
                <button
                    class="adi-btn adi-btn--icon"
                    type="button"
                    aria-label="Close settings"
                    on:click=move |_| close.run(())
                >
                    <Icon icon=Lucide::X size=IconSize::Md/>
                </button>
            </header>

            <section class="adi-new-sheet__section">
                <h2 class="adi-new-sheet__label">"Background"</h2>
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

            <section class="adi-new-sheet__section">
                <h2 class="adi-new-sheet__label">"Custom"</h2>
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
        </aside>
    }
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
                class="adi-new-sheet__preview"
                style=format!("background-image: url(\"{url}\")")
            ></span>
        })}
        <div class="adi-new-sheet__actions">
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
        {move || error.get().map(|e| view! { <p class="adi-new-sheet__note">{e}</p> })}
        <p class="adi-new-sheet__note">"Kept on this device only."</p>
    }
}
