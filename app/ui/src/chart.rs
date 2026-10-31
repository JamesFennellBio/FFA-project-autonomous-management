//! History chart in SVG (replaces recharts). The geometry is plain functions
//! so it can be tested; the component adds the draw-in and the hover readout.

use leptos::prelude::*;

const W: f64 = 800.0;
const H: f64 = 280.0;
const PAD_L: f64 = 52.0;
const PAD_R: f64 = 12.0;
const PAD_T: f64 = 14.0;
const PAD_B: f64 = 28.0;

/// Keeps at most `max` points, evenly spaced, always keeping the last one
/// (fewer in Lite: see the `max_points` prop).
pub fn decimate(points: &[(i64, f64)], max: usize) -> Vec<(i64, f64)> {
    if points.len() <= max || max < 2 {
        return points.to_vec();
    }
    let step = (points.len() - 1) as f64 / (max - 1) as f64;
    (0..max)
        .map(|i| points[((i as f64 * step).round() as usize).min(points.len() - 1)])
        .collect()
}

/// "Nice" axis ticks covering lo..hi (1, 2, 2.5, 5 x 10^n steps).
pub fn ticks(lo: f64, hi: f64, want: usize) -> Vec<f64> {
    // Also catches NaN.
    if hi.partial_cmp(&lo) != Some(std::cmp::Ordering::Greater) {
        return vec![lo];
    }
    let raw = (hi - lo) / want.max(1) as f64;
    let mag = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0]
        .into_iter()
        .map(|m| m * mag)
        .find(|s| *s >= raw)
        .unwrap_or(10.0 * mag);
    let start = (lo / step).ceil() * step;
    let mut out = Vec::new();
    let mut v = start;
    while v <= hi + step * 1e-9 && out.len() < 20 {
        out.push((v / step).round() * step);
        v += step;
    }
    out
}

pub struct Geometry {
    pub t0: i64,
    pub t1: i64,
    pub lo: f64,
    pub hi: f64,
}

impl Geometry {
    pub fn new(points: &[(i64, f64)], digital: bool) -> Option<Self> {
        let (first, last) = (points.first()?, points.last()?);
        let (mut lo, mut hi) = if digital {
            (0.0, 1.0)
        } else {
            points
                .iter()
                .fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)))
        };
        if !digital {
            let pad = ((hi - lo) * 0.12).max(0.5);
            lo -= pad;
            hi += pad;
        }
        if hi - lo < 1e-9 {
            hi = lo + 1.0;
        }
        Some(Self {
            t0: first.0,
            t1: last.0.max(first.0 + 1),
            lo,
            hi,
        })
    }
    pub fn x(&self, t: i64) -> f64 {
        PAD_L + (t - self.t0) as f64 / (self.t1 - self.t0) as f64 * (W - PAD_L - PAD_R)
    }
    pub fn y(&self, v: f64) -> f64 {
        PAD_T + (1.0 - (v - self.lo) / (self.hi - self.lo)) * (H - PAD_T - PAD_B)
    }
    /// Line path; on/off channels step instead of sloping.
    pub fn line(&self, points: &[(i64, f64)], step: bool) -> String {
        let mut d = String::new();
        let mut prev_y = 0.0;
        for (i, (t, v)) in points.iter().enumerate() {
            let (x, y) = (self.x(*t), self.y(*v));
            if i == 0 {
                d += &format!("M{x:.1} {y:.1}");
            } else if step {
                d += &format!("H{x:.1}");
                if (y - prev_y).abs() > 0.01 {
                    d += &format!("V{y:.1}");
                }
            } else {
                d += &format!("L{x:.1} {y:.1}");
            }
            prev_y = y;
        }
        d
    }
    /// The line closed down to the baseline, for the soft fill under it.
    pub fn area(&self, points: &[(i64, f64)], step: bool) -> String {
        let Some(first) = points.first() else {
            return String::new();
        };
        let base = H - PAD_B;
        let line = self.line(points, step);
        format!(
            "M{:.1} {base:.1}L{}V{base:.1}Z",
            self.x(first.0),
            &line[1..]
        )
    }
}

/// The history chart. `series` changes when a different channel or range is
/// picked: only then does the line draw in again (not on every refresh).
#[component]
pub fn LineChart(
    #[prop(into)] points: Signal<Vec<(i64, f64)>>,
    #[prop(into)] series: Signal<String>,
    #[prop(into)] digital: Signal<bool>,
    #[prop(into)] unit: Signal<String>,
    #[prop(into)] long_range: Signal<bool>,
    #[prop(into)] max_points: Signal<usize>,
) -> impl IntoView {
    let shown = Memo::new(move |_| decimate(&points.get(), max_points.get()));
    let geo = Memo::new(move |_| {
        shown.with(|p| Geometry::new(p, digital.get()).map(|g| (g.t0, g.t1, g.lo, g.hi)))
    });
    let g = move || {
        geo.get()
            .map(|(t0, t1, lo, hi)| Geometry { t0, t1, lo, hi })
    };
    let (hover, set_hover) = signal(None::<usize>);
    let fmt_v = move |v: f64| {
        if digital.get() {
            if v >= 0.5 {
                "On".to_string()
            } else {
                "Off".to_string()
            }
        } else {
            format!("{v:.2} {}", unit.get())
        }
    };

    let y_axis = move || {
        let Some(g) = g() else { return Vec::new() };
        let ts = if digital.get() {
            vec![0.0, 1.0]
        } else {
            ticks(g.lo, g.hi, 4)
        };
        ts.into_iter()
            .map(|v| {
                let y = g.y(v);
                let label = if digital.get() {
                    if v >= 0.5 { "On".into() } else { "Off".into() }
                } else {
                    format!("{v}")
                };
                view! {
                    <line class="grid-line" x1=PAD_L x2=W - PAD_R y1=y y2=y></line>
                    <text class="axis" x=PAD_L - 8.0 y=y + 4.0 text-anchor="end">{label}</text>
                }
            })
            .collect_view()
    };
    let x_axis = move || {
        let Some(g) = g() else { return Vec::new() };
        (0..5)
            .map(|i| {
                let t = g.t0 + (g.t1 - g.t0) * i / 4;
                let anchor = match i {
                    0 => "start",
                    4 => "end",
                    _ => "middle",
                };
                view! {
                    <text class="axis" x=g.x(t) y=H - 8.0 text-anchor=anchor>
                        {crate::format::clock(t, long_range.get())}
                    </text>
                }
            })
            .collect_view()
    };

    let on_move = move |e: leptos::ev::MouseEvent| {
        let Some(svg) = e
            .current_target()
            .and_then(|t| wasm_bindgen::JsCast::dyn_into::<web_sys::Element>(t).ok())
        else {
            return;
        };
        let r = svg.get_bounding_client_rect();
        let fx = (f64::from(e.client_x()) - r.left()) / r.width() * W;
        let Some(g) = g() else { return };
        shown.with(|p| {
            let best = p
                .iter()
                .enumerate()
                .min_by(|a, b| (g.x(a.1.0) - fx).abs().total_cmp(&(g.x(b.1.0) - fx).abs()))
                .map(|(i, _)| i);
            set_hover.set(best);
        });
    };

    view! {
        <div class="chart">
            <svg
                viewBox=format!("0 0 {W} {H}")
                role="img"
                aria-label="History chart"
                on:mousemove=on_move
                on:mouseleave=move |_| set_hover.set(None)
            >
                {y_axis}
                {x_axis}
                {move || {
                    series.track();
                    view! {
                        <path class="area" d=move || g().map(|g| shown.with(|p| g.area(p, digital.get()))).unwrap_or_default()></path>
                        <path
                            class="line draw"
                            pathLength="1"
                            d=move || g().map(|g| shown.with(|p| g.line(p, digital.get()))).unwrap_or_default()
                        ></path>
                    }
                }}
                {move || {
                    let (i, g) = (hover.get()?, g()?);
                    let (t, v) = shown.with(|p| p.get(i).copied())?;
                    let (x, y) = (g.x(t), g.y(v));
                    Some(view! {
                        <line class="cross" x1=x x2=x y1=PAD_T y2=H - PAD_B></line>
                        <circle class="pt" cx=x cy=y r="4"></circle>
                    })
                }}
            </svg>
            {move || {
                let (i, g) = (hover.get()?, g()?);
                let (t, v) = shown.with(|p| p.get(i).copied())?;
                let left = g.x(t) / W * 100.0;
                Some(view! {
                    <div class="chart-tip" style=format!("left: {left:.2}%")>
                        <div class="mono">{fmt_v(v)}</div>
                        <div class="faint small">{crate::format::clock(t, long_range.get())}</div>
                    </div>
                })
            }}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimation_keeps_ends() {
        let pts: Vec<(i64, f64)> = (0..1000).map(|i| (i, i as f64)).collect();
        let d = decimate(&pts, 100);
        assert_eq!(d.len(), 100);
        assert_eq!(d[0], (0, 0.0));
        assert_eq!(*d.last().unwrap(), (999, 999.0));
        assert_eq!(decimate(&pts[..10], 100).len(), 10);
    }

    #[test]
    fn nice_ticks() {
        assert_eq!(ticks(0.0, 10.0, 4), vec![0.0, 2.5, 5.0, 7.5, 10.0]);
        assert_eq!(ticks(18.3, 24.9, 4), vec![20.0, 22.0, 24.0]);
        assert_eq!(ticks(5.0, 5.0, 4), vec![5.0]);
    }

    #[test]
    fn geometry_maps_into_the_plot_area() {
        let pts = [(100, 0.0), (200, 1.0), (300, 0.0)];
        let g = Geometry::new(&pts, true).unwrap();
        assert_eq!(g.x(100), PAD_L);
        assert_eq!(g.x(300), W - PAD_R);
        assert_eq!(g.y(1.0), PAD_T);
        // On/off channels step.
        assert_eq!(g.line(&pts, true), "M52.0 252.0H420.0V14.0H788.0V252.0");
        assert!(g.area(&pts, true).starts_with("M52.0 252.0L52.0 252.0"));
        assert!(g.area(&pts, true).ends_with('Z'));
    }
}
