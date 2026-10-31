//! Alerts. The desktop keeps acknowledged ones; the phone shows the hub's
//! last 30 days.

use leptos::prelude::*;

use crate::api;
use crate::format;
use crate::ui::{self, Tabs, ctx, poll};

#[component]
pub fn Alerts() -> impl IntoView {
    let c = ctx();
    let which = RwSignal::new("open".to_string());
    let list = poll(6000, move || {
        let open = which.get_untracked() == "open" && !ctx_phone();
        api::alerts(open)
    });
    Effect::new(move |_| {
        which.track();
        list.refresh();
    });
    let ack = move |id: i64| {
        ui::run(api::ack_alert(id), move || list.refresh());
    };
    view! {
        {(!c.phone).then(|| view! {
            <header class="page-head rise">
                <div>
                    <h1>"Alerts"</h1>
                    <p>"From nodes and recipes. Acknowledge them once dealt with."</p>
                </div>
            </header>
            <Tabs options=vec![("open".into(), "Open".into()), ("all".into(), "All".into())] selected=which/>
        })}
        {move || list.error.get().map(|e| view! { <div class="banner">{e}</div> })}
        {move || list.loading().then(|| view! { <div class="card skel" style="height: 120px"></div> })}
        {move || list.data.with(|d| d.as_ref().is_some_and(Vec::is_empty)).then(|| view! {
            <div class="card empty">{ui::icon(ui::Icon::Check, 24)}"No alerts."</div>
        })}
        <div class="rows" style:display=move || if list.data.with(|d| d.as_ref().is_none_or(Vec::is_empty)) { "none" } else { "block" }>
            <For each=move || list.data.get().unwrap_or_default() key=|a| (a.id, a.acked) let:a>
                {
                    let id = a.id;
                    view! {
                        <div class="row rise" class:dim=a.acked>
                            <span class=if a.acked { "pill" } else { "pill bad" }><span class="dot"></span>{format::alert_source(&a)}</span>
                            <span class="grow">{a.message.clone()}</span>
                            <span class="faint small" style="white-space: nowrap">{move || format::ago(Some(a.ts), c.now.get())}</span>
                            {(!a.acked && !c.phone).then(|| view! {
                                <button class="btn sm" on:click=move |_| ack(id)>"Acknowledge"</button>
                            })}
                        </div>
                    }
                }
            </For>
        </div>
        {c.phone.then(|| view! { <p class="faint small">"Acknowledge alerts and see older ones in the desktop app."</p> })}
    }
}

fn ctx_phone() -> bool {
    crate::api::backend().is_phone()
}
