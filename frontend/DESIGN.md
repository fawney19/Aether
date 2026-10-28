# Aether UI design contract

## 1. Atmosphere & identity

Preserve the existing compact management UI. Queue visibility is a quiet, textual request status, not a new dashboard or animation.

## 2. Color

Use the semantic Tailwind tokens from `src/style.css` and `src/config/theme.ts`: background, foreground, card, muted, muted-foreground, border, primary and destructive. Queue badges use the existing outline variant with muted-foreground text and a neutral border. No new palette.

## 3. Typography

Keep the existing system sans stack and monospace request metadata. Badge uses text-xs; compact mobile badges retain the existing 10px metadata size. Status labels do not wrap.

## 4. Spacing & layout

Reuse existing table cells and mobile status clusters, their spacing utilities and responsive breakpoints. Do not introduce a new scroll owner or change page geometry.

## 5. Components

The reusable `Badge` primitive is `src/components/ui/badge.vue`. Its outline variant is a rounded-full neutral hairline and tonal card surface. Request status remains in the existing `UsageRecordsTable` desktop status column and mobile model cluster. “排队中” appears only while the backend reports an active concurrency wait. Terminal outcomes take precedence. Ordinary pending requests keep their existing waiting label. Queue state is transient and must clear when admission succeeds, times out or is cancelled.

Concurrency settings reuse `ConcurrencyQueueSetting`, a visible label and the existing `Switch` primitive next to the concurrency limit in normal key, OAuth key, batch-import and both pool batch forms. Normal and OAuth forms use two columns on mobile and four on desktop; the concurrency group spans two columns to keep its label readable. The label “并发满时排队” names the enabled behavior; supporting text explains immediate fallback when disabled. Default is enabled, including older keys without an explicit setting. The switch keeps its setting when the limit is unlimited. Batch changes require independently selecting this setting; otherwise existing values remain unchanged. Labels associate with the switch, keyboard focus remains visible, and no new colors, motion or surface layers are introduced.

## 6. Motion & interaction

The queue indicator is static text without pulse, timer, queue position or new controls. Existing unrelated interactions remain unchanged. Text identifies state without relying on motion or color.

## 7. Depth & surface

Reuse the Badge primitive's rounded-full border and existing surface tokens. No new shadows, stripes or elevation layers.

## 8. Accessibility constraints & accepted debt

Keep the visible Chinese label in both themes and mobile/desktop views. Queue status is informational, not focusable. Check for clipping and terminal-state precedence. This scoped contract documents the existing status primitives; it does not claim a whole-application accessibility audit.
