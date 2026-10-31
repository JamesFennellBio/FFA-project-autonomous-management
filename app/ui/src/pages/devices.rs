//! Devices (desktop): rename nodes and channels, unpair.

use std::collections::HashMap;

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api;
use crate::format;
use crate::model::NodeInfo;
use crate::ui::{self, Polled, Route, Tabs, ctx, poll};

#[component]
pub fn Devices() -> impl IntoView {
    let c = ctx();
    let nodes = poll(10_000, api::nodes);
    let filter = RwSignal::new("all".to_string());
    let ids = move || {
        let now = c.now.get_untracked();
        let f = filter.get();
        nodes.data.with(|d| {
            d.iter()
                .flatten()
                .filter(|n| match f.as_str() {
                    "online" => format::is_online(n.last_seen, now),
                    "offline" => !format::is_online(n.last_seen, now),
                    _ => true,
                })
                .map(|n| n.id)
                .collect::<Vec<_>>()
        })
    };
    view! {
        <header class="page-head rise">
            <div>
                <h1>{move || c.prefs.with(|p| p.mode.copy().devices)}</h1>
                <p>"Names show up everywhere, including the phone app and recipes."</p>
            </div>
            <button class="btn" on:click=move |_| c.go(Route::Pairing)>"Add " {move || c.prefs.with(|p| p.mode.copy().device)}</button>
        </header>
        <Tabs options=vec![("all".into(), "All".into()), ("online".into(), "Online".into()), ("offline".into(), "Offline".into())] selected=filter/>
        {move || nodes.error.get().map(|e| view! { <div class="banner">{e}</div> })}
        {move || nodes.loading().then(|| view! { <div class="card skel" style="height: 180px"></div> })}
        {move || (nodes.data.with(|d| d.is_some()) && ids().is_empty()).then(|| view! { <div class="card empty">"Nothing here."</div> })}
        <For each=ids key=|id| *id let:id>
            {
                let node = nodes.data.with_untracked(|d| d.iter().flatten().find(|n| n.id == id).cloned());
                node.map(|n| view! { <DeviceCard node=n nodes=nodes/> })
            }
        </For>
    }
}

#[component]
fn DeviceCard(node: NodeInfo, nodes: Polled<Vec<NodeInfo>>) -> impl IntoView {
    let c = ctx();
    let id = node.id;
    let name = RwSignal::new(node.name.clone());
    let ch_names = RwSignal::new(HashMap::<u8, String>::new());
    let (busy, set_busy) = signal(false);
    let original = node.name.clone();
    let save = move |_| {
        let (n, chans, orig) = (
            name.get_untracked(),
            ch_names.get_untracked(),
            original.clone(),
        );
        set_busy.set(true);
        spawn_local(async move {
            let mut r = Ok(());
            if n.trim() != orig && !n.trim().is_empty() {
                r = api::rename_node(id, n.trim()).await;
            }
            for (ch, cn) in chans {
                if r.is_ok() && !cn.trim().is_empty() {
                    r = api::rename_channel(id, ch, cn.trim()).await;
                }
            }
            match r {
                Ok(()) => {
                    ch_names.set(HashMap::new());
                    ui::toast("Saved");
                    nodes.refresh();
                }
                Err(e) => ui::toast_err(e),
            }
            set_busy.set(false);
        });
    };
    let unpair_name = node.name.clone();
    let unpair = move |_| {
        if !ui::confirm(&format!(
            "Unpair \"{unpair_name}\"?\n\nIt stops sending data until it is paired again. Its history is kept."
        )) {
            return;
        }
        ui::run(api::unpair_node(id), move || {
            ui::toast("Unpaired");
            nodes.refresh();
        });
    };
    let channels = node.channels.clone();
    view! {
        <section class="card rise stack">
            <div class="hstack" style="justify-content: space-between">
                <input class="field" style="font-size: var(--fs-section); font-weight: 500; height: 40px; flex: 1; max-width: 420px; border-color: transparent" aria-label="Name" prop:value=name on:input=move |e| name.set(event_target_value(&e))/>
                <div class="hstack">
                    <button class="btn primary" disabled=busy on:click=save>"Save"</button>
                    <button class="btn danger" on:click=unpair>"Unpair"</button>
                </div>
            </div>
            <div class="hstack small muted">
                <span class="pill">{format!("#{id}")}</span>
                <span class="pill">{node.transport.clone().unwrap_or_else(|| "—".into()).to_uppercase()}</span>
                <span class="fp">{node.fingerprint.clone()}</span>
                <span>"·"</span>
                <span>{format!("{} key exchanges", node.sessions)}</span>
                <span>"·"</span>
                <span>{move || format!("seen {}", format::ago(node.last_seen, c.now.get()))}</span>
            </div>
            <hr class="chalk-rule"/>
            <div class="rows">
                {channels.into_iter().map(|ch| {
                    let n = ch.channel;
                    view! {
                        <div class="row">
                            <span class="fp faint" style="width: 28px">{format!("{n}")}</span>
                            <span class="small muted" style="width: 110px">{ch.kind.label()}</span>
                            <input
                                class="field grow"
                                aria-label="Channel name"
                                prop:value=ch.name.clone()
                                on:input=move |e| ch_names.update(|m| { m.insert(n, event_target_value(&e)); })
                            />
                            <span class="mono small" style="width: 90px; text-align: right">{format::value(ch.kind, ch.last_value)}</span>
                        </div>
                    }
                }).collect_view()}
            </div>
        </section>
    }
}
