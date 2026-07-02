# 0031 — Semantic design tokens + class-based dark mode

## Status

Accepted.

## Context

The frontend used hardcoded Tailwind palette classes (`bg-neutral-50`, `text-neutral-600`,
`bg-yellow-200`, …) scattered across every component, with no dark mode beyond a
`color-scheme` hint. We wanted a coherent black/white + shadow visual identity, a real
dark mode toggled per user, and a small set of accent colors that carry meaning
(move quality: good / mistake / blunder).

## Decision

Introduce a **CSS-variable-backed semantic token layer** in `frontend/src/style.css`,
exposed to Tailwind v4 via `@theme`:

- Tokens: `surface`, `surface-2`, `border`, `fg`, `muted` (chrome) and
  `good` (green), `warn` (orange), `bad` (red), `accent` (green, primary).
- Each Tailwind color token (`--color-surface: var(--surface)`) maps to a raw CSS
  variable. The raw variables are defined under `:root` (light) and re-defined under
  `.dark` (dark), so utilities like `bg-surface` / `text-fg` / `border-border`
  **auto-flip** with the theme — components rarely need a `dark:` prefix.
- Dark mode is **class-based**: `@custom-variant dark (&:where(.dark, .dark *))`. The
  settings store (`stores/settings.ts:applyTheme`) already toggles `.dark` on `<html>`
  from the per-user `theme` preference (`system` resolves via `prefers-color-scheme`).
- Move quality maps to accents in `lib/moveTree.ts:nagClass`: `!`,`!!` → good,
  `?`,`?!` → warn, `??` → bad, `!?` → accent. The selected move uses the accent
  (`bg-accent/15 ring-accent`) instead of the old yellow highlight.

Components migrated from the hardcoded palette to these tokens; data-visualization
grayscale that *encodes* meaning (eval bar fills, WDL bars, the eval-graph canvas, the
violet master-move legend) was intentionally left literal.

## Consequences

- One place to retune the palette / add a theme; dark mode is consistent and instant.
- New components should use the semantic tokens, not raw `neutral-*` / color shades.
  Accent families (`emerald`/`amber`) fold to `accent`/`warn` for consistency.
- Panels adopt `bg-surface` + `shadow-sm` + `rounded-lg` for the shadow motif.
