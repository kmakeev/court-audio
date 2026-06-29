# «Правовой кодекс» — design system conventions

A serious, paper-and-ink legal aesthetic ("paper-cream + ink + oxblood") for a
Russian criminal-sentencing expert system. Build screens by composing the real
components below; style your own layout glue with the **CSS-variable tokens** —
never invent hex colors or import a utility-class framework.

## Styling idiom: CSS-variable tokens + inline styles

Components are styled with inline `style={{ ... }}` that reads `var(--token)`.
There is **no Tailwind utility vocabulary** to use (`bg-*`, `p-*`, `text-*` do
not apply here, apart from a couple of internal dark-bar classes). Color,
typography and spacing all go through the tokens declared in `tokens/tokens.css`
(reachable from `styles.css`). Use them directly:

```jsx
<div style={{ background: 'var(--paper-elev)', border: '1px solid var(--hairline)',
              color: 'var(--ink)', fontFamily: 'var(--serif)' }}>…</div>
```

Token families (names are exact; see `tokens/tokens.css` for values):

| Group | Tokens |
|---|---|
| Paper / canvas | `--paper`, `--paper-soft`, `--paper-strong`, `--paper-elev` |
| Ink / text | `--ink`, `--ink-soft`, `--body`, `--muted`, `--muted-soft` |
| Accent (oxblood) | `--accent`, `--accent-deep`, `--accent-soft`, `--accent-tint` |
| Secondary | `--gold`, `--gold-tint`, `--green` |
| Dark surface | `--dark`, `--dark-soft`, `--on-dark`, `--on-dark-soft` |
| Hairlines | `--hairline`, `--hairline-soft` |
| Type | `--serif` (Lora — headings/quotes), `--sans` (Inter — body/UI), `--mono` (JetBrains Mono — numbers, case/article numbers) |

Conventions worth copying from the components: serif (`--serif`) for titles and
legal text; mono (`--mono`, often `className="num"`) for case numbers, article
citations and terms; oxblood `--accent` for primary actions and emphasis;
rectangular shapes with thin `--hairline` borders (radius is mostly 0–2px).

## Setup the host app must provide

- **Fonts** load at runtime from Google Fonts (Lora / Inter / JetBrains Mono) —
  `styles.css` already pulls them via `@import`. Nothing to add.
- **Router**: several components use react-router (`Stepper`, `EmptyState`,
  `CriticalNotice`, `Header`, `AiInitBanner`). Render the app inside a
  react-router context (`<BrowserRouter>`) — they use `<Link>` / `useNavigate`.
- **Icon sprite**: `Icon` renders `<use href="/icons.svg#…">`. The host app
  **must serve the sprite at `/icons.svg`** (ship `public/icons.svg`). Without
  it, icons render empty. Valid `name`s are the `IconName` union in
  `Icon.d.ts` (e.g. `brand-mark`, `icon-step-case`, `icon-pun-prison`,
  `icon-add`, `icon-norm-mitig`, `icon-ai`).

## Brand mark

The logo is `[§]` (oxblood side-brackets + a Lora `§`). Render it with
`<Icon name="brand-mark" />` (or `brand-mark-on-dark` on dark surfaces). The
`ParagraphMark` component renders the same mark as a sized brand block.

## Where the truth lives

- `styles.css` and its `@import` closure (`tokens/tokens.css` for all `--*`
  tokens; `tokens/globals.css` etc. for class-based bits) — read these before
  styling.
- Per component: `<Name>.d.ts` (the prop contract) and `<Name>.prompt.md`.

## Build snippet (idiomatic)

```jsx
import { Card, BlockHead, Button, Tag } from '<pkg>';

function CaseHeader() {
  return (
    <Card>
      <BlockHead numeral="1." title="Иванов Сергей Петрович"
                 hint="2 эпизода · совокупность преступлений" />
      <div style={{ display: 'flex', gap: 8, alignItems: 'center', margin: '8px 0 16px' }}>
        <span className="num" style={{ fontFamily: 'var(--mono)', color: 'var(--ink-soft)' }}>
          ст. 158 ч. 2
        </span>
        <Tag tone="accent">Тяжкое</Tag>
      </div>
      <Button variant="primary">Рассчитать наказание</Button>
    </Card>
  );
}
```

Note: a few components are **app-coupled** and ship as placeholder "floor cards"
(no live preview) — `ArticlePickerModal`, `HandoffModal`,
`CourtCompositionPicker` — they need live data/query context to render.
