# Провенанс PravoUI в «Аудиопротокол»

`court-audio` — **отдельный репозиторий** и при сборке не имеет доступа к
`ex_system_front`. Поэтому дизайн-система «Правовой кодекс» (PravoUI)
переиспользуется как **вендоренный снапшот** (а не npm-зависимость): нужное
подмножество физически скопировано сюда. Это решение этапа 00 (см.
`promts/00_infra.md`, шаг 4).

## Источник

- Репозиторий: `~/Progs/ex_system/ex_system_front`
- Снапшот снят: **2026-06-29**
- Регламент исходной DS: [`conventions.md`](conventions.md) (копия из источника)

## Что именно вендорится

| Файл здесь | Источник в `ex_system_front` |
|---|---|
| `src/styles/tokens.css` | `src/styles/tokens.css` (один-в-один) |
| `src/styles/globals.css` (фрагменты) | `.design-sync/ds-extra.css` (шрифты + `.bg-dark`/`.text-on-dark*`) + `src/styles/responsive.css` (`.empty-state*`) + `globals.css` (`.num`) |
| `public/icons.svg` | `ds-bundle/icons.svg` |
| `src/design/Icon.tsx` | `src/components/design/Icon.tsx` |
| `src/design/ParagraphMark.tsx` | `src/components/design/ParagraphMark.tsx` |
| `src/design/Button.tsx` | `src/components/design/Button.tsx` |
| `src/design/Card.tsx` | `src/components/design/Card.tsx` |
| `src/design/BlockHead.tsx` | `src/components/design/BlockHead.tsx` |
| `src/design/Tag.tsx` | `src/components/design/Tag.tsx` |
| `src/design/EmptyState.tsx` | `src/components/design/EmptyState.tsx` |
| `src/design/Checkbox.tsx` | `src/components/design/Checkbox.tsx` |
| `src/design/Field.tsx` *(этап 04)* | `src/components/design/Field.tsx` |
| `src/design/Select.tsx` *(этап 04)* | `src/components/design/Select.tsx` |
| `src/design/CriticalNotice.tsx` *(этап 04)* | `src/components/design/CriticalNotice.tsx` |
| `src/design/InfoTip.tsx` *(этап 04)* | `src/components/design/InfoTip.tsx` |
| `src/design/Skeleton.tsx` *(этап 04)* | `src/components/design/Skeleton.tsx` |
| `src/styles/globals.css` (`.skeleton`/`.infotip-pop`, этап 04) | `src/styles/skeleton.css` + `src/styles/responsive.css` (`.infotip-pop`) |

## Что НЕ вендорится (намеренно)

- `Header`/`Footer` из `ex_system` — app-coupled (auth store, нотификации,
  фидбэк, конфиг бренда). Вместо них — собственный `src/shell/AppShell.tsx` на
  токенах PravoUI.
- Остальные компоненты DS — по мере надобности следующих этапов.

## Правила сопровождения

- Компоненты стилизуются **только токенами** (`var(--…)` из `tokens.css`) —
  не задавать hex/CSS-переменные вручную (правило CLAUDE.md).
- `public/icons.svg` обслуживается приложением по пути `/icons.svg` (его
  ожидает `Icon` через `<use href="/icons.svg#…">`).
- При обновлении DS в `ex_system_front` — пересинхронизировать перечисленные
  файлы и обновить дату снапшота выше.
