//! The settings window's contents, at `/settings`. The window itself — title bar, drag, close —
//! is [`super::windows::Frame`]'s; this is what goes in it.
//!
//! Two sections: the background, and the layout of the chrome over it.

use adi_ui::{Icon, IconSize, Lucide};
use leptos::{ev, prelude::*};
use wasm_bindgen::JsCast;

use super::background::{Appearance, Kind, MAX_BLUR, Preset, Wallpaper, is_hex};
use super::shell::{Edge, Shell};

/// The custom kinds, in the order the segmented control shows them.
const CUSTOM: [(Kind, &str); 3] = [
    (Kind::Color, "Colour"),
    (Kind::Gradient, "Gradient"),
    (Kind::Image, "Image"),
];

#[component]
pub(super) fn Settings(wall: Wallpaper, shell: Shell) -> impl IntoView {
    // Which custom editor is open. Not the same as what is showing: opening Image before there
    // is one must not swap the wallpaper for nothing.
    let tab = RwSignal::new({
        let kind = wall.choice.get_untracked().kind;
        (kind != Kind::Preset).then_some(kind)
    });

    let pick_tab = move |kind: Kind| {
        tab.set(Some(kind));
        if kind != Kind::Image || wall.image.get_untracked().is_some() {
            wall.set(|c| c.kind = kind);
        }
    };

    view! {
        {layout_section(shell)}
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
    }
}

/// Where the island sits, and whether the top bar is drawn.
fn layout_section(shell: Shell) -> impl IntoView {
    let top_bar = move || shell.layout.get().top_bar;
    view! {
        <section class="adi-new-win__section">
            <h2 class="adi-new-win__label">"Layout"</h2>
            <div class="adi-new-field">
                <span class="adi-new-field__label">"Island"</span>
                <div class="adi-segmented" role="group" aria-label="Island position">
                    {Edge::ALL
                        .into_iter()
                        .map(|(e, label)| view! {
                            <button
                                class="adi-segmented__option"
                                type="button"
                                aria-pressed=move || (shell.layout.get().island == e).to_string()
                                on:click=move |_| shell.set(|l| l.island = e)
                            >
                                {label}
                            </button>
                        })
                        .collect_view()}
                </div>
            </div>
            <div class="adi-new-field">
                <span class="adi-new-field__label" id="adi-new-top-bar">"Top bar"</span>
                <button
                    class="adi-new-switch"
                    type="button"
                    role="switch"
                    aria-labelledby="adi-new-top-bar"
                    aria-checked=move || top_bar().to_string()
                    on:click=move |_| shell.set(|l| l.top_bar = !l.top_bar)
                ></button>
            </div>
        </section>
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
