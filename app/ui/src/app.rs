//! App root: context, theme switches on <html>, the desktop shell (Linear's
//! inverted L), the phone shell (tab bar), the command menu and shortcuts.

use std::time::Duration;

use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::Closure;

use crate::api;
use crate::pages;
use crate::prefs::{Prefs, Theme};
use crate::ui::{self, Ctx, Icon, Route, Toasts, ctx, icon, poll};

fn system_dark() -> bool {
    window()
        .match_media("(prefers-color-scheme: dark)")
        .ok()
        .flatten()
        .is_some_and(|m| m.matches())
}

/// Puts the preferences on <html> (tokens.css reads them).
fn apply_root(p: &Prefs, phone: bool, sys_dark: bool) {
    let Some(root) = document().document_element() else {
        return;
    };
    let set = |k: &str, v: Option<&str>| {
        let _ = match v {
            Some(v) => root.set_attribute(k, v),
            None => root.remove_attribute(k),
        };
    };
    let dark = match p.theme {
        Theme::Dark => true,
        Theme::Light => false,
        Theme::System => sys_dark,
    };
    let sun = p.sunlight_on(phone);
    set("data-mode", Some(p.mode.code()));
    set(
        "data-theme",
        Some(if dark && !sun { "dark" } else { "light" }),
    );
    set("data-sunlight", sun.then_some(""));
    set("data-boomer", p.boomer.then_some(""));
    set(
        "data-lite",
        (p.perf == crate::prefs::Perf::Lite).then_some(""),
    );
    set(
        "data-platform",
        Some(if phone { "phone" } else { "desktop" }),
    );
}

#[component]
pub fn App(initial: Prefs, route: Route, editing: Option<u32>) -> impl IntoView {
    let phone = api::backend().is_phone();
    let prefs = RwSignal::new(initial);
    let now = RwSignal::new(crate::format::now());
    let c = Ctx {
        prefs,
        phone,
        route: RwSignal::new(route),
        history_sel: RwSignal::new(None),
        editing: RwSignal::new(editing),
        toasts: RwSignal::new(Vec::new()),
        cmdk: RwSignal::new(None),
        now,
        // Replaced right below, once the context exists for poll().
        status: ui::Polled::placeholder(),
        open_alerts: RwSignal::new(0),
        pending: RwSignal::new(0),
        limits: RwSignal::new(None),
    };
    provide_context(c);
    let c = Ctx {
        status: poll(3000, api::status),
        ..c
    };
    provide_context(c);

    // Theme: follow the OS while "system" is picked.
    let sys_dark = RwSignal::new(system_dark());
    if let Ok(Some(mql)) = window().match_media("(prefers-color-scheme: dark)") {
        let cb = Closure::<dyn Fn()>::new(move || sys_dark.set(system_dark()));
        let _ = mql.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref());
        cb.forget();
    }
    Effect::new(move |_| prefs.with(|p| apply_root(p, phone, sys_dark.get())));

    if let Ok(h) = set_interval_with_handle(
        move || now.set(crate::format::now()),
        Duration::from_secs(1),
    ) {
        on_cleanup(move || h.clear());
    }

    // Badge counts.
    let alerts = poll(6000, move || async move {
        if api::backend().is_desktop() {
            api::alerts(true).await.map(|a| a.len())
        } else {
            Ok(0)
        }
    });
    let pending = poll(4000, || async { api::pending().await.map(|p| p.len()) });
    Effect::new(move |_| c.open_alerts.set(alerts.data.get().unwrap_or(0)));
    Effect::new(move |_| c.pending.set(pending.data.get().unwrap_or(0)));

    // Apply the saved limits once at start (the Tauri side re-applies them).
    c.save_prefs();
    shortcuts(c);

    let setup_done = move || prefs.with(|p| p.setup_done);
    view! {
        {move || {
            if !setup_done() {
                view! { <pages::wizard::Wizard/> }.into_any()
            } else if phone {
                view! { <PhoneShell/> }.into_any()
            } else {
                view! { <DesktopShell/> }.into_any()
            }
        }}
        <CommandMenu/>
        <Toasts/>
    }
}

/// ⌘K / Ctrl+K: command menu. G then a letter: jump (Linear). C: new recipe.
/// Ignored while typing in a field or the block editor.
fn shortcuts(c: Ctx) {
    let last_g = std::rc::Rc::new(std::cell::Cell::new(0.0f64));
    let handle = window_event_listener(ev::keydown, move |e: web_sys::KeyboardEvent| {
        if c.phone || !c.prefs.with_untracked(|p| p.setup_done) {
            return;
        }
        let key = e.key();
        if (e.meta_key() || e.ctrl_key()) && key.eq_ignore_ascii_case("k") {
            e.prevent_default();
            // Opened from the keyboard: no animation (Rauno).
            c.cmdk
                .update(|m| *m = if m.is_some() { None } else { Some(true) });
            return;
        }
        if key == "Escape" {
            c.cmdk.set(None);
            return;
        }
        if c.cmdk.get_untracked().is_some() || e.meta_key() || e.ctrl_key() || e.alt_key() {
            return;
        }
        let typing = document().active_element().is_some_and(|el| {
            matches!(el.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT")
                || el
                    .closest(".blockly, [contenteditable]")
                    .ok()
                    .flatten()
                    .is_some()
        });
        if typing {
            return;
        }
        let t = js_sys::Date::now();
        if key.eq_ignore_ascii_case("g") {
            last_g.set(t);
            return;
        }
        if t - last_g.get() < 1200.0 {
            last_g.set(0.0);
            if let Some(r) = Route::DESKTOP
                .into_iter()
                .find(|r| key.eq_ignore_ascii_case(&r.key().to_string()))
            {
                c.go(r);
            }
            return;
        }
        if key.eq_ignore_ascii_case("c") {
            c.route.set(Route::Automation);
            c.editing.set(Some(0));
        }
    });
    on_cleanup(move || handle.remove());
}

fn page_view(r: Route) -> AnyView {
    match r {
        Route::Overview => view! { <pages::overview::Overview/> }.into_any(),
        Route::Devices => view! { <pages::devices::Devices/> }.into_any(),
        Route::History => view! { <pages::history::History/> }.into_any(),
        Route::Automation => view! { <pages::automation::Automation/> }.into_any(),
        Route::Alerts => view! { <pages::alerts::Alerts/> }.into_any(),
        Route::Pairing => view! { <pages::pairing::Pairing/> }.into_any(),
        Route::Settings => view! { <pages::settings::Settings/> }.into_any(),
    }
}

#[component]
fn DesktopShell() -> impl IntoView {
    let c = ctx();
    let nav = Route::DESKTOP
        .into_iter()
        .map(|r| {
            let count = move || match r {
                Route::Alerts => c.open_alerts.get(),
                Route::Pairing => c.pending.get(),
                _ => 0,
            };
            view! {
                <button
                    class="nav"
                    class:on=move || c.route.get() == r
                    aria-current=move || (c.route.get() == r).then_some("page")
                    on:click=move |_| c.go(r)
                    title=move || c.prefs.with(|p| r.label(p, false))
                >
                    {icon(r.icon(), 16)}
                    <span class="nav-text">{move || c.prefs.with(|p| r.label(p, false))}</span>
                    {move || (count() > 0).then(|| {
                        let cls = if r == Route::Alerts { "pill bad count" } else { "pill ok count" };
                        view! { <span class=cls>{count()}</span> }
                    })}
                </button>
            }
        })
        .collect_view();

    let theme_toggle = move |_| {
        let dark = document()
            .document_element()
            .and_then(|e| e.get_attribute("data-theme"))
            .is_some_and(|t| t == "dark");
        c.update_prefs(|p| p.theme = if dark { Theme::Light } else { Theme::Dark });
    };

    view! {
        <div class="shell">
            <aside class="sidebar">
                <div class="brand">
                    <div class="brand-mark">{icon(Icon::Hex, 16)}</div>
                    <div class="brand-text">
                        <div class="brand-name">"Farm & Lab"</div>
                        <div class="faint small">{move || format!("{} mode", c.prefs.with(|p| p.mode.name()))}</div>
                    </div>
                </div>
                <nav class="nav-list" aria-label="Main">{nav}</nav>
                <HubCard/>
            </aside>
            <main class="panel">
                <div class="topbar">
                    <span class="faint">{move || c.prefs.with(|p| p.mode.name())}</span>
                    <span class="crumb-sep">"/"</span>
                    <span style="font-weight: 500">{move || c.prefs.with(|p| c.route.get().label(p, false))}</span>
                    <span class="spacer"></span>
                    <button
                        class="btn sm"
                        style="width: 240px; justify-content: space-between; color: var(--faint); font-weight: 400"
                        on:click=move |_| c.cmdk.set(Some(false))
                    >
                        <span class="hstack">{icon(Icon::Search, 14)}"Search or jump to…"</span>
                        <span class="kbd">"⌘K"</span>
                    </button>
                    <button class="btn sm icon" title="Light / dark" aria-label="Switch light or dark" on:click=theme_toggle>
                        {move || {
                            let dark = c.prefs.with(|p| p.theme) == Theme::Dark;
                            icon(if dark { Icon::Sun } else { Icon::Moon }, 14)
                        }}
                    </button>
                    <button class="btn sm icon" aria-label="Alerts" on:click=move |_| c.go(Route::Alerts)>
                        {icon(Icon::Bell, 14)}
                    </button>
                </div>
                {move || {
                    c.status.error.get().map(|e| view! {
                        <div class="banner" style="margin: 12px 16px 0">{e}</div>
                    })
                }}
                <div class="page">
                    <div class="page-inner">{move || page_view(c.route.get())}</div>
                </div>
            </main>
        </div>
    }
}

/// Hub link summary at the bottom of the sidebar.
#[component]
fn HubCard() -> impl IntoView {
    let c = ctx();
    view! {
        <div class="hub-card">
            <div class="hstack" style="justify-content: space-between">
                <span style="font-weight: 600; font-size: var(--fs-label)">"Hub"</span>
                {move || {
                    let s = c.status.data.get();
                    match (c.status.error.get().is_some(), s.map(|s| s.hub)) {
                        (true, _) => view! { <span class="pill bad"><span class="dot"></span>"App offline"</span> }.into_any(),
                        (_, None) => view! { <span class="pill">"…"</span> }.into_any(),
                        (_, Some(h)) if !h.paired => view! { <span class="pill warn"><span class="dot"></span>"Not paired"</span> }.into_any(),
                        (_, Some(h)) => ui::status_pill(h.connected, "Linked", "Offline").into_any(),
                    }
                }}
            </div>
            <span class="fp muted">{move || c.status.data.get().and_then(|s| s.hub.fingerprint).unwrap_or_else(|| "—".into())}</span>
            <span class="faint small">
                {move || c.status.data.get().map_or(String::new(), |s| format!("PQ hybrid · {} key exchanges", s.hub.sessions))}
            </span>
        </div>
    }
}

#[component]
fn PhoneShell() -> impl IntoView {
    let c = ctx();
    let tabs = Route::PHONE
        .into_iter()
        .map(|r| {
            let count = move || match r {
                Route::Pairing => c.pending.get(),
                _ => 0,
            };
            view! {
                <button class:on=move || c.route.get() == r on:click=move |_| c.go(r)>
                    {icon(r.icon(), 22)}
                    <span>
                        {move || c.prefs.with(|p| r.label(p, true))}
                        {move || (count() > 0).then(|| format!(" ({})", count()))}
                    </span>
                </button>
            }
        })
        .collect_view();
    // History and the recipe editor are sub-pages with a back button.
    let back = move || match c.route.get() {
        Route::History => Some(Route::Overview),
        Route::Automation if c.editing.get().is_some() => Some(Route::Automation),
        _ => None,
    };
    let title = move || {
        let r = c.route.get();
        if r == Route::Automation && c.editing.get().is_some() {
            return "Recipe".to_string();
        }
        c.prefs.with(|p| r.label(p, true)).to_string()
    };
    view! {
        <div class="phone">
            <header class="phone-top">
                {move || back().map(|to| view! {
                    <button class="btn icon ghost" aria-label="Back" on:click=move |_| c.go(to)>{icon(Icon::Back, 20)}</button>
                })}
                <h1>{title}</h1>
                <span class="spacer"></span>
                {move || {
                    let ok = c.status.error.get().is_none() && c.status.data.get().is_some();
                    ui::status_pill(ok, "Hub", "Offline")
                }}
            </header>
            <div class="phone-page">
                <div class="page-inner">{move || page_view(c.route.get())}</div>
            </div>
            <nav class="tabbar" aria-label="Main">{tabs}</nav>
        </div>
    }
}

/// One entry in the command menu.
#[derive(Clone)]
struct Cmd {
    group: &'static str,
    label: String,
    keys: Vec<String>,
    run: Callback<()>,
}

/// Linear-style command menu: "Jump to" and "Actions", type to filter,
/// arrows + Enter.
#[component]
fn CommandMenu() -> impl IntoView {
    let c = ctx();
    let (query, set_query) = signal(String::new());
    let (sel, set_sel) = signal(0usize);
    let outputs = RwSignal::new(Vec::<(u16, u8, String, bool)>::new());
    let input = NodeRef::<leptos::html::Input>::new();

    // Outputs for the "Turn … on/off" actions, loaded when the menu opens.
    Effect::new(move |_| {
        if c.cmdk.get().is_none() {
            return;
        }
        set_query.set(String::new());
        set_sel.set(0);
        spawn_local(async move {
            if let Ok(nodes) = api::nodes().await {
                outputs.set(
                    nodes
                        .iter()
                        .flat_map(|n| {
                            n.channels
                                .iter()
                                .filter(|ch| ch.kind.is_output())
                                .map(|ch| {
                                    (
                                        n.id,
                                        ch.channel,
                                        format!("{} / {}", n.name, ch.name),
                                        ch.last_value == Some(1),
                                    )
                                })
                        })
                        .collect(),
                );
            }
        });
        request_animation_frame(move || {
            if let Some(i) = input.get_untracked() {
                let _ = i.focus();
            }
        });
    });

    let commands = move || {
        let mut v: Vec<Cmd> = Route::DESKTOP
            .into_iter()
            .map(|r| Cmd {
                group: "Jump to",
                label: c.prefs.with_untracked(|p| r.label(p, false)).to_string(),
                keys: vec!["G".into(), r.key().to_string()],
                run: Callback::new(move |_| c.go(r)),
            })
            .collect();
        v.push(Cmd {
            group: "Actions",
            label: "New recipe".into(),
            keys: vec!["C".into()],
            run: Callback::new(move |_| {
                c.route.set(Route::Automation);
                c.editing.set(Some(0));
            }),
        });
        v.push(Cmd {
            group: "Actions",
            label: "Add phone (QR code)".into(),
            keys: vec![],
            run: Callback::new(move |_| c.go(Route::Pairing)),
        });
        v.push(Cmd {
            group: "Actions",
            label: "Switch light / dark".into(),
            keys: vec![],
            run: Callback::new(move |_| {
                c.update_prefs(|p| {
                    p.theme = if p.theme == Theme::Dark {
                        Theme::Light
                    } else {
                        Theme::Dark
                    }
                })
            }),
        });
        for (node, ch, name, on) in outputs.get() {
            v.push(Cmd {
                group: "Actions",
                label: format!("Turn {name} {}", if on { "off" } else { "on" }),
                keys: vec![],
                run: Callback::new(move |_| {
                    let value = i64::from(!on);
                    spawn_local(async move {
                        match api::command(node, ch, value).await {
                            Ok(_) => ui::toast(format!(
                                "Turned {}",
                                if value == 1 { "on" } else { "off" }
                            )),
                            Err(e) => ui::toast_err(e),
                        }
                    });
                }),
            });
        }
        let q = query.get().to_lowercase();
        v.retain(|c| q.is_empty() || c.label.to_lowercase().contains(&q));
        v
    };

    let run = move |i: usize| {
        if let Some(cmd) = commands().get(i).cloned() {
            c.cmdk.set(None);
            cmd.run.run(());
        }
    };

    view! {
        {move || c.cmdk.get().map(|keyboard| {
            let cls = if keyboard { "instant" } else { "" };
            view! {
                <div class=format!("scrim {cls}") on:click=move |_| c.cmdk.set(None)></div>
                <div class=format!("cmdk {cls}") role="dialog" aria-label="Command menu">
                    <input
                        node_ref=input
                        placeholder="Type a command or search…"
                        prop:value=query
                        on:input=move |e| {
                            set_query.set(event_target_value(&e));
                            set_sel.set(0);
                        }
                        on:keydown=move |e: web_sys::KeyboardEvent| {
                            let n = commands().len().max(1);
                            match e.key().as_str() {
                                "ArrowDown" => { e.prevent_default(); set_sel.update(|s| *s = (*s + 1) % n); }
                                "ArrowUp" => { e.prevent_default(); set_sel.update(|s| *s = (*s + n - 1) % n); }
                                "Enter" => { e.prevent_default(); run(sel.get_untracked()); }
                                _ => {}
                            }
                        }
                    />
                    <div class="cmdk-list" role="listbox">
                        {move || {
                            let list = commands();
                            if list.is_empty() {
                                return view! { <div class="empty">"No matches."</div> }.into_any();
                            }
                            let mut last = "";
                            list.into_iter().enumerate().map(|(i, cmd)| {
                                let head = (cmd.group != last).then(|| view! { <div class="cmdk-group eyebrow">{cmd.group}</div> });
                                last = cmd.group;
                                view! {
                                    {head}
                                    <button
                                        class="cmdk-item"
                                        role="option"
                                        class:sel=move || sel.get() == i
                                        on:mouseenter=move |_| set_sel.set(i)
                                        on:click=move |_| run(i)
                                    >
                                        {cmd.label}
                                        <span class="kbds">{cmd.keys.into_iter().map(|k| view! { <span class="kbd">{k}</span> }).collect_view()}</span>
                                    </button>
                                }
                            }).collect_view().into_any()
                        }}
                    </div>
                    <div class="cmdk-foot">
                        <span><span class="kbd">"↑↓"</span>" move"</span>
                        <span><span class="kbd">"↵"</span>" run"</span>
                        <span><span class="kbd">"esc"</span>" close"</span>
                    </div>
                </div>
            }
        })}
    }
}
