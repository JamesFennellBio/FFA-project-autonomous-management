//! Small shared pieces: app context, polling, toasts, icons, switch, tabs.

use std::cell::Cell;
use std::future::Future;
use std::rc::Rc;
use std::time::Duration;

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{self, Res};
use crate::model::Status;
use crate::prefs::Prefs;

// ---- app context ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Overview,
    Devices,
    History,
    Automation,
    Alerts,
    Pairing,
    Settings,
}

impl Route {
    pub const DESKTOP: [Route; 7] = [
        Route::Overview,
        Route::Devices,
        Route::History,
        Route::Automation,
        Route::Alerts,
        Route::Pairing,
        Route::Settings,
    ];
    /// Phone tab bar. History opens from a reading; Devices are in Live.
    pub const PHONE: [Route; 5] = [
        Route::Overview,
        Route::Alerts,
        Route::Automation,
        Route::Pairing,
        Route::Settings,
    ];

    pub fn label(self, prefs: &Prefs, phone: bool) -> &'static str {
        match self {
            Route::Overview if phone => "Live",
            Route::Overview => "Overview",
            Route::Devices => prefs.mode.copy().devices,
            Route::History => "History",
            Route::Automation if phone => "Recipes",
            Route::Automation => "Automation",
            Route::Alerts => "Alerts",
            Route::Pairing => "Pairing",
            Route::Settings => "Settings",
        }
    }
    pub fn icon(self) -> Icon {
        match self {
            Route::Overview => Icon::Grid,
            Route::Devices => Icon::Chip,
            Route::History => Icon::Chart,
            Route::Automation => Icon::Bolt,
            Route::Alerts => Icon::Bell,
            Route::Pairing => Icon::Link,
            Route::Settings => Icon::Sliders,
        }
    }
    /// Second key of the "G then key" jump shortcut (Linear).
    pub fn key(self) -> char {
        match self {
            Route::Overview => 'O',
            Route::Devices => 'D',
            Route::History => 'H',
            Route::Automation => 'A',
            Route::Alerts => 'L',
            Route::Pairing => 'P',
            Route::Settings => 'S',
        }
    }
}

#[derive(Clone, Copy)]
pub struct Ctx {
    pub prefs: RwSignal<Prefs>,
    pub phone: bool,
    pub route: RwSignal<Route>,
    /// Channel the History page shows.
    pub history_sel: RwSignal<Option<(u16, u8)>>,
    /// Recipe being edited (Some(0) = a new one).
    pub editing: RwSignal<Option<u32>>,
    pub toasts: RwSignal<Vec<Toast>>,
    /// Command menu: Some(opened_by_keyboard).
    pub cmdk: RwSignal<Option<bool>>,
    /// Ticks every second, for "12s ago" texts.
    pub now: RwSignal<i64>,
    pub status: Polled<Status>,
    pub open_alerts: RwSignal<usize>,
    pub pending: RwSignal<usize>,
    pub limits: RwSignal<Option<api::LimitsReport>>,
}

pub fn ctx() -> Ctx {
    expect_context::<Ctx>()
}

impl Ctx {
    pub fn go(&self, r: Route) {
        self.editing.set(None);
        self.route.set(r);
    }

    /// Saves preferences (and applies the RAM / CPU limits on the Tauri side).
    pub fn save_prefs(&self) {
        let p = self.prefs.get_untracked();
        let limits = self.limits;
        spawn_local(async move {
            match api::save_prefs(&p).await {
                Ok(r) => limits.set(Some(r)),
                Err(e) => toast_err(format!("Couldn't save settings: {e}")),
            }
        });
    }

    pub fn update_prefs(&self, f: impl FnOnce(&mut Prefs)) {
        self.prefs.update(f);
        self.save_prefs();
    }
}

// ---- polling ----

/// Data that refreshes on its own. Copy, like signals.
pub struct Polled<T: Send + Sync + 'static> {
    pub data: RwSignal<Option<T>>,
    pub error: RwSignal<Option<String>>,
    tick: RwSignal<u64>,
}

impl<T: Send + Sync + 'static> Clone for Polled<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: Send + Sync + 'static> Copy for Polled<T> {}

impl<T: Send + Sync + 'static> Polled<T> {
    /// Empty and never refreshing; only used before the context exists.
    pub fn placeholder() -> Self {
        Self {
            data: RwSignal::new(None),
            error: RwSignal::new(None),
            tick: RwSignal::new(0),
        }
    }
    pub fn refresh(&self) {
        self.tick.update(|t| *t += 1);
    }
    pub fn loading(&self) -> bool {
        self.data.with(Option::is_none) && self.error.with(Option::is_none)
    }
}

/// Calls `f` now and then every `base_ms` (stretched by the CPU limit and
/// Lite, see Prefs::poll_ms). A call still running is never doubled up.
pub fn poll<T, F, Fut>(base_ms: u32, f: F) -> Polled<T>
where
    T: Send + Sync + 'static,
    F: Fn() -> Fut + 'static,
    Fut: Future<Output = Res<T>> + 'static,
{
    let p = Polled {
        data: RwSignal::new(None),
        error: RwSignal::new(None),
        tick: RwSignal::new(0),
    };
    let f = Rc::new(f);
    let busy = Rc::new(Cell::new(false));
    Effect::new(move |_| {
        p.tick.track();
        if busy.get() {
            return;
        }
        busy.set(true);
        let (f, busy) = (f.clone(), busy.clone());
        spawn_local(async move {
            match f().await {
                Ok(v) => {
                    p.data.try_set(Some(v));
                    p.error.try_set(None);
                }
                Err(e) => {
                    p.error.try_set(Some(e));
                }
            }
            busy.set(false);
        });
    });
    let ms =
        use_context::<Ctx>().map_or(base_ms, |c| c.prefs.with_untracked(|p| p.poll_ms(base_ms)));
    if let Ok(h) = set_interval_with_handle(move || p.refresh(), Duration::from_millis(ms.into())) {
        on_cleanup(move || h.clear());
    }
    p
}

/// Runs an action, toasting its error. Returns nothing; refresh in `then`.
pub fn run<Fut>(fut: Fut, then: impl FnOnce() + 'static)
where
    Fut: Future<Output = Res<()>> + 'static,
{
    spawn_local(async move {
        match fut.await {
            Ok(()) => then(),
            Err(e) => toast_err(e),
        }
    });
}

pub fn confirm(msg: &str) -> bool {
    window().confirm_with_message(msg).unwrap_or(false)
}

pub fn copy_text(text: String) {
    let clip = window().navigator().clipboard();
    let p = clip.write_text(&text);
    spawn_local(async move {
        if wasm_bindgen_futures::JsFuture::from(p).await.is_ok() {
            toast("Copied");
        }
    });
}

// ---- toasts (Sonner): stack, fan out on hover, 4 s, Undo ----

#[derive(Clone)]
pub struct Toast {
    pub id: u32,
    pub text: String,
    pub bad: bool,
    pub undo: Option<Callback<()>>,
}

thread_local! {
    static NEXT_TOAST: Cell<u32> = const { Cell::new(1) };
    static TOASTS_HOVERED: Cell<bool> = const { Cell::new(false) };
}

const TOAST_MS: u64 = 4000;

fn push(text: String, bad: bool, undo: Option<Callback<()>>) {
    let Some(c) = use_context::<Ctx>() else {
        return;
    };
    let id = NEXT_TOAST.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    });
    c.toasts.update(|t| {
        t.push(Toast {
            id,
            text,
            bad,
            undo,
        });
        if t.len() > 5 {
            t.remove(0);
        }
    });
    dismiss_later(c.toasts, id);
}

fn dismiss_later(toasts: RwSignal<Vec<Toast>>, id: u32) {
    set_timeout(
        move || {
            // The timer pauses while the stack is hovered.
            if TOASTS_HOVERED.with(Cell::get) {
                dismiss_later(toasts, id);
            } else {
                toasts.try_update(|t| t.retain(|x| x.id != id));
            }
        },
        Duration::from_millis(TOAST_MS),
    );
}

pub fn toast(text: impl Into<String>) {
    push(text.into(), false, None);
}

pub fn toast_err(text: impl Into<String>) {
    push(text.into(), true, None);
}

pub fn toast_undo(text: impl Into<String>, undo: Callback<()>) {
    push(text.into(), false, Some(undo));
}

#[component]
pub fn Toasts() -> impl IntoView {
    let c = ctx();
    let (hover, set_hover) = signal(false);
    view! {
        <div
            class="toasts"
            role="status"
            aria-live="polite"
            on:mouseenter=move |_| {
                set_hover.set(true);
                TOASTS_HOVERED.with(|h| h.set(true));
            }
            on:mouseleave=move |_| {
                set_hover.set(false);
                TOASTS_HOVERED.with(|h| h.set(false));
            }
        >
            <For each=move || c.toasts.get() key=|t| t.id let:t>
                {
                    let id = t.id;
                    // Newest in front; older ones tuck behind (-14 px, -5 %), or
                    // fan out on hover.
                    let style = move || {
                        let list = c.toasts.get();
                        let n = list.len();
                        let pos = list.iter().position(|x| x.id == id).unwrap_or(0);
                        let depth = (n - 1 - pos) as f64;
                        if hover.get() {
                            format!("transform: translateY(-{}px); opacity: 1; z-index: {}", depth * 62.0, 100 - depth as i32)
                        } else {
                            let hidden = if depth >= 3.0 { 0 } else { 1 };
                            format!(
                                "transform: translateY(-{}px) scale({}); opacity: {hidden}; z-index: {}",
                                depth * 14.0,
                                1.0 - depth * 0.05,
                                100 - depth as i32
                            )
                        }
                    };
                    let undo = t.undo;
                    view! {
                        <div class="toast" class:bad=t.bad style=style>
                            <span class="dot"></span>
                            <span class="grow">{t.text.clone()}</span>
                            {undo.map(|u| view! {
                                <button class="btn sm" on:click=move |_| {
                                    u.run(());
                                    c.toasts.update(|v| v.retain(|x| x.id != id));
                                }>"Undo"</button>
                            })}
                        </div>
                    }
                }
            </For>
        </div>
    }
}

// ---- icons (inline SVG, 24 px grid, 1.8 stroke) ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Grid,
    Chip,
    Chart,
    Bolt,
    Bell,
    Link,
    Sliders,
    Search,
    Plus,
    Check,
    Back,
    Qr,
    Camera,
    Sun,
    Moon,
    Hex,
}

fn icon_paths(i: Icon) -> &'static str {
    match i {
        Icon::Grid => {
            r#"<rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/><rect x="3" y="14" width="7" height="7" rx="1.5"/><rect x="14" y="14" width="7" height="7" rx="1.5"/>"#
        }
        Icon::Chip => {
            r#"<rect x="5" y="5" width="14" height="14" rx="2"/><path d="M9 1v4M15 1v4M9 19v4M15 19v4M1 9h4M1 15h4M19 9h4M19 15h4"/>"#
        }
        Icon::Chart => r#"<path d="M3 3v18h18"/><path d="M7 15l4-4 3 3 5-6"/>"#,
        Icon::Bolt => r#"<path d="M13 2L3 14h8l-1 8 10-12h-8l1-8z"/>"#,
        Icon::Bell => {
            r#"<path d="M18 8a6 6 0 10-12 0c0 7-3 9-3 9h18s-3-2-3-9"/><path d="M13.7 21a2 2 0 01-3.4 0"/>"#
        }
        Icon::Link => {
            r#"<path d="M10 13a5 5 0 007.5.5l3-3a5 5 0 00-7-7l-1.7 1.7"/><path d="M14 11a5 5 0 00-7.5-.5l-3 3a5 5 0 007 7l1.7-1.7"/>"#
        }
        Icon::Sliders => {
            r#"<path d="M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3M1 14h6M9 8h6M17 16h6"/>"#
        }
        Icon::Search => r#"<circle cx="11" cy="11" r="7"/><path d="M20 20l-3.5-3.5"/>"#,
        Icon::Plus => r#"<path d="M12 5v14M5 12h14"/>"#,
        Icon::Check => r#"<path d="M5 12.5l4.5 4.5L19 7.5"/>"#,
        Icon::Back => r#"<path d="M15 18l-6-6 6-6"/>"#,
        Icon::Qr => {
            r#"<rect x="3" y="3" width="7" height="7" rx="1"/><rect x="14" y="3" width="7" height="7" rx="1"/><rect x="3" y="14" width="7" height="7" rx="1"/><path d="M14 14h3v3h-3zM20 14v.01M14 20h.01M17 20h4v-3"/>"#
        }
        Icon::Camera => {
            r#"<path d="M4 7h3l2-3h6l2 3h3a1 1 0 011 1v11a1 1 0 01-1 1H4a1 1 0 01-1-1V8a1 1 0 011-1z"/><circle cx="12" cy="13" r="4"/>"#
        }
        Icon::Sun => {
            r#"<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/>"#
        }
        Icon::Moon => r#"<path d="M21 12.8A9 9 0 1111.2 3a7 7 0 009.8 9.8z"/>"#,
        Icon::Hex => r#"<path d="M12 2l8.7 5v10L12 22l-8.7-5V7z"/>"#,
    }
}

pub fn icon(i: Icon, size: u32) -> impl IntoView {
    view! {
        <svg
            width=size
            height=size
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="1.8"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
            inner_html=icon_paths(i)
        ></svg>
    }
}

// ---- controls ----

/// On/off switch. `label` is read by screen readers.
#[component]
pub fn Switch(
    #[prop(into)] on: Signal<bool>,
    #[prop(into)] label: String,
    #[prop(optional, into)] disabled: Signal<bool>,
    on_toggle: impl Fn(bool) + 'static,
) -> impl IntoView {
    view! {
        <button
            type="button"
            role="switch"
            class="sw"
            class:on=move || on.get()
            aria-checked=move || on.get().to_string()
            aria-label=label
            disabled=move || disabled.get()
            on:click=move |_| on_toggle(!on.get_untracked())
        ></button>
    }
}

/// Text tabs with a hover highlight that follows the pointer and an
/// underline that slides to the active tab (Vercel). Boomer Mode turns them
/// into big buttons (CSS).
#[component]
pub fn Tabs(options: Vec<(String, String)>, selected: RwSignal<String>) -> impl IntoView {
    let refs: Vec<NodeRef<html::Button>> = options.iter().map(|_| NodeRef::new()).collect();
    let (ul, set_ul) = signal((0.0f64, 0.0f64));
    let (hl, set_hl) = signal(None::<(f64, f64)>);
    let keys: Vec<String> = options.iter().map(|o| o.0.clone()).collect();
    {
        let refs = refs.clone();
        Effect::new(move |_| {
            let sel = selected.get();
            let i = keys.iter().position(|k| *k == sel).unwrap_or(0);
            let r = refs[i];
            request_animation_frame(move || {
                if let Some(b) = r.get_untracked() {
                    set_ul.set((f64::from(b.offset_left()), f64::from(b.offset_width())));
                }
            });
        });
    }
    let buttons = options
        .into_iter()
        .zip(refs)
        .map(|((key, label), r)| {
            let k = key.clone();
            let click_key = key.clone();
            view! {
                <button
                    type="button"
                    role="tab"
                    class="tab"
                    node_ref=r
                    class:on=move || selected.get() == k
                    aria-selected=move || (selected.get() == key).to_string()
                    on:mouseenter=move |_| {
                        if let Some(b) = r.get_untracked() {
                            set_hl.set(Some((f64::from(b.offset_left()), f64::from(b.offset_width()))));
                        }
                    }
                    on:click=move |_| selected.set(click_key.clone())
                >
                    {label}
                </button>
            }
        })
        .collect_view();
    view! {
        <div class="tabs" role="tablist" on:mouseleave=move |_| set_hl.set(None)>
            <span
                class="tab-hl"
                style=move || match hl.get() {
                    Some((x, w)) => format!("transform: translateX({x}px); width: {w}px; opacity: 1"),
                    None => {
                        let (x, w) = ul.get();
                        format!("transform: translateX({x}px); width: {w}px; opacity: 0")
                    }
                }
            ></span>
            {buttons}
            <span
                class="tab-ul"
                style=move || {
                    let (x, w) = ul.get();
                    format!("transform: translateX({x}px); width: {w}px")
                }
            ></span>
        </div>
    }
}

pub fn status_pill(ok: bool, ok_text: &'static str, bad_text: &'static str) -> impl IntoView {
    view! {
        <span class=if ok { "pill ok" } else { "pill bad" }>
            <span class="dot"></span>
            {if ok { ok_text } else { bad_text }}
        </span>
    }
}

/// A sparkline path for `values` in an 88x28 box.
pub fn spark_path(values: &[f64]) -> String {
    if values.len() < 2 {
        return String::new();
    }
    let (lo, hi) = values
        .iter()
        .fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
    let span = if hi - lo < 1e-9 { 1.0 } else { hi - lo };
    let n = values.len() - 1;
    values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let x = i as f64 / n as f64 * 86.0 + 1.0;
            let y = 26.0 - (v - lo) / span * 24.0;
            format!("{}{x:.1} {y:.1}", if i == 0 { "M" } else { "L" })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spark_fits_its_box() {
        assert_eq!(spark_path(&[1.0]), "");
        let p = spark_path(&[0.0, 5.0, 10.0]);
        assert!(p.starts_with("M1.0 26.0"));
        assert!(p.ends_with("L87.0 2.0"));
        // Flat lines don't divide by zero.
        assert!(spark_path(&[3.0, 3.0]).contains("26.0"));
    }
}
