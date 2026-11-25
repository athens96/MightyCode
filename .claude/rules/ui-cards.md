# Card Styling Rule

Do not use the colored left-border card pattern (a thick or colored accent stripe along a card's left/leading edge) anywhere in Mighty Code UI (desktop web `native/windows/web/src` or mobile `mobile/src`).

- **Avoid on cards:** `border-left` / `border-left-color` / `border-inline-start` accent stripes, left-edge `box-shadow: inset <n>px 0 0 ...` bars, or React Native `borderLeftWidth` + `borderLeftColor` accent bars on card containers.
- **Use instead:**
  - Keep card borders uniform on all four sides (`1px solid var(--line)` or a subtle uniform 4-sided `border-color` tint).
  - Show unread, status, or emphasis inside the card header using a small circular dot (`.tone-dot`, `.report-card-unread` — Option ⑨) together with status text/chips.
- **Not affected:** Structural non-card hairlines in `--line` / `--line-strong` (side panel dividers, split-pane separators, sidebar tree indent guides) and Markdown blockquotes.
