//! The Blockly recipe editor, through vendor/blockly-shim.js. Blockly is the
//! one piece of the UI that stays JavaScript (there's no Rust equivalent);
//! what the blocks mean is decided in recipe_edit.rs.

use leptos::html;
use leptos::prelude::*;
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;

use crate::model::{EditorChannel, RecipeBody};
use crate::prefs::Mode;
use crate::recipe_edit;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = farmlabBlockly, js_name = open)]
    fn bl_open(
        div: &web_sys::HtmlElement,
        state: &str,
        channels: &str,
        dark: bool,
        colors: &str,
        on_change: &js_sys::Function,
    ) -> u32;
    #[wasm_bindgen(js_namespace = farmlabBlockly, js_name = close)]
    fn bl_close(id: u32);
}

/// Block colours per mode. Darker than the UI accents so white block text
/// stays readable.
fn colors(mode: Mode) -> Value {
    let cond = match mode {
        Mode::Lab => "#2563eb",
        Mode::Farm => "#15803d",
        Mode::Home => "#c2410c",
    };
    json!({ "recipe": "#52525b", "cond": cond, "set": "#475569", "alert": "#b42318" })
}

/// Shows `body` (or an empty recipe) as blocks. `result` always holds what
/// the blocks currently say: the recipe, or what to fix.
#[component]
pub fn BlockEditor(
    body: Option<RecipeBody>,
    channels: Vec<EditorChannel>,
    mode: Mode,
    dark: bool,
    result: RwSignal<Result<RecipeBody, String>>,
) -> impl IntoView {
    let div = NodeRef::<html::Div>::new();
    let mut chans = channels;
    let state = recipe_edit::workspace_state(body.as_ref(), &mut chans);
    let chans_json = serde_json::to_string(&chans).unwrap_or_default();
    let id = StoredValue::new(None::<u32>);
    div.on_load(move |el| {
        let chans = chans.clone();
        let cb = Closure::<dyn Fn(String)>::new(move |json: String| {
            let r = serde_json::from_str::<Value>(&json)
                .map_err(|e| e.to_string())
                .and_then(|v| recipe_edit::read_workspace(&v, &chans));
            result.try_set(r);
        });
        let el: &web_sys::HtmlElement = &el;
        id.set_value(Some(bl_open(
            el,
            &state.to_string(),
            &chans_json,
            dark,
            &colors(mode).to_string(),
            cb.as_ref().unchecked_ref(),
        )));
        // Blockly keeps calling it until close(); it lives as long as the page.
        cb.forget();
    });
    on_cleanup(move || {
        if let Some(i) = id.try_get_value().flatten() {
            bl_close(i);
        }
    });
    view! { <div class="blockly" node_ref=div></div> }
}
