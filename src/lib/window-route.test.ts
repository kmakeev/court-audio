import { describe, it, expect } from 'vitest';
import { isOverlayWindow, OVERLAY_WINDOW } from './window-route';

describe('isOverlayWindow — маршрут окна (R-006)', () => {
  it('распознаёт окно оверлея по ?window=overlay', () => {
    expect(isOverlayWindow('?window=overlay')).toBe(true);
    expect(isOverlayWindow(`?window=${OVERLAY_WINDOW}`)).toBe(true);
  });

  it('распознаёт оверлей среди других query-параметров', () => {
    expect(isOverlayWindow('?foo=1&window=overlay&bar=2')).toBe(true);
  });

  it('основное окно — не оверлей', () => {
    expect(isOverlayWindow('')).toBe(false);
    expect(isOverlayWindow('?window=main')).toBe(false);
    expect(isOverlayWindow('?other=overlay')).toBe(false);
    // Точное совпадение значения: подстрока не проходит.
    expect(isOverlayWindow('?window=overlay-x')).toBe(false);
  });
});
