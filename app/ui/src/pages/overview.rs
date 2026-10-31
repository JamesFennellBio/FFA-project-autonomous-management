//! Overview (desktop) and Live (phone): what every sensor reads now, and the
//! outputs, with a toast + Undo when one is switched.

use std::collections::HashMap;

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api;
use crate::format;
use crate::model::NodeInfo;
use crate::ui::{self, Icon, Polled, Route, Switch, ctx, icon, poll};

type Key = (u16, u8);

/// Switches an output right away in the UI, then tells the hub. A toast
/// confirms it, with Undo.
pub fn set_output(
    key: Key,
    name: String,
    on: bool,
    shown: RwSignal<HashMap<Key, bool>>,
    nodes: Polled<Vec<NodeInfo>>,
) {
    shown.update(|m| {
        m.insert(key, on);
    });
    spawn_local(async move {
        let r = api::command(key.0, key.1, i64::from(on)).await;
        match r {
            Ok(note) => {
                let msg = format!("{name} turned {}", if on { "on" } else { "off" });
                let undo = Callback::new(move |_| {
                    set_output(key, name.clone(), !on, shown, nodes);
                });
                ui::toast_undo(msg, undo);
                if let Some(n) = note {
                    ui::toast(n);
                }
            }
            Err(e) => ui::toast_err(e),
        }
        nodes.refresh();
        // Keep showing the new state until the next reading confirms it.
        set_timeout(
            move || {
                shown.try_update(|m| {
                    m.remove(&key);
                });
            },
            std::time::Duration::from_secs(4),
        );
    });
}

fn find(nodes: &[NodeInfo], key: Key) -> Option<(NodeInfo, crate::model::Channel)> {
    let n = nodes.iter().find(|n| n.id == key.0)?;
    let ch = n.channels.iter().find(|c| c.channel == key.1)?;
    Some((n.clone(), ch.clone()))
}

#[component]
pub fn Overview() -> impl IntoView {
    if ctx().phone {
        view! { <Live/> }.into_any()
    } else {
        view! { <DesktopOverview/> }.into_any()
    }
}

#[component]
fn DesktopOverview() -> impl IntoView {
    let c = ctx();
    let nodes = poll(3000, api::nodes);
    let recipes = poll(15_000, api::recipes);
    let alerts = poll(8000, || api::alerts(true));
    let shown = RwSignal::new(HashMap::<Key, bool>::new());
    let spark = RwSignal::new(HashMap::<Key, Vec<f64>>::new());

    // Sparklines: the last hour once, then each new reading.
    let seeded = StoredValue::new(false);
    Effect::new(move |_| {
        let Some(list) = nodes.data.get() else { return };
        if !seeded.get_value() {
            seeded.set_value(true);
            let since = format::now() - 3600;
            for n in &list {
                for ch in n.channels.iter().filter(|c| !c.kind.is_output()) {
                    let (key, kind) = ((n.id, ch.channel), ch.kind);
                    spawn_local(async move {
                        if let Ok(r) = api::readings(key.0, key.1, since, 40).await {
                            let v: Vec<f64> = r
                                .iter()
                                .map(|x| format::chart_value(kind, x.value))
                                .collect();
                            spark.try_update(|m| {
                                m.insert(key, v);
                            });
                        }
                    });
                }
            }
        }
        spark.update(|m| {
            for n in &list {
                for ch in &n.channels {
                    if let Some(v) = ch.last_value {
                        let buf = m.entry((n.id, ch.channel)).or_default();
                        buf.push(format::chart_value(ch.kind, v));
                        if buf.len() > 60 {
                            buf.remove(0);
                        }
                    }
                }
            }
        });
    });

    let sensors = move || {
        nodes.data.with(|d| {
            d.iter()
                .flatten()
                .flat_map(|n| {
                    n.channels
                        .iter()
                        .filter(|c| !c.kind.is_output())
                        .map(move |c| (n.id, c.channel))
                })
                .collect::<Vec<_>>()
        })
    };
    let outputs = move || {
        nodes.data.with(|d| {
            d.iter()
                .flatten()
                .flat_map(|n| {
                    n.channels
                        .iter()
                        .filter(|c| c.kind.is_output())
                        .map(move |c| (n.id, c.channel))
                })
                .collect::<Vec<_>>()
        })
    };
    let online = move || {
        let now = c.now.get();
        nodes.data.with(|d| {
            d.as_ref().map(|d| {
                (
                    d.iter()
                        .filter(|n| format::is_online(n.last_seen, now))
                        .count(),
                    d.len(),
                )
            })
        })
    };
    let last_reading = move || {
        let now = c.now.get();
        nodes.data.with(|d| {
            let t = d
                .iter()
                .flatten()
                .flat_map(|n| n.channels.iter().filter_map(|c| c.last_ts))
                .max();
            format::ago(t, now).replace(" ago", "")
        })
    };
    let hub_unpaired = move || {
        c.status
            .data
            .with(|s| s.as_ref().is_some_and(|s| !s.hub.paired))
    };

    view! {
        <header class="page-head rise">
            <div>
                <h1>"Overview"</h1>
                <p>{move || c.prefs.with(|p| p.mode.copy().tagline)}</p>
            </div>
            <div class="hstack">
                <button class="btn" on:click=move |_| c.go(Route::Pairing)>"Add device"</button>
                <button class="btn primary" on:click=move |_| { c.route.set(Route::Automation); c.editing.set(Some(0)); }>
                    "New recipe" <span class="kbd">"C"</span>
                </button>
            </div>
        </header>

        {move || hub_unpaired().then(|| view! {
            <div class="banner warn hstack rise">
                <span class="grow" style="flex: 1">"The hub isn't paired yet. On the Pi run "<code>"hub-node pair"</code>", then enter its code under Pairing."</span>
                <button class="btn sm" on:click=move |_| c.go(Route::Pairing)>"Pair hub"</button>
            </div>
        })}

        <section class="stats">
            <div class="card stat rise">
                <span class="label muted">{move || format!("{} online", c.prefs.with(|p| p.mode.copy().devices))}</span>
                <span class="num">
                    {move || online().map_or("—".into(), |(a, _)| a.to_string())}
                    <small>{move || online().map(|(_, b)| format!(" / {b}"))}</small>
                </span>
            </div>
            <button class="card stat rise" on:click=move |_| c.go(Route::Alerts)>
                <span class="label muted">"Open alerts"</span>
                <span class="num">{move || alerts.data.get().map_or("—".into(), |a| a.len().to_string())}</span>
            </button>
            <button class="card stat rise" on:click=move |_| c.go(Route::Automation)>
                <span class="label muted">"Recipes on"</span>
                <span class="num">
                    {move || recipes.data.get().map_or("—".into(), |r| r.recipes.iter().filter(|r| r.enabled).count().to_string())}
                </span>
            </button>
            <div class="card stat rise">
                <span class="label muted">"Last reading"</span>
                <span class="num">{last_reading}<small>" ago"</small></span>
            </div>
        </section>

        {move || nodes.error.get().map(|e| view! { <div class="banner">{e}</div> })}
        {move || (nodes.data.with(|d| d.as_ref().is_some_and(Vec::is_empty))).then(|| view! {
            <div class="card empty">
                {icon(Icon::Chip, 28)}
                <h2>{move || format!("No {} yet", c.prefs.with(|p| p.mode.copy().devices.to_lowercase()))}</h2>
                <p>"Hold the BOOT button on a node for 3 seconds, then approve it on the Pairing page."</p>
                <button class="btn primary" on:click=move |_| c.go(Route::Pairing)>"Go to Pairing"</button>
            </div>
        })}
        {move || nodes.loading().then(|| view! {
            <div class="grid">{(0..6).map(|_| view! { <div class="card skel" style="height: 104px"></div> }).collect_view()}</div>
        })}

        <section class="stack" style:display=move || if sensors().is_empty() { "none" } else { "flex" }>
            <div class="hstack" style="justify-content: space-between">
                <h2>"Live readings"</h2>
                <button class="btn ghost sm" style="color: var(--accent-text)" on:click=move |_| c.go(Route::History)>"Open history"</button>
            </div>
            <div class="grid">
                <For each=sensors key=|k| *k let:key>
                    {
                        let item = Memo::new(move |_| nodes.data.with(|d| d.as_deref().and_then(|d| find(d, key))));
                        view! {
                            <button
                                class="card reading rise stack"
                                style="gap: 10px"
                                on:click=move |_| { c.history_sel.set(Some(key)); c.go(Route::History); }
                            >
                                <div class="hstack" style="justify-content: space-between; width: 100%">
                                    <span class="label muted">{move || item.get().map(|(n, ch)| format!("{} / {}", n.name, ch.name))}</span>
                                    {move || item.get().map(|(n, _)| ui::status_pill(format::is_online(n.last_seen, c.now.get()), "Live", "Offline"))}
                                </div>
                                <div class="hstack" style="justify-content: space-between; width: 100%; align-items: flex-end">
                                    <span class="num">
                                        {move || item.get().map(|(_, ch)| format::value_parts(ch.kind, ch.last_value).0)}
                                        <small>{move || item.get().map(|(_, ch)| format!(" {}", format::value_parts(ch.kind, ch.last_value).1))}</small>
                                    </span>
                                    <svg class="spark" viewBox="0 0 88 28" aria-hidden="true">
                                        <path d=move || spark.with(|m| m.get(&key).map(|v| ui::spark_path(v)).unwrap_or_default())></path>
                                    </svg>
                                </div>
                            </button>
                        }
                    }
                </For>
            </div>
        </section>

        <section class="stack" style:display=move || if outputs().is_empty() { "none" } else { "flex" }>
            <h2>"Outputs"</h2>
            <div class="rows">
                <For each=outputs key=|k| *k let:key>
                    {
                        let item = Memo::new(move |_| nodes.data.with(|d| d.as_deref().and_then(|d| find(d, key))));
                        let on = Signal::derive(move || {
                            shown.with(|m| m.get(&key).copied()).unwrap_or_else(|| item.get().is_some_and(|(_, ch)| ch.last_value == Some(1)))
                        });
                        let name = move || item.get().map(|(n, ch)| format!("{} / {}", n.name, ch.name)).unwrap_or_default();
                        view! {
                            <div class="row">
                                <span class="grow title">{name}</span>
                                <span class="faint small">{move || item.get().map(|(_, ch)| format::ago(ch.last_ts, c.now.get()))}</span>
                                <Switch on=on label=name() on_toggle=move |v| set_output(key, name(), v, shown, nodes)/>
                            </div>
                        }
                    }
                </For>
            </div>
        </section>

        <section class="stack" style:display=move || if alerts.data.with(|a| a.as_ref().is_none_or(Vec::is_empty)) { "none" } else { "flex" }>
            <div class="hstack" style="justify-content: space-between">
                <h2>"Recent alerts"</h2>
                <button class="btn ghost sm" style="color: var(--accent-text)" on:click=move |_| c.go(Route::Alerts)>"View all"</button>
            </div>
            <div class="rows">
                {move || alerts.data.get().unwrap_or_default().into_iter().take(4).map(|a| view! {
                    <div class="row">
                        <span class="pill bad"><span class="dot"></span>{format::alert_source(&a)}</span>
                        <span class="grow">{a.message.clone()}</span>
                        <span class="faint small">{format::ago(Some(a.ts), c.now.get())}</span>
                    </div>
                }).collect_view()}
            </div>
        </section>
    }
}

/// Phone: one card per node, every channel as a row. Tap a reading for its
/// history; outputs have a switch.
#[component]
fn Live() -> impl IntoView {
    let c = ctx();
    let nodes = poll(5000, api::nodes);
    let shown = RwSignal::new(HashMap::<Key, bool>::new());
    let ids = move || {
        nodes
            .data
            .with(|d| d.iter().flatten().map(|n| n.id).collect::<Vec<_>>())
    };
    view! {
        {move || nodes.error.get().map(|e| view! { <div class="banner">{e}</div> })}
        {move || nodes.loading().then(|| view! { <div class="card skel" style="height: 160px"></div> })}
        {move || nodes.data.with(|d| d.as_ref().is_some_and(Vec::is_empty)).then(|| view! {
            <div class="card empty">"No nodes yet. Pair one from the Pairing tab or the desktop app."</div>
        })}
        <For each=ids key=|id| *id let:id>
            {
                let node = Memo::new(move |_| nodes.data.with(|d| d.iter().flatten().find(|n| n.id == id).cloned()));
                let chans = move || node.with(|n| n.iter().flat_map(|n| n.channels.iter().map(|c| c.channel)).collect::<Vec<_>>());
                view! {
                    <section class="card rise">
                        <div class="card-head">
                            <h3>{move || node.get().map(|n| n.name)}</h3>
                            {move || node.get().map(|n| {
                                let on = format::is_online(n.last_seen, c.now.get());
                                if on {
                                    ui::status_pill(true, "Online", "").into_any()
                                } else {
                                    view! { <span class="pill bad"><span class="dot"></span>{format!("Offline · {}", format::ago(n.last_seen, c.now.get()))}</span> }.into_any()
                                }
                            })}
                        </div>
                        <div class="rows" style="border: 0">
                            <For each=chans key=|ch| *ch let:ch>
                                {
                                    let key = (id, ch);
                                    let item = Memo::new(move |_| nodes.data.with(|d| d.as_deref().and_then(|d| find(d, key))));
                                    let name = move || item.get().map(|(_, c)| c.name).unwrap_or_default();
                                    let on = Signal::derive(move || {
                                        shown.with(|m| m.get(&key).copied()).unwrap_or_else(|| item.get().is_some_and(|(_, ch)| ch.last_value == Some(1)))
                                    });
                                    let open = move |_| { c.history_sel.set(Some(key)); c.go(Route::History); };
                                    view! {
                                        <div class="row" style="padding-left: 0; padding-right: 0">
                                            <button class="link grow stack" style="gap: 2px; background: none; border: 0; text-align: left" on:click=open>
                                                <span class="title">{name}</span>
                                                <span class="faint small">{move || item.get().map(|(_, ch)| if ch.last_ts.is_some() { format::ago(ch.last_ts, c.now.get()) } else { "no reading yet".into() })}</span>
                                            </button>
                                            {move || match item.get() {
                                                Some((_, ch)) if ch.kind.is_output() => view! {
                                                    <Switch on=on label=name() on_toggle=move |v| set_output(key, name(), v, shown, nodes)/>
                                                }.into_any(),
                                                Some((_, ch)) => view! {
                                                    <span class="num" style="font-size: calc(var(--fs-body) + 4px); font-weight: 600">{format::value(ch.kind, ch.last_value)}</span>
                                                }.into_any(),
                                                None => ().into_any(),
                                            }}
                                        </div>
                                    }
                                }
                            </For>
                        </div>
                    </section>
                }
            }
        </For>
    }
}
