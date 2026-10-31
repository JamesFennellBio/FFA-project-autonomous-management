# Reference spec sheets

Collected 2026-10-30 as the basis for the Farm & Lab v2 design tokens (`farmlab-spec.md`).

**Sources:**
- **Vercel:** the official `vercel.com/geist/vercel-brand.css` and `vercel.com/design.md`. These values are exact.
- **Linear:** Linear's own redesign write-up, plus a third-party token extract.
- **Chronicle:** a third-party style analysis (Refero).

Linear and Chronicle values are **approximate**, so treat them as direction, not gospel.

---

## Vercel · Geist (official)

| Area | Values |
|---|---|
| Philosophy | "Design in monochrome." Color only for state, action or data, always paired with a non-color cue. |
| Rejected | Decorative gradients, gradient text, glows, blobs, textures, grid backgrounds, glass effects, ornamental shadows, fake depth |
| Dark surfaces | background-100 `oklch(0 0 0)`, background-200 `oklch(0.027 0 0)`, contrast surface `oklch(0.205 0 0)` |
| Dark grays | 100 `oklch(.218)` · 200 `.239` · 300 `.281` · 400 `.301` · 500 `.39` · 600 `.623` · 700 `.65` · 800 `.59` · 900 `.706` · 1000 `.946` |
| Text (dark) | Primary `oklch(.946 0 0)`, secondary `oklch(.706 0 0)` |
| Borders (dark) | Subtle `oklch(1 0 0 / .13)`, default `/ .14`, strong `/ .54` |
| Status (dark) | Info `oklch(57.61% .2321 258.23)`, success `oklch(73.1% .2158 148.29)`, warning `oklch(77.21% .1991 64.28)`, error `oklch(62.56% .2234 23.03)` |
| Focus (dark) | `oklch(71.7% .1648 250.794)` |
| Type | Geist Sans for everything; Geist Mono only for code, paths, IDs and timestamps; tabular numerals for comparisons |
| Type scale | Display 48/56 · page title 40/48 · title 32/40 · section 24/32 · subsection 20/26 · lede 18/28 · body 16/24 · compact 14/20 · label 13 · metadata 13 · caption leading 18 |
| Weights | 400 regular · 450 heading · 500 medium · 600 semibold |
| Radii | 6 px small, 8 px default, "restrained" |
| Spacing | 4 8 12 16 20 24 32 40 48 64. Within a group 8–16, between groups 24–32, section turns 32–48 |
| Grid | 12 columns desktop / 6 tablet / 4 mobile; gutters "unmistakable" |
| Shadows | None as tokens; separation comes from spacing, then hairlines |
| Motion | "Default to stillness." Buttons use `background-color 150ms ease, border-color 150ms ease`. No pulsing indicators, bounce, parallax or cinematic transitions. Motion only explains a state change, keeps continuity or confirms an action. |

## Linear (approximate)

| Area | Values |
|---|---|
| Color system | Generated in LCH from three inputs (base, accent, contrast). Accent bleed into neutrals is limited. Contrast is a user-tunable variable (30 to 100). |
| Dark surfaces | `#08090a` canvas · `#0f1011` panel · `#191a1b` level 3 · `#28282c` secondary |
| Text | `#f7f8f8` · `#d0d6e0` · `#8a8f98` · `#62666d` (4 steps) |
| Borders | `rgba(255,255,255,.05)` subtle · `.08` standard · solid `#23252a` / `#34343a` |
| Fills | Cards and buttons `rgba(255,255,255,.02–.05)` |
| Accent | Indigo `#5e6ad2`, interactive `#7170ff` |
| Type | Inter Variable with `cv01`, `ss03`; Inter Display for headings. Weights 400 / 510 / 590 (never 700). |
| Type scale | H1 32 / −0.704 px · H2 24 / −0.288 · H3 20 · 590 · body 16/1.5 · small 15 · caption 13 · label 12 · micro 11 |
| Radii | 2 badges · 4 small · 6 buttons and inputs · 8 cards · 12 panels · pill |
| Shadows | Dialog: stacked 1–8 px low-alpha layers; otherwise hairline rings `0 0 0 1px rgba(0,0,0,.2)` |
| Layout | Inverted-L chrome (sidebar + top bar); dense, aligned icons and labels; keyboard-first |

## Chronicle (approximate)

| Area | Values |
|---|---|
| Philosophy | Achromatic UI. The only color is the product content itself. Hierarchy comes from weight, size and gray value. |
| Grays | `#000` · `#050505` · `#151515` · `#292929` · `#6b6b6b` · `#7e7e7e` · `#929292` · `#b3b3b3` · `#e2e2e2` · `#f3f3f3` · `#fff` |
| Type | Diatype (fallback Inter); weights 400 and 500 only |
| Type scale | 12 · 14 · 16 · 20 · 32 · 48 · 54. Leading 1.0 display → 1.4 body. Tracking −0.03em at 48+, −0.023em at 32, −0.02em at 20, 0 at 16 and below. |
| Radii | 4 px controls, 8 px cards and frames only |
| Borders | 1 px hairlines only |
| Shadow | One level: `0 2px 24px rgba(5,5,5,.08)` |
| Buttons | Primary filled black (white in dark), weight 500; secondary is ghost text or underline |
| Motion | Color 0.2 s ease, transforms 0.3 s, reveals ≤ 0.4 s, curve `cubic-bezier(.41,.66,.36,.86)`, no bounce or spring |
| Rules | No more than two type sizes in a block; at most one shadow level; nothing heavier than 500 |
