//! History: one channel over 1 h / 24 h / 7 d / 30 d.

use leptos::prelude::*;

use crate::api;
use crate::chart::LineChart;
use crate::format;
use crate::model::Reading;
use crate::prefs::Perf;
use crate::ui::{Tabs, ctx, poll};

const RANGES: [(&str, &str, i64); 4] = [
    ("1h", "1 hour", 3600),
    ("24h", "24 hours", 86_400),
    ("7d", "7 days", 7 * 86_400),
    ("30d", "30 days", 30 * 86_400),
];

#[component]
pub fn History() -> impl IntoView {
    let c = ctx();
    let nodes = poll(15_000, api::nodes);
    let range = RwSignal::new("24h".to_string());
    let secs = move || {
        RANGES
            .iter()
            .find(|r| r.0 == range.get())
            .map_or(86_400, |r| r.2)
    };

    // Pick the first channel if none was chosen yet.
    Effect::new(move |_| {
        if c.history_sel.get().is_none()
            && let Some((n, ch)) = nodes.data.with(|d| {
                d.iter()
                    .flatten()
                    .find_map(|n| n.channels.first().map(|ch| (n.id, ch.channel)))
            })
        {
            c.history_sel.set(Some((n, ch)));
        }
    });
    let channel = Memo::new(move |_| {
        let (n, ch) = c.history_sel.get()?;
        nodes.data.with(|d| {
            let node = d.iter().flatten().find(|x| x.id == n)?;
            Some((
                node.name.clone(),
                node.channels.iter().find(|x| x.channel == ch)?.clone(),
            ))
        })
    });

    let readings = RwSignal::new(Vec::<Reading>::new());
    let (error, set_error) = signal(None::<String>);
    let limit = move || c.prefs.with(|p| p.history_limit());
    let load = move || {
        let Some((n, ch)) = c.history_sel.get_untracked() else {
            return;
        };
        let since = format::now() - secs();
        let lim = limit();
        leptos::task::spawn_local(async move {
            match api::readings(n, ch, since, lim).await {
                Ok(r) => {
                    readings.try_set(r);
                    set_error.try_set(None);
                }
                Err(e) => {
                    set_error.try_set(Some(e));
                }
            }
        });
    };
    // Reload on a new channel or range, and every 30 s.
    Effect::new(move |_| {
        c.history_sel.track();
        range.track();
        load();
    });
    let refresher = poll(30_000, move || async move {
        load();
        Ok(())
    });
    let _ = refresher;

    let points = Signal::derive(move || {
        let kind = channel
            .with(|c| c.as_ref().map(|c| c.1.kind))
            .unwrap_or_default();
        readings.with(|r| {
            r.iter()
                .map(|x| (x.ts, format::chart_value(kind, x.value)))
                .collect::<Vec<_>>()
        })
    });
    let stats = move || {
        let p = points.get();
        if p.is_empty() {
            return None;
        }
        let vals: Vec<f64> = p.iter().map(|x| x.1).collect();
        let lo = vals.iter().copied().fold(f64::MAX, f64::min);
        let hi = vals.iter().copied().fold(f64::MIN, f64::max);
        let avg = vals.iter().sum::<f64>() / vals.len() as f64;
        Some((lo, hi, avg))
    };
    let unit = Signal::derive(move || {
        channel
            .get()
            .map(|c| c.1.kind.unit().to_string())
            .unwrap_or_default()
    });
    let fmt = move |v: f64| {
        if channel.get().is_some_and(|c| c.1.kind.is_digital()) {
            format!("{:.0} %", v * 100.0)
        } else {
            format!("{v:.2} {}", unit.get())
        }
    };
    let options = move || {
        nodes.data.with(|d| {
            d.iter()
                .flatten()
                .flat_map(|n| {
                    n.channels.iter().map(move |ch| {
                        (
                            format!("{}:{}", n.id, ch.channel),
                            format!("{} — {}", n.name, ch.name),
                        )
                    })
                })
                .collect::<Vec<_>>()
        })
    };

    view! {
        {(!c.phone).then(|| view! {
            <header class="page-head rise">
                <div>
                    <h1>"History"</h1>
                    <p>"The PC keeps every reading; the hub keeps 30 days."</p>
                </div>
            </header>
        })}
        <div class="hstack" style="justify-content: space-between">
            <select
                class="field"
                style="min-width: 260px; max-width: 100%"
                aria-label="Channel"
                on:change=move |e| {
                    let v = event_target_value(&e);
                    if let Some((n, ch)) = v.split_once(':')
                        && let (Ok(n), Ok(ch)) = (n.parse(), ch.parse())
                    {
                        c.history_sel.set(Some((n, ch)));
                    }
                }
            >
                {move || {
                    let sel = c.history_sel.get().map(|(n, ch)| format!("{n}:{ch}"));
                    options().into_iter().map(|(k, label)| {
                        let selected = Some(&k) == sel.as_ref();
                        view! { <option value=k selected=selected>{label}</option> }
                    }).collect_view()
                }}
            </select>
            <Tabs options=RANGES.iter().map(|r| (r.0.to_string(), r.1.to_string())).collect() selected=range/>
        </div>
        <section class="card stack rise">
            <div class="hstack" style="justify-content: space-between">
                <h3>{move || channel.get().map(|(n, ch)| format!("{n} / {}", ch.name))}</h3>
                <span class="muted">{move || channel.get().map(|(_, ch)| format!("Now {}", format::value(ch.kind, ch.last_value)))}</span>
            </div>
            {move || error.get().map(|e| view! { <div class="banner">{e}</div> })}
            {move || if points.with(Vec::is_empty) {
                view! { <div class="empty">"No readings in this range."</div> }.into_any()
            } else {
                view! {
                    <LineChart
                        points=points
                        series=Signal::derive(move || format!("{:?}{}", c.history_sel.get(), range.get()))
                        digital=Signal::derive(move || channel.get().is_some_and(|c| c.1.kind.is_digital()))
                        unit=unit
                        long_range=Signal::derive(move || secs() > 86_400)
                        max_points=Signal::derive(move || if c.prefs.with(|p| p.perf) == Perf::Lite { 200 } else { 600 })
                    />
                }.into_any()
            }}
            {move || stats().map(|(lo, hi, avg)| view! {
                <div class="stats">
                    <div class="stat"><span class="eyebrow">"Low"</span><span class="mono">{fmt(lo)}</span></div>
                    <div class="stat"><span class="eyebrow">"Average"</span><span class="mono">{fmt(avg)}</span></div>
                    <div class="stat"><span class="eyebrow">"High"</span><span class="mono">{fmt(hi)}</span></div>
                    <div class="stat"><span class="eyebrow">"Points"</span><span class="mono">{points.with(Vec::len)}</span></div>
                </div>
            })}
        </section>
    }
}
