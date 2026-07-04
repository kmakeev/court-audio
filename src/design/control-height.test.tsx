import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import { Button } from './Button';
import { Field } from './Field';
import { Select } from './Select';
import { CONTROL_HEIGHT, CONTROL_HEIGHT_COMPACT } from './patterns';

// R-009/R-012: единая высота контролов в дизайн-системе — `Field` и `Select`
// одной высоты в ряду фильтров; кнопке доступен компактный размер ряда, чтобы
// primary-подсказка не была выше соседних secondary-кнопок ролей.

describe('Единая высота контролов формы', () => {
  it('Field и Select берут высоту из общего токена CONTROL_HEIGHT', () => {
    render(
      <>
        <Field label="Поиск" />
        <Select ariaLabel="Статус" value="" onChange={() => {}} options={[]} />
      </>,
    );
    const input = screen.getByLabelText('Поиск') as HTMLInputElement;
    const trigger = screen.getByLabelText('Статус') as HTMLButtonElement;
    // У `Field` высоту из токена держит бордюрная обёртка (border-box включает её
    // рамку), а input тянется на 100% — так итоговая высота совпадает с высотой
    // триггера `Select`, чья рамка тоже входит в его высоту.
    const fieldBox = input.parentElement as HTMLElement;
    expect(fieldBox.style.height).toBe(`${CONTROL_HEIGHT}px`);
    expect(input.style.height).toBe('100%');
    expect(trigger.style.minHeight).toBe(`${CONTROL_HEIGHT}px`);
  });

  it('Button size="sm" приводит к компактной высоте ряда (не меняя вариант)', () => {
    render(
      <Button variant="primary" size="sm">
        Активная дорожка
      </Button>,
    );
    const btn = screen.getByText('Активная дорожка').closest('button') as HTMLButtonElement;
    expect(btn.style.height).toBe(`${CONTROL_HEIGHT_COMPACT}px`);
  });

  it('Button без size сохраняет высоту варианта (primary = CONTROL_HEIGHT)', () => {
    render(<Button variant="primary">Старт</Button>);
    const btn = screen.getByText('Старт').closest('button') as HTMLButtonElement;
    expect(btn.style.height).toBe(`${CONTROL_HEIGHT}px`);
  });
});
