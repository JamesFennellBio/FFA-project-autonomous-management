//! First-run setup: welcome -> mode -> look (Standard / Boomer) -> theme ->
//! performance -> link the hub. Every choice applies at once, so the screen
//! itself shows what was picked.

use leptos::prelude::*;

use crate::pages::pairing::{AddHub, PhoneLink};
use crate::pages::settings::{LookPicker, ModePicker, PerfPicker, ThemePicker};
use crate::ui::{Icon, ctx, icon};

const STEPS: usize = 6;

#[component]
pub fn Wizard() -> impl IntoView {
    let c = ctx();
    let step = RwSignal::new(0usize);
    let finish = move || c.update_prefs(|p| p.setup_done = true);
    let next = move |_| {
        if step.get_untracked() + 1 >= STEPS {
            finish();
        } else {
            step.update(|s| *s += 1);
        }
    };
    let body = move || -> AnyView {
        match step.get() {
            0 => view! {
                <div class="brand-mark" style="width: 44px; height: 44px; border-radius: 12px">{icon(Icon::Hex, 24)}</div>
                <h1>"Welcome to Farm & Lab"</h1>
                <p class="muted">"A few quick choices, then link your hub. Everything can be changed later in Settings."</p>
            }.into_any(),
            1 => view! {
                <h2>"What is this for?"</h2>
                <p class="muted">"Sets the color and the words the app uses."</p>
                <ModePicker/>
            }.into_any(),
            2 => view! {
                <h2>"How should it look?"</h2>
                <LookPicker/>
            }.into_any(),
            3 => view! {
                <h2>"Light or dark?"</h2>
                <ThemePicker/>
            }.into_any(),
            4 => view! {
                <h2>"Performance"</h2>
                <PerfPicker/>
            }.into_any(),
            _ => {
                let done = Callback::new(move |_| ());
                if c.phone {
                    view! {
                        <h2>"Link this phone"</h2>
                        <PhoneLink on_done=done/>
                    }
                    .into_any()
                } else {
                    view! {
                        <h2>"Link your hub"</h2>
                        <AddHub on_done=done/>
                        <p class="faint small">"No hub yet? Skip this; it's under Pairing."</p>
                    }
                    .into_any()
                }
            }
        }
    };
    view! {
        <div class="wizard">
            <div class="wizard-card">
                <div class="steps" aria-hidden="true">
                    {(0..STEPS).map(|i| view! { <span class:done=move || i <= step.get()></span> }).collect_view()}
                </div>
                <div class="stack" style="gap: 16px">{body}</div>
                <div class="hstack">
                    {move || (step.get() > 0).then(|| view! {
                        <button class="btn" on:click=move |_| step.update(|s| *s -= 1)>"Back"</button>
                    })}
                    <span class="spacer"></span>
                    {move || (step.get() + 1 == STEPS).then(|| view! {
                        <button class="btn ghost" on:click=move |_| finish()>"Skip for now"</button>
                    })}
                    <button class="btn primary" on:click=next>
                        {move || match step.get() {
                            0 => "Get started",
                            s if s + 1 == STEPS => "Finish",
                            _ => "Continue",
                        }}
                    </button>
                </div>
            </div>
        </div>
    }
}
