// Общие стиль-паттерны поверх токенов PravoUI (этап 10.5, гигиена стилей).
// Здесь живут инлайн-стили, ранее продублированные по экранам: нейтральная
// кнопка, подпись поля, контейнер экрана. Только токены `var(--*)` — никаких
// самодельных CSS-переменных (соглашение проекта, .design-sync/conventions.md).
import type { CSSProperties } from 'react';

/**
 * Нейтральная кнопка на светлой карточке: вариант `secondary` дизайн-системы
 * рассчитан на тёмную панель (светлый текст `--on-dark`), поэтому на бумаге его
 * нужно переопределить под тёмный текст/рамку — иначе кнопка «исчезает».
 * Единый источник для всех экранов (`Record`/`Settings`/`Export`/… импортируют).
 */
export const NEUTRAL_BTN: CSSProperties = {
  color: 'var(--ink)',
  borderColor: 'var(--ink-soft)',
};

/**
 * Подпись поля/секции: капслок-микротекст над контролом (uppercase, разрядка,
 * приглушённый `--muted`). Базовый набор без раскладки — при необходимости
 * дополняется `display`/`marginBottom` на месте.
 */
export const fieldCaptionStyle: CSSProperties = {
  fontSize: 11,
  textTransform: 'uppercase',
  letterSpacing: '0.14em',
  color: 'var(--muted)',
  fontWeight: 500,
};

/**
 * Контейнер экрана: вертикальный стек карточек с общим шагом и ограничением
 * ширины контента. `width: 100%` + `maxWidth` — чтобы контент тянулся на узких
 * окнах без горизонтального скролла (адаптив, этап 10.5).
 */
export function screenStackStyle(maxWidth: number): CSSProperties {
  return {
    display: 'flex',
    flexDirection: 'column',
    gap: 20,
    width: '100%',
    maxWidth,
  };
}
