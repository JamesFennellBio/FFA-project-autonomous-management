//! Pairing. Desktop: the hub's one-time code, phone QR codes, approving
//! nodes (and phones that asked by address), paired phones. Phone: approving
//! nodes; linking the phone itself is `PhoneLink` (Settings, first run).

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api;
use crate::format;
use crate::model::{PairKind, PendingPair, PhoneTicket};
use crate::ui::{self, Icon, Polled, ctx, icon, poll};

#[component]
pub fn Pairing() -> impl IntoView {
    if ctx().phone {
        view! { <PhonePairing/> }.into_any()
    } else {
        view! { <DesktopPairing/> }.into_any()
    }
}

#[component]
fn DesktopPairing() -> impl IntoView {
    let c = ctx();
    let pending = poll(3000, api::pending);
    let phones = poll(10_000, api::phones);
    view! {
        <header class="page-head rise">
            <div>
                <h1>"Pairing"</h1>
                <p>"The one place you decide which devices to trust."</p>
            </div>
        </header>
        <div class="grid" style="grid-template-columns: repeat(auto-fit, minmax(320px, 1fr))">
            <section class="card stack rise"><AddHub/></section>
            <section class="card stack rise"><AddPhone phones=phones/></section>
        </div>

        <section class="stack">
            <h2>"Waiting for approval"</h2>
            <p class="muted small">
                "Hold a node's BOOT button for 3 seconds and it shows up here. Approve it only if the fingerprint matches the node's serial monitor."
            </p>
            {move || pending.data.with(|d| d.as_ref().is_some_and(Vec::is_empty)).then(|| view! {
                <div class="card empty">"Nothing is asking to pair right now."</div>
            })}
            <div class="rows" style:display=move || if pending.data.with(|d| d.as_ref().is_none_or(Vec::is_empty)) { "none" } else { "block" }>
                <For each=move || pending.data.get().unwrap_or_default() key=|p| p.public_key.clone() let:p>
                    <PendingRow p=p pending=pending/>
                </For>
            </div>
        </section>

        <section class="stack">
            <h2>"Paired phones"</h2>
            {move || phones.data.with(|d| d.as_ref().is_some_and(Vec::is_empty)).then(|| view! {
                <div class="card empty">"No phones yet. Use Add phone above."</div>
            })}
            <div class="rows" style:display=move || if phones.data.with(|d| d.as_ref().is_none_or(Vec::is_empty)) { "none" } else { "block" }>
                <For each=move || phones.data.get().unwrap_or_default() key=|p| p.id let:p>
                    {
                        let (id, name) = (p.id, p.name.clone());
                        view! {
                            <div class="row">
                                <span class="grow title">{p.name.clone()}</span>
                                <span class="fp muted">{p.fingerprint.clone()}</span>
                                <span class="faint small">{move || format!("paired {}", format::ago(Some(p.paired_at), c.now.get()))}</span>
                                <button class="btn sm danger" on:click=move |_| {
                                    if ui::confirm(&format!("Unpair {name}?\n\nIt stops working until it is paired again.")) {
                                        ui::run(api::unpair_phone(id), move || phones.refresh());
                                    }
                                }>"Unpair"</button>
                            </div>
                        }
                    }
                </For>
            </div>
        </section>

        <section class="card stack">
            <span class="eyebrow">"This PC"</span>
            <div class="hstack">
                <span class="muted small" style="width: 120px">"Fingerprint"</span>
                <span class="fp">{move || c.status.data.get().map(|s| s.pc_fingerprint)}</span>
            </div>
            <div class="hstack">
                <span class="muted small" style="width: 120px">"Key ID"</span>
                <span class="fp muted" style="word-break: break-all; flex: 1">{move || c.status.data.get().map(|s| s.pc_key_id)}</span>
                <button class="btn sm" on:click=move |_| ui::copy_text(c.status.data.get_untracked().map(|s| s.pc_key_id).unwrap_or_default())>"Copy"</button>
            </div>
            <p class="faint small">"Optional extra pin: put the key ID in the Pi's hub.toml as pc_key_id."</p>
        </section>
    }
}

#[component]
fn PendingRow(p: PendingPair, pending: Polled<Vec<PendingPair>>) -> impl IntoView {
    let c = ctx();
    let name = RwSignal::new(String::new());
    let (busy, set_busy) = signal(false);
    let key = p.public_key.clone();
    let key2 = key.clone();
    let what = match p.kind {
        PairKind::Node => "Data node",
        PairKind::Phone => "Phone app",
    };
    let placeholder = match p.kind {
        PairKind::Node => format!(
            "Name, e.g. {}",
            c.prefs.with_untracked(|p| p.mode.copy().example_node)
        ),
        PairKind::Phone => "Name, e.g. Sam's phone".into(),
    };
    view! {
        <div class="row">
            <div class="stack" style="gap: 2px; min-width: 160px">
                <span class="title">{what}</span>
                <span class="faint small">
                    {p.transport.clone().map(|t| t.to_uppercase()).unwrap_or_else(|| "LAN".into())}
                    " · "
                    {move || format::ago(Some(p.seen_at), c.now.get())}
                </span>
            </div>
            <span class="fp big">{p.fingerprint.clone()}</span>
            <input class="field grow" placeholder=placeholder aria-label="Name" prop:value=name on:input=move |e| name.set(event_target_value(&e))/>
            <button class="btn primary" disabled=busy on:click=move |_| {
                let (key, n) = (key.clone(), name.get_untracked());
                set_busy.set(true);
                spawn_local(async move {
                    let n = (!n.trim().is_empty()).then(|| n.trim().to_string());
                    match api::approve(&key, n).await {
                        Ok(()) => { ui::toast("Approved. It pairs the next time it asks (a few seconds)."); pending.refresh(); }
                        Err(e) => ui::toast_err(e),
                    }
                    set_busy.set(false);
                });
            }>"Approve"</button>
            {(!c.phone).then(|| view! {
                <button class="btn" on:click=move |_| {
                    let k = key2.clone();
                    ui::run(async move { api::reject(&k).await }, move || pending.refresh());
                }>"Dismiss"</button>
            })}
        </div>
    }
}

/// "1234 5678 9012" or pasted text -> "1234-5678-9012" as it's typed.
pub fn format_code(s: &str) -> String {
    let digits: String = s.chars().filter(char::is_ascii_digit).take(12).collect();
    digits
        .as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("-")
}

/// The hub's one-time code (SPAKE2, uplink/src/pake.rs). Used on the Pairing
/// page and as the last first-run step.
#[component]
pub fn AddHub(#[prop(optional)] on_done: Option<Callback<()>>) -> impl IntoView {
    let c = ctx();
    let address = RwSignal::new(String::new());
    let code = RwSignal::new(String::new());
    let (busy, set_busy) = signal(false);
    let (done, set_done) = signal(None::<String>);
    let (err, set_err) = signal(None::<String>);
    let paired = move || {
        c.status
            .data
            .with(|s| s.as_ref().is_some_and(|s| s.hub.paired))
    };
    let ready = move || {
        !address.get().trim().is_empty()
            && code.get().chars().filter(char::is_ascii_digit).count() == 12
    };
    let submit = move || {
        if paired()
            && done.get_untracked().is_none()
            && !ui::confirm(
                "Replace the paired hub?\n\nThe current one stops sending data to this PC.",
            )
        {
            return;
        }
        set_busy.set(true);
        set_err.set(None);
        let (a, k) = (address.get_untracked(), code.get_untracked());
        spawn_local(async move {
            match api::pair_hub(a.trim(), &k).await {
                Ok(fp) => {
                    set_done.set(Some(fp));
                    code.set(String::new());
                    c.status.refresh();
                    if let Some(cb) = on_done {
                        cb.run(());
                    }
                }
                Err(e) => set_err.set(Some(e)),
            }
            set_busy.set(false);
        });
    };
    view! {
        <div class="hstack" style="justify-content: space-between">
            <h3>{move || if paired() { "Replace hub" } else { "Add hub" }}</h3>
            {icon(Icon::Hex, 18)}
        </div>
        {move || match done.get() {
            Some(fp) => view! {
                <div class="hstack">
                    <div class="check">{icon(Icon::Check, 24)}</div>
                    <div class="stack" style="gap: 2px">
                        <span class="title">"Hub paired"</span>
                        <span class="fp muted">{fp}</span>
                        <span class="faint small">"It connects within a few seconds."</span>
                    </div>
                </div>
            }.into_any(),
            None => view! {
                <p class="muted small">
                    "On the Pi run "<code>"sudo -u farmlab hub-node pair"</code>". It prints its address and a 12-digit code that only works once, for 10 minutes. Only that Pi can finish pairing with it."
                </p>
                <label class="form">"Pi address"
                    <input class="field mono" placeholder="192.168.1.20" autocomplete="off" prop:value=address on:input=move |e| address.set(event_target_value(&e))/>
                </label>
                <label class="form">"Pairing code"
                    <input
                        class="field code-input"
                        inputmode="numeric"
                        autocomplete="one-time-code"
                        placeholder="0000-0000-0000"
                        prop:value=code
                        on:input=move |e| code.set(format_code(&event_target_value(&e)))
                        on:keydown=move |e: web_sys::KeyboardEvent| if e.key() == "Enter" && ready() { submit() }
                    />
                </label>
                {move || err.get().map(|e| view! { <div class="error-text">{e}</div> })}
                <div class="hstack">
                    <button class="btn primary" disabled=move || busy.get() || !ready() on:click=move |_| submit()>
                        {move || if busy.get() { "Pairing…" } else { "Pair hub" }}
                    </button>
                </div>
            }.into_any(),
        }}
    }
}

/// One-time phone QR code: works once, for 5 minutes. Watches the phone
/// list to show when the phone has paired.
#[component]
fn AddPhone(phones: Polled<Vec<crate::model::Phone>>) -> impl IntoView {
    let c = ctx();
    let ticket = RwSignal::new(None::<PhoneTicket>);
    let (busy, set_busy) = signal(false);
    let online = move || {
        c.status
            .data
            .with(|s| s.as_ref().is_some_and(|s| s.hub.connected))
    };
    let paired = move || {
        let t = ticket.get()?;
        phones.data.with(|p| {
            p.as_ref()
                .and_then(|p| p.iter().find(|x| x.id == t.phone_id).cloned())
        })
    };
    let left = move || ticket.get().map_or(0, |t| t.expires - c.now.get());
    // Look for the new phone every 2 s while a code is open.
    Effect::new(move |_| {
        c.now.track();
        if ticket.with(Option::is_some)
            && paired().is_none()
            && left() > 0
            && c.now.get_untracked() % 2 == 0
        {
            phones.refresh();
        }
    });
    let make = move |_| {
        set_busy.set(true);
        spawn_local(async move {
            match api::phone_ticket().await {
                Ok(t) => ticket.set(Some(t)),
                Err(e) => ui::toast_err(e),
            }
            set_busy.set(false);
        });
    };
    view! {
        <div class="hstack" style="justify-content: space-between">
            <h3>"Add phone"</h3>
            {icon(Icon::Qr, 18)}
        </div>
        {move || match (ticket.get(), paired()) {
            (None, _) => view! {
                <p class="muted small">"Shows a QR code for the phone app to scan. No approval click needed: the code works once, for 5 minutes."</p>
            }.into_any(),
            (Some(_), Some(p)) => view! {
                <div class="hstack">
                    <div class="check">{icon(Icon::Check, 24)}</div>
                    <div class="stack" style="gap: 2px">
                        <span class="title">{format!("{} is paired", p.name)}</span>
                        <span class="fp muted">{p.fingerprint}</span>
                    </div>
                </div>
            }.into_any(),
            (Some(t), None) if left() > 0 => view! {
                <div class="hstack" style="align-items: flex-start; gap: 20px">
                    <div class="qr" inner_html=t.svg.clone()></div>
                    <div class="stack" style="gap: 8px; flex: 1; min-width: 160px">
                        <span class="eyebrow">"Waiting for the phone"</span>
                        <span class="num mono" style="font-size: var(--fs-section)">
                            {move || { let l = left().max(0); format!("{}:{:02}", l / 60, l % 60) }}
                        </span>
                        <span class="muted small">"The phone should show hub "<span class="fp">{t.hub_fingerprint.clone()}</span>"."</span>
                        <button class="btn sm" on:click={
                            let uri = t.uri.clone();
                            move |_| ui::copy_text(uri.clone())
                        }>"Copy link"</button>
                    </div>
                </div>
            }.into_any(),
            (Some(_), None) => view! { <p class="muted small">"That code expired."</p> }.into_any(),
        }}
        <div class="hstack">
            <button class="btn primary" disabled=move || busy.get() || !online() on:click=make>
                {move || if ticket.with(Option::is_some) { "New code" } else { "Show QR code" }}
            </button>
            {move || (!online()).then(|| view! { <span class="faint small">"The hub must be online."</span> })}
        </div>
    }
}

/// Phone: approve nodes asking to pair (the PC picks the id, so it must be on).
#[component]
fn PhonePairing() -> impl IntoView {
    let pending = poll(4000, api::pending);
    view! {
        <section class="card stack">
            <h3>"Pair a node"</h3>
            <p class="muted small">
                "Hold the node's BOOT button for 3 seconds. It shows up here within a few seconds. Check that the fingerprint matches the node's serial monitor, then approve it. The PC must be running."
            </p>
        </section>
        {move || pending.error.get().map(|e| view! { <div class="banner">{e}</div> })}
        {move || pending.data.with(|d| d.as_ref().is_some_and(Vec::is_empty)).then(|| view! {
            <div class="card empty">"No nodes are asking to pair right now."</div>
        })}
        <For each=move || pending.data.get().unwrap_or_default() key=|p| p.public_key.clone() let:p>
            <div class="rows"><PendingRow p=p pending=pending/></div>
        </For>
    }
}

/// Links this phone to a hub: scan the desktop's QR code (camera on
/// Android / iOS), paste its link, or pair by address and approve on the
/// desktop.
#[component]
pub fn PhoneLink(#[prop(optional)] on_done: Option<Callback<()>>) -> impl IntoView {
    let state = RwSignal::new(None::<api::PhoneState>);
    let (busy, set_busy) = signal(None::<&'static str>);
    let (err, set_err) = signal(None::<String>);
    let (can_scan, set_can_scan) = signal(false);
    let link = RwSignal::new(String::new());
    let addr = RwSignal::new(String::new());
    let key = RwSignal::new(String::new());
    spawn_local(async move {
        state.try_set(api::phone_state().await.ok());
        set_can_scan.try_set(api::can_scan().await);
    });
    let finish = move |r: api::Res<api::PhoneState>| match r {
        Ok(s) => {
            state.set(Some(s));
            ui::toast("Paired with the hub");
            if let Some(cb) = on_done {
                cb.run(());
            }
        }
        Err(e) => set_err.set(Some(e)),
    };
    let pair_uri = move |uri: String| {
        set_busy.set(Some("Pairing…"));
        set_err.set(None);
        spawn_local(async move {
            finish(api::phone_pair_qr(uri.trim()).await);
            set_busy.set(None);
        });
    };
    let scan = move |_| {
        set_err.set(None);
        spawn_local(async move {
            match api::scan_qr().await {
                Ok(uri) => pair_uri(uri),
                Err(e) => set_err.set(Some(format!("Camera: {e}"))),
            }
        });
    };
    let manual = move |_| {
        set_busy.set(Some("Waiting for approval on the desktop…"));
        set_err.set(None);
        let (a, k) = (addr.get_untracked(), key.get_untracked());
        spawn_local(async move {
            let k = (!k.trim().is_empty()).then(|| k.trim().to_string());
            finish(api::phone_pair_manual(a.trim(), k).await);
            set_busy.set(None);
        });
    };
    view! {
        {move || state.get().filter(|s| s.paired).map(|s| view! {
            <div class="hstack">
                <div class="check">{icon(Icon::Check, 24)}</div>
                <div class="stack" style="gap: 2px">
                    <span class="title">"Paired with the hub"</span>
                    <span class="fp muted">{s.hub_fingerprint.unwrap_or_default()}</span>
                    <span class="faint small">{s.hub_addr.unwrap_or_default()}</span>
                </div>
            </div>
        })}
        {move || state.get().is_none_or(|s| !s.paired).then(|| view! {
            <p class="muted small">"On the desktop app open Pairing → Add phone, then scan the code it shows."</p>
            {move || can_scan.get().then(|| view! {
                <button class="btn primary" style="height: 56px" disabled=move || busy.get().is_some() on:click=scan>
                    {icon(Icon::Camera, 20)}"Scan pairing code"
                </button>
            })}
            <label class="form">"Or paste the pairing link"
                <input class="field mono" placeholder="farmlab://pair?…" prop:value=link on:input=move |e| link.set(event_target_value(&e))/>
            </label>
            <button class="btn" disabled=move || busy.get().is_some() || link.get().trim().is_empty() on:click=move |_| pair_uri(link.get_untracked())>"Pair with link"</button>
            <details>
                <summary class="muted small" style="cursor: pointer">"Pair by address instead"</summary>
                <div class="stack" style="margin-top: 12px">
                    <label class="form">"Hub address"
                        <input class="field mono" placeholder="192.168.1.20" prop:value=addr on:input=move |e| addr.set(event_target_value(&e))/>
                    </label>
                    <label class="form">"Hub key ID (optional, refuses any other hub)"
                        <input class="field mono" placeholder="64 hex characters" prop:value=key on:input=move |e| key.set(event_target_value(&e))/>
                    </label>
                    <button class="btn" disabled=move || busy.get().is_some() || addr.get().trim().is_empty() on:click=manual>"Ask to pair"</button>
                    <p class="faint small">"Then approve “Phone app” on the desktop's Pairing page within 2 minutes, after checking the fingerprint."</p>
                </div>
            </details>
        })}
        {move || busy.get().map(|b| view! { <div class="muted small">{b}</div> })}
        {move || err.get().map(|e| view! { <div class="error-text">{e}</div> })}
        {move || state.get().map(|s| view! {
            <p class="faint small">"This phone's fingerprint: "<span class="fp">{s.phone_fingerprint}</span></p>
        })}
    }
}

#[cfg(test)]
mod tests {
    use super::format_code;

    #[test]
    fn codes_are_grouped_as_typed() {
        assert_eq!(format_code("4821"), "4821");
        assert_eq!(format_code("48217"), "4821-7");
        assert_eq!(format_code("4821 7730 1956 99"), "4821-7730-1956");
        assert_eq!(format_code("a-b"), "");
    }
}
