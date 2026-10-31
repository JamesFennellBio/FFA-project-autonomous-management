//! Automation: recipes stored and run on the hub (they keep working with the
//! PC and phone off). Two editors producing the same recipe JSON: a simple
//! sentence ("When ___ is above ___, turn ___ on") and Blockly.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api;
use crate::blockly::BlockEditor;
use crate::model::{EditorChannel, Recipe, RecipeBody, editor_channels};
use crate::recipe_edit::SimpleRule;
use crate::ui::{self, Switch, Tabs, ctx, poll};

#[component]
pub fn Automation() -> impl IntoView {
    let c = ctx();
    move || match c.editing.get() {
        Some(id) => view! { <Editor id=id/> }.into_any(),
        None => view! { <RecipeList/> }.into_any(),
    }
}

#[component]
fn RecipeList() -> impl IntoView {
    let c = ctx();
    let list = poll(8000, api::recipes);
    let full = move || {
        list.data.with(|d| {
            d.as_ref()
                .is_some_and(|d| d.recipes.len() >= d.max_recipes && d.max_recipes > 0)
        })
    };
    view! {
        <header class="page-head rise">
            <div>
                {(!c.phone).then(|| view! { <h1>"Automation"</h1> })}
                <p>"Recipes run on the hub itself, so they keep working when this device is off."</p>
            </div>
            <button class="btn primary" disabled=full on:click=move |_| c.editing.set(Some(0))>
                {ui::icon(ui::Icon::Plus, 14)} "New recipe" <span class="kbd">"C"</span>
            </button>
        </header>
        {move || full().then(|| view! { <div class="banner warn">"The hub holds this many recipes at most. Delete one to add another."</div> })}
        {move || list.error.get().map(|e| view! { <div class="banner">{e}</div> })}
        {move || list.loading().then(|| view! { <div class="card skel" style="height: 120px"></div> })}
        {move || list.data.with(|d| d.as_ref().is_some_and(|d| d.recipes.is_empty())).then(|| view! {
            <div class="card empty">
                {ui::icon(ui::Icon::Bolt, 24)}
                <p>{move || format!("No recipes yet. For example: {}", c.prefs.with(|p| p.mode.copy().example_rule))}</p>
            </div>
        })}
        <div class="rows" style:display=move || if list.data.with(|d| d.as_ref().is_none_or(|d| d.recipes.is_empty())) { "none" } else { "block" }>
            <For each=move || list.data.get().map(|d| d.recipes).unwrap_or_default() key=|r| (r.id, r.enabled, r.name.clone()) let:r>
                {
                    let (id, name, enabled) = (r.id, r.name.clone(), r.enabled);
                    let del_name = name.clone();
                    view! {
                        <div class="row rise" class:dim=!enabled>
                            <button class="link grow title" on:click=move |_| c.editing.set(Some(id))>{name.clone()}</button>
                            <span class="small muted">{if enabled { "On" } else { "Off" }}</span>
                            <Switch on=enabled label=format!("{name} on or off") on_toggle=move |v| {
                                ui::run(api::set_recipe_enabled(id, v), move || list.refresh());
                            }/>
                            <button class="btn sm" on:click=move |_| c.editing.set(Some(id))>"Edit"</button>
                            <button class="btn sm danger" on:click=move |_| {
                                if ui::confirm(&format!("Delete the recipe \"{del_name}\"?\n\nIf it's on right now, its 'otherwise' part runs first.")) {
                                    ui::run(api::delete_recipe(id), move || { ui::toast("Recipe deleted"); list.refresh(); });
                                }
                            }>"Delete"</button>
                        </div>
                    }
                }
            </For>
        </div>
    }
}

/// Loaded once: the recipe (if editing) and the channels to choose from.
#[derive(Clone)]
struct Loaded {
    recipe: Option<Recipe>,
    channels: Vec<EditorChannel>,
}

#[component]
fn Editor(id: u32) -> impl IntoView {
    let loaded = RwSignal::new(None::<Result<Loaded, String>>);
    spawn_local(async move {
        let r = async {
            let nodes = api::nodes().await?;
            let recipe = if id == 0 {
                None
            } else {
                Some(api::recipe(id).await?)
            };
            Ok(Loaded {
                recipe,
                channels: editor_channels(&nodes),
            })
        }
        .await;
        loaded.try_set(Some(r));
    });
    move || match loaded.get() {
        None => view! { <div class="card skel" style="height: 320px"></div> }.into_any(),
        Some(Err(e)) => view! { <div class="banner">{e}</div> }.into_any(),
        Some(Ok(l)) => view! { <EditorForm id=id loaded=l/> }.into_any(),
    }
}

#[component]
fn EditorForm(id: u32, loaded: Loaded) -> impl IntoView {
    let c = ctx();
    let Loaded { recipe, channels } = loaded;
    let name = RwSignal::new(recipe.as_ref().map(|r| r.name.clone()).unwrap_or_default());
    let enabled = RwSignal::new(recipe.as_ref().is_none_or(|r| r.enabled));
    let body = recipe.as_ref().map(|r| r.body.clone());
    let simple_start = body
        .as_ref()
        .and_then(|b| SimpleRule::from_body(b, &channels));
    let fits_simple = body.is_none() || simple_start.is_some();
    let prefer_simple = c.phone || c.prefs.with_untracked(|p| p.boomer);
    // A recipe that fits the sentence opens in it; a new one starts there in
    // Boomer Mode and on the phone (blocks are fiddly on a small screen).
    let start_simple = match (&body, fits_simple) {
        (_, false) => false,
        (Some(_), true) => true,
        (None, true) => prefer_simple,
    };
    let tab = RwSignal::new(if start_simple { "simple" } else { "blocks" }.to_string());
    let blocks_result = RwSignal::new(Err::<RecipeBody, String>(
        "Loading the block editor…".into(),
    ));
    let simple_result = RwSignal::new(Err::<RecipeBody, String>("Pick a sensor.".into()));
    let current = move || {
        if tab.get() == "simple" {
            simple_result.get()
        } else {
            blocks_result.get()
        }
    };
    let (busy, set_busy) = signal(false);

    let save = move |_| {
        let n = name.get_untracked().trim().to_string();
        if n.is_empty() {
            ui::toast_err("Give the recipe a name.");
            return;
        }
        let Ok(b) = current() else { return };
        let r = Recipe {
            id,
            name: n,
            enabled: enabled.get_untracked(),
            body: b,
        };
        set_busy.set(true);
        spawn_local(async move {
            match api::save_recipe(&r).await {
                Ok(_) => {
                    ui::toast("Saved to the hub");
                    c.editing.set(None);
                }
                Err(e) => ui::toast_err(e),
            }
            set_busy.set(false);
        });
    };
    let delete = move |_| {
        if ui::confirm(
            "Delete this recipe?\n\nIf it's on right now, its 'otherwise' part runs first.",
        ) {
            ui::run(api::delete_recipe(id), move || {
                ui::toast("Recipe deleted");
                c.editing.set(None);
            });
        }
    };
    let dark = document()
        .document_element()
        .and_then(|e| e.get_attribute("data-theme"))
        .is_some_and(|t| t == "dark");
    let mode = c.prefs.with_untracked(|p| p.mode);
    let (chans_b, chans_s) = (channels.clone(), channels);
    let body_b = body.clone();

    view! {
        <header class="page-head rise">
            <div class="stack" style="gap: 6px; flex: 1; min-width: 240px">
                <span class="eyebrow">{if id == 0 { "New recipe" } else { "Edit recipe" }}</span>
                <input
                    class="field"
                    style="font-size: var(--fs-section); height: 44px; max-width: 520px"
                    maxlength="32"
                    placeholder="Name, e.g. Cool the barn"
                    aria-label="Recipe name"
                    prop:value=name
                    on:input=move |e| name.set(event_target_value(&e))
                />
            </div>
            <label class="hstack label">
                <Switch on=enabled label="Recipe on or off" on_toggle=move |v| enabled.set(v)/>
                {move || if enabled.get() { "On" } else { "Off" }}
            </label>
        </header>
        <Tabs options=vec![("simple".into(), "Simple".into()), ("blocks".into(), "Blocks".into())] selected=tab/>
        <section class="card stack rise">
            {move || if tab.get() == "simple" {
                if fits_simple {
                    view! { <SimpleBuilder start=simple_start.clone() channels=chans_s.clone() result=simple_result/> }.into_any()
                } else {
                    view! { <p class="muted">"This recipe has more than one condition, so it only opens in Blocks."</p> }.into_any()
                }
            } else {
                view! { <BlockEditor body=body_b.clone() channels=chans_b.clone() mode=mode dark=dark result=blocks_result/> }.into_any()
            }}
            <hr class="chalk-rule" style="margin: 4px 0"/>
            <div class="hstack">
                {move || match current() {
                    Ok(_) => view! { <span class="ok-text">{ui::icon(ui::Icon::Check, 14)}" Ready to save"</span> }.into_any(),
                    Err(e) => view! { <span class="muted small">{e}</span> }.into_any(),
                }}
                <span class="spacer"></span>
                {(id != 0).then(|| view! { <button class="btn danger" on:click=delete>"Delete"</button> })}
                <button class="btn" on:click=move |_| c.editing.set(None)>"Cancel"</button>
                <button class="btn primary" disabled=move || busy.get() || current().is_err() on:click=save>
                    {move || if busy.get() { "Saving…" } else { "Save to hub" }}
                </button>
            </div>
        </section>
    }
}

/// "When [sensor] is [above] [30] °C, turn [Fans] [on]. Otherwise turn it back."
#[component]
fn SimpleBuilder(
    start: Option<SimpleRule>,
    channels: Vec<EditorChannel>,
    result: RwSignal<Result<RecipeBody, String>>,
) -> impl IntoView {
    let key = |k: (u16, u8)| format!("{}:{}", k.0, k.1);
    let parse = |s: &str| -> Option<(u16, u8)> {
        let (a, b) = s.split_once(':')?;
        Some((a.parse().ok()?, b.parse().ok()?))
    };
    let first_sensor = channels
        .iter()
        .find(|c| !c.output)
        .or(channels.first())
        .map(|c| (c.node, c.channel));
    let first_output = channels
        .iter()
        .find(|c| c.output)
        .map(|c| (c.node, c.channel));
    let s = start.unwrap_or(SimpleRule {
        sensor: first_sensor.unwrap_or((0, 0)),
        above: true,
        value: 30.0,
        output: first_output,
        turn_on: true,
        undo_otherwise: true,
        alert: String::new(),
    });
    let sensor = RwSignal::new(key(s.sensor));
    let above = RwSignal::new(s.above);
    let value = RwSignal::new(format!("{}", s.value));
    let output = RwSignal::new(s.output.map(key).unwrap_or_default());
    let turn_on = RwSignal::new(s.turn_on);
    let undo = RwSignal::new(s.undo_otherwise);
    let alert = RwSignal::new(s.alert);

    let chans = StoredValue::new(channels);
    Effect::new(move |_| {
        let r = (|| {
            let sensor = parse(&sensor.get()).ok_or("Pick a sensor.")?;
            let v: f64 = value
                .get()
                .trim()
                .replace(',', ".")
                .parse()
                .map_err(|_| "Type a number.")?;
            let rule = SimpleRule {
                sensor,
                above: above.get(),
                value: v,
                output: parse(&output.get()),
                turn_on: turn_on.get(),
                undo_otherwise: undo.get(),
                alert: alert.get(),
            };
            chans.with_value(|c| rule.to_body(c))
        })();
        result.set(r.map_err(|e: String| e));
    });

    let unit = move || {
        let k = parse(&sensor.get());
        chans.with_value(|c| {
            c.iter()
                .find(|x| Some((x.node, x.channel)) == k)
                .map(|x| x.unit.clone())
                .unwrap_or_default()
        })
    };
    let sensor_opts = chans.with_value(|c| {
        c.iter()
            .map(|x| (x.key(), x.label.clone()))
            .collect::<Vec<_>>()
    });
    let output_opts = chans.with_value(|c| {
        c.iter()
            .filter(|x| x.output)
            .map(|x| (x.key(), x.label.clone()))
            .collect::<Vec<_>>()
    });
    let sel = |list: Vec<(String, String)>,
               sig: RwSignal<String>,
               aria: &'static str,
               none: Option<&'static str>| {
        view! {
            <select class="field" aria-label=aria on:change=move |e| sig.set(event_target_value(&e))>
                {none.map(|n| view! { <option value="" selected=move || sig.get().is_empty()>{n}</option> })}
                {list.into_iter().map(|(k, l)| {
                    let k2 = k.clone();
                    view! { <option value=k selected=move || sig.get() == k2>{l}</option> }
                }).collect_view()}
            </select>
        }
    };
    let bool_sel = |sig: RwSignal<bool>,
                    yes: &'static str,
                    no: &'static str,
                    aria: &'static str| {
        view! {
            <select class="field" aria-label=aria on:change=move |e| sig.set(event_target_value(&e) == "1")>
                <option value="1" selected=move || sig.get()>{yes}</option>
                <option value="0" selected=move || !sig.get()>{no}</option>
            </select>
        }
    };

    view! {
        <div class="sentence">
            <span>"When"</span>
            {sel(sensor_opts, sensor, "Sensor", None)}
            <span>"is"</span>
            {bool_sel(above, "above", "below", "Above or below")}
            <input class="field" style="width: 96px" inputmode="decimal" aria-label="Value" prop:value=value on:input=move |e| value.set(event_target_value(&e))/>
            <span>{unit}","</span>
            <span>"turn"</span>
            {sel(output_opts, output, "Output", Some("(nothing)"))}
            {bool_sel(turn_on, "on", "off", "On or off")}
            <span>"."</span>
        </div>
        <label class="hstack label">
            <Switch on=undo label="Undo it otherwise" on_toggle=move |v| undo.set(v)/>
            "Switch it back when that's no longer true"
        </label>
        <label class="form">
            "Also send an alert (optional)"
            <input class="field" maxlength="60" placeholder="e.g. Greenhouse is too hot" prop:value=alert on:input=move |e| alert.set(event_target_value(&e))/>
        </label>
    }
}
