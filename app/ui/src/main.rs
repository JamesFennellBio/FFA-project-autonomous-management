//! Farm & Lab app UI: Leptos, compiled to WASM. The same build runs in the
//! desktop app and the phone app (Tauri), and in a plain browser with sample
//! data (`trunk serve`; add `?phone` for the phone layout).

mod api;
mod app;
mod blockly;
mod chart;
mod format;
mod model;
mod pages;
mod prefs;
mod recipe_edit;
mod ui;

use leptos::prelude::*;

fn main() {
    console_error_panic_hook::set_once();
    // Leptos's own executor starts with mount_to_body, so this first task
    // (which decides what to mount) runs on wasm-bindgen's.
    wasm_bindgen_futures::spawn_local(async {
        let backend = if api::has_tauri() {
            match api::invoke::<String>("platform", serde_json::json!({}))
                .await
                .as_deref()
            {
                Ok("phone") => api::Backend::Phone,
                _ => api::Backend::Desktop,
            }
        } else {
            let phone = window()
                .location()
                .search()
                .unwrap_or_default()
                .contains("phone");
            api::Backend::Demo { phone }
        };
        api::set_backend(backend);
        let mut prefs = api::load_prefs().await.ok().flatten().unwrap_or_default();
        let mut route = ui::Route::Overview;
        let mut editing = None;
        if let api::Backend::Demo { .. } = backend {
            editing = preview_params(&mut prefs, &mut route);
        }
        leptos::mount::mount_to_body(
            move || view! { <app::App initial=prefs.clone() route=route editing=editing/> },
        );
    });
}

/// Browser preview only: `?page=history&mode=lab&theme=light&boomer&sun&lite`
/// (and `edit=<recipe id>`, 0 = new) opens any screen in any look, for
/// checking screens against the wireframes. Returns the recipe to edit.
fn preview_params(p: &mut prefs::Prefs, route: &mut ui::Route) -> Option<u32> {
    let q = window().location().search().unwrap_or_default();
    let q = q.trim_start_matches('?');
    if q.is_empty() {
        return None;
    }
    let get = |k: &str| {
        q.split('&')
            .find_map(|kv| kv.strip_prefix(k).and_then(|v| v.strip_prefix('=')))
    };
    let has = |k: &str| {
        q.split('&')
            .any(|kv| kv == k || kv.starts_with(&format!("{k}=")))
    };
    if let Some(page) = get("page") {
        p.setup_done = true;
        *route = match page {
            "devices" => ui::Route::Devices,
            "history" => ui::Route::History,
            "automation" => ui::Route::Automation,
            "alerts" => ui::Route::Alerts,
            "pairing" => ui::Route::Pairing,
            "settings" => ui::Route::Settings,
            _ => ui::Route::Overview,
        };
    }
    if has("wizard") {
        p.setup_done = false;
    }
    match get("mode") {
        Some("lab") => p.mode = prefs::Mode::Lab,
        Some("home") => p.mode = prefs::Mode::Home,
        Some("farm") => p.mode = prefs::Mode::Farm,
        _ => {}
    }
    match get("theme") {
        Some("light") => p.theme = prefs::Theme::Light,
        Some("dark") => p.theme = prefs::Theme::Dark,
        _ => {}
    }
    p.boomer = has("boomer");
    p.sunlight = Some(has("sun"));
    p.perf = if has("lite") {
        prefs::Perf::Lite
    } else {
        prefs::Perf::Normal
    };
    get("edit").and_then(|e| e.parse().ok())
}
