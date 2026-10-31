# Farm & Lab v2 spec sheet

This is derived from `reference-specs.md` (Vercel, Linear, Chronicle) plus these requirements from James:
- dot grid instead of a line grid
- charcoal / grey background
- thin white "chalk" lines
- Farm on the phone much brighter, for glare

The tokens below are the source for `app/ui/style/tokens.css`.

## Principles
1. **Monochrome first** (Vercel, Chronicle). The mode accent is the only color. It marks state, action and data: active nav, switches, charts, focus and status. It never appears on large surfaces.
2. **Charcoal, not black.** Surfaces are layered by lightness in small steps (Linear). Separation comes from spacing first, then chalk hairlines, almost never shadows.
3. **Chalk lines.** Every line is 1 px warm off-white (`#ECE9E2`) at low alpha, like chalk on slate. Section breaks inside a panel are dashed.
4. **Dot field.** The canvas has a 1 px dot every 20 px, at very low alpha. It is never shown behind text blocks inside cards.
5. **Stillness** (Vercel, Chronicle). Motion only explains a change, with no bounce, no spring and no decorative pulsing.

## Color: dark (default)
| Token | Value | Note |
|---|---|---|
| `--bg` | `#121315` | Chrome: sidebar and window frame (about Linear `#0f1011`, lifted) |
| `--panel` | `#17181A` | Inset content panel, carries the dot field |
| `--surface` | `#1C1D20` | Cards inside the panel (Linear level 3 `#191a1b`, lifted) |
| `--surface-2` | `rgba(236,233,226,.045)` | Hover, inset wells, segmented control tracks |
| `--raised` | `#202124` | Command menu, toasts, dialogs |
| `--border` | `rgba(236,233,226,.09)` | Chalk hairline |
| `--border-strong` | `rgba(236,233,226,.18)` | Inputs, emphasized edges |
| `--dot` | `rgba(236,233,226,.075)` | Dot field (panel only, 20 px) |
| `--text` | `#F2F1EE` | Primary (Vercel gray-1000, warmed) |
| `--muted` | `#A9A7A1` | Secondary (about Vercel gray-900) |
| `--faint` | `#8A8883` | Tertiary; 4.5:1 on `--surface` |
| `--bad` | `#FF6B5E` | Vercel error (dark) |
| `--warn` | `#F5A524` | Vercel warning (dark) |

## Color: light
`--bg #F1F1EF` · `--panel #FAFAF8` · `--surface #FFFFFF` · `--surface-2 rgba(20,20,18,.04)` · `--border rgba(20,20,18,.10)` · `--border-strong rgba(20,20,18,.20)` · `--dot rgba(20,20,18,.10)` · `--text #171717` · `--muted #4D4D4D` · `--faint #6B6B6B` (Vercel and Chronicle grays)

## Color: Sunlight (phone, outdoors)
This is the default for **Farm mode on the phone**. It's available as a toggle in every mode.
- Pure white `#FFFFFF` and pure black `#000000` text, which is the maximum luminance contrast.
- Lines are `rgba(0,0,0,.28)` at 1.5 px, so they don't vanish in glare.
- Secondary text is never lighter than `#262626`.
- The accent is a vivid green fill `#14C95B` with **black** ink (about 9.5:1). Green text uses `#00702F`.
- Type is one step larger, and every touch target is at least 48 px.

## Accents (per mode)
| Mode | Accent (fills, charts) | Text on dark | Text on light | Button ink |
|---|---|---|---|---|
| Lab | `#3B82F6` (fills use `#2563EB`, 5.2:1 with white) | `#8DB6FF` | `#0A4FC4` | white |
| Farm | `#2FBF71` (8.8:1 with black) | `#6BE3A2` | `#11693B` | black |
| Home | `#FF8A3D` (9:1 with black) | `#FFB27A` | `#A3460A` | black |

- **Primary button** (Vercel, Chronicle): solid `--text` fill with `--bg` ink, so it's white on charcoal in dark and black in light. Mode accents never fill buttons except in Boomer Mode and Sunlight, where a big colored target is clearer.
- **Accent soft**: the accent at 14% alpha, for selected rows and pills.

## Type
- **Families:** Geist Sans for UI; Geist Mono for fingerprints, addresses, codes, timestamps and eyebrow labels. Tabular numerals are on everywhere.
- **Weights:** 400 and 500, plus 600 only for numbers at 24 px and up. Never 700 (Linear, Chronicle).

| Role | Size / leading | Tracking |
|---|---|---|
| Display | 48 / 56 | −0.03em |
| Page title | 28 / 36 | −0.025em |
| Section | 20 / 26 | −0.02em |
| Body | 14 / 20 | 0 |
| Label | 13 / 18 | 0 |
| Meta | 12 / 16 | 0 |
| Eyebrow | 11 / 16 mono, uppercase | +0.12em |

## Shape and space
- **Radii:** 6 for controls, 8 for cards, 12 for panels and sheets, full for pills and switches.
- **Spacing:** 4 8 12 16 20 24 32 40 48 64. Within a group 8–16, between groups 24–32.
- **Shadows:** none on cards. Dialogs use Linear's stacked low-alpha shadow.
- **Grid:** 12 / 6 / 4 columns.

## Layout patterns
- **Inverted-L shell (Linear).** The sidebar sits directly on the chrome background (`--bg #121315`), with no fill and no divider. The page content lives in an inset panel: 8 px margin, 12 px radius, chalk border, `--panel #17181A`, with the dot field.
- **Panel top bar (Linear).** 44 px tall: breadcrumb `Mode / Page` on the left; on the right, a `Search or jump to… ⌘K` button and a notifications icon button.
- **Command menu (Linear).** 600 px wide, 96 px from the top, `--raised` surface, `--shadow-2`. Sections are "Jump to" (`G` + key) and "Actions" (single key), with a footer of key hints.
- **Tabs (Vercel).** Text tabs with no box. A hover highlight slides under the pointer, and a 2 px underline in `--text` slides to the active tab.
- **Toasts (Vercel, Sonner).** Bottom-right, 340 px wide. Collapsed, they stack at −14 px and −5% scale per step, showing at most 3. They fan out on hover, auto-dismiss after 4 s (the timer pauses on hover), and offer Undo.
- **Keyboard hints.** `kbd` chips are 18 px, mono 11 px, with a chalk border. They appear on primary actions and in menus.
- **Rows.** List and table rows are flat (no cards) with a hover fill of `--surface-2`. The selected row uses the same fill; the accent never fills a row.

## Motion
Sources: Linear's own write-ups; Rauno Freiberg on interaction design (Vercel); Emil Kowalski's Sonner and animation notes (Vercel); the Chronicle style analysis.

| Pattern | Spec | From |
|---|---|---|
| Hover on rows, nav, buttons | Appears in 0 ms, fades out over 150 ms | Linear, Rauno |
| Press | `scale(.97)` on `:active`, 150 ms ease-out | Emil (Vercel) |
| Menus and popovers | `scale(.96)` + opacity → 1 in 150 ms ease-out, origin at the trigger; close is a 100 ms fade | Emil |
| Command menu | Same as menus, with a 120 ms backdrop fade. **No animation when opened from the keyboard.** | Rauno, Emil |
| Toasts | Enter `translateY(100%)` → 0 at `transform 400ms ease`; stack and expand transitions also 400 ms | Sonner |
| Tabs | Highlight slides 150 ms ease-out; underline slides 250 ms ease-out | Vercel |
| Page reveal | `blur(2px)` + 6 px rise + opacity → clear over 400 ms `cubic-bezier(.41,.66,.36,.86)`, 40 ms stagger, first load only | Chronicle, Emil (blur trick) |
| Switch | Knob slides 200 ms ease-out, no overshoot | Vercel |
| Chart | Line draws once, 600 ms ease-out | n/a |

- **Ease-out curve:** `cubic-bezier(.23,1,.32,1)`.
- **Rules:**
  - Animate only `transform`, `opacity` and `filter`.
  - Use CSS transitions, not keyframes, for anything that can be interrupted.
  - Nothing over 400 ms.
  - No spring or bounce, no glow, no glass, no infinite pulses.
  - Never animate a keyboard-driven action.
- **Turned off for:** `prefers-reduced-motion` (durations drop to 1 ms), Boomer Mode, and Lite (reveals only).
