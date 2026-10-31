//! Settings, and the pickers the first-run wizard shares with it.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api;
use crate::prefs::{Mode, Perf, Theme};
use crate::ui::{self, Switch, ctx};

fn swatch(m: Mode) -> &'static str {
    match m {
        Mode::Lab => "#3B82F6",
        Mode::Farm => "#2FBF71",
        Mode::Home => "#FF8A3D",
    }
}

fn mode_blurb(m: Mode) -> &'static str {
    match m {
        Mode::Lab => "Blue. Benches, instruments, incubators.",
        Mode::Farm => "Green. Fields, barns, greenhouses.",
        Mode::Home => "Orange. Rooms, heating, the garden.",
    }
}

/// Lab / Farm / Home. Colors change as soon as one is picked.
#[component]
pub fn ModePicker() -> impl IntoView {
    let c = ctx();
    view! {
        <div class="choices" role="radiogroup" aria-label="Mode">
            {Mode::ALL.into_iter().map(|m| view! {
                <button
                    class="choice"
                    role="radio"
                    class:on=move || c.prefs.with(|p| p.mode) == m
                    aria-checked=move || (c.prefs.with(|p| p.mode) == m).to_string()
                    on:click=move |_| c.update_prefs(|p| p.mode = m)
                >
                    <span class="swatch" style=format!("background: {}", swatch(m))></span>
                    <span class="grow stack" style="gap: 2px">
                        <span class="title">{m.name()}</span>
                        <span class="muted small">{mode_blurb(m)}</span>
                    </span>
                </button>
            }).collect_view()}
        </div>
    }
}

#[component]
pub fn ThemePicker() -> impl IntoView {
    let c = ctx();
    let opt = move |t: Theme, label: &'static str| {
        view! {
            <button class:on=move || c.prefs.with(|p| p.theme) == t on:click=move |_| c.update_prefs(|p| p.theme = t)>{label}</button>
        }
    };
    view! {
        <div class="seg" role="radiogroup" aria-label="Theme">
            {opt(Theme::Light, "Light")}
            {opt(Theme::Dark, "Dark")}
            {opt(Theme::System, "Match system")}
        </div>
    }
}

/// Standard or Boomer Mode, as two big choices.
#[component]
pub fn LookPicker() -> impl IntoView {
    let c = ctx();
    let opt = move |boomer: bool, title: &'static str, blurb: &'static str| {
        view! {
            <button class="choice" class:on=move || c.prefs.with(|p| p.boomer) == boomer on:click=move |_| c.update_prefs(|p| p.boomer = boomer)>
                <span class="grow stack" style="gap: 2px">
                    <span class="title">{title}</span>
                    <span class="muted small">{blurb}</span>
                </span>
            </button>
        }
    };
    view! {
        <div class="choices">
            {opt(false, "Standard", "Compact, with keyboard shortcuts and the block editor.")}
            {opt(true, "Boomer Mode", "Big text, big buttons, plain words, no animations. Rules are written as a simple sentence.")}
        </div>
    }
}

/// Lite / Normal and the CPU slider, with what this machine has.
#[component]
pub fn PerfPicker() -> impl IntoView {
    let c = ctx();
    let sys = RwSignal::new(None::<api::SystemInfo>);
    spawn_local(async move {
        sys.try_set(Some(api::system_info().await));
    });
    let opt = move |perf: Perf, title: &'static str, blurb: &'static str| {
        view! {
            <button class="choice" class:on=move || c.prefs.with(|p| p.perf) == perf on:click=move |_| c.update_prefs(|p| p.perf = perf)>
                <span class="grow stack" style="gap: 2px">
                    <span class="title">{title}</span>
                    <span class="muted small">{blurb}</span>
                </span>
            </button>
        }
    };
    let ram_gb = move || {
        sys.get()
            .filter(|s| s.total_ram_mb > 0)
            .map(|s| s.total_ram_mb as f64 / 1024.0)
    };
    view! {
        <div class="choices">
            {opt(Perf::Lite, "Lite", "Uses less than 3 GB of memory. Fewer animations and shorter charts. For older or busy computers.")}
            {opt(Perf::Normal, "Normal", "Uses less than 6 GB of memory.")}
        </div>
        {move || ram_gb().map(|g| {
            let tip = if g < 8.0 { "Lite is recommended." } else { "Either works." };
            view! { <p class="muted small">{format!("This device has {g:.1} GB of memory and {} processor cores. {tip}", sys.get().map_or(0, |s| s.cpus))}</p> }
        })}
        <label class="form">
            <span class="hstack" style="justify-content: space-between">
                <span>"Processor limit"</span>
                <span class="mono" style="color: var(--text)">{move || format!("{} %", c.prefs.with(|p| p.cpu_pct))}</span>
            </span>
            <input
                type="range"
                min="10"
                max="100"
                step="5"
                aria-label="Processor limit in percent"
                prop:value=move || c.prefs.with(|p| p.cpu_pct).to_string()
                on:input=move |e| {
                    if let Ok(v) = event_target_value(&e).parse::<u8>() {
                        c.prefs.update(|p| p.cpu_pct = v);
                    }
                }
                on:change=move |_| c.save_prefs()
            />
            <span class="faint small">"The most of the processor this app (and the PC daemon) may use. Lower values also refresh less often."</span>
        </label>
        {move || c.limits.get().map(|l| {
            let cls = if l.kind == "hard" { "pill ok" } else { "pill warn" };
            let label = if l.kind == "hard" { "Enforced by the system" } else { "Best effort" };
            view! { <div class="hstack small"><span class=cls>{label}</span><span class="muted">{l.detail}</span></div> }
        })}
    }
}

#[component]
pub fn Settings() -> impl IntoView {
    let c = ctx();
    let section = |title: &'static str, body: AnyView| {
        view! {
            <section class="card stack rise">
                <span class="eyebrow">{title}</span>
                {body}
            </section>
        }
    };
    let sunlight = Signal::derive(move || c.prefs.with(|p| p.sunlight_on(c.phone)));
    view! {
        {(!c.phone).then(|| view! {
            <header class="page-head rise">
                <div><h1>"Settings"</h1><p>"Saved on this device."</p></div>
            </header>
        })}
        {c.phone.then(|| section("Hub", view! {
            <crate::pages::pairing::PhoneLink/>
            <button class="btn danger" on:click=move |_| {
                if ui::confirm("Forget the hub?\n\nThis phone stops working with it until it is paired again.") {
                    spawn_local(async move {
                        match api::phone_forget().await {
                            Ok(_) => ui::toast("Hub forgotten"),
                            Err(e) => ui::toast_err(e),
                        }
                    });
                }
            }>"Forget this hub"</button>
        }.into_any()))}
        {section("Mode", view! { <ModePicker/> }.into_any())}
        {section("Look", view! {
            <LookPicker/>
            <div class="hstack" style="justify-content: space-between">
                <span>"Theme"</span>
                <ThemePicker/>
            </div>
            <div class="hstack" style="justify-content: space-between">
                <span class="stack" style="gap: 2px">
                    <span>"Sunlight"</span>
                    <span class="faint small">"Black on white, thick lines, bigger buttons: readable in bright sun. On by default for Farm on the phone."</span>
                </span>
                <Switch on=sunlight label="Sunlight" on_toggle=move |v| c.update_prefs(|p| p.sunlight = Some(v))/>
            </div>
        }.into_any())}
        {section("Performance", view! { <PerfPicker/> }.into_any())}
        {section("About", view! {
            <div class="hstack">
                <span class="muted small" style="width: 140px">{if c.phone { "This phone" } else { "This PC" }}</span>
                <span class="fp">{move || c.status.data.get().map(|s| s.pc_fingerprint)}</span>
            </div>
            <div class="hstack">
                <span class="muted small" style="width: 140px">"Hub"</span>
                <span class="fp">{move || c.status.data.get().and_then(|s| s.hub.fingerprint).unwrap_or_else(|| "not paired".into())}</span>
            </div>
            <p class="faint small">"Links to the hub use hybrid post-quantum encryption: Ed25519 + ML-DSA-65 signatures, X25519 + ML-KEM-768 key exchange."</p>
            <div class="hstack">
                <button class="btn" on:click=move |_| c.update_prefs(|p| p.setup_done = false)>"Run first-time setup again"</button>
            </div>
        }.into_any())}
    }
}
