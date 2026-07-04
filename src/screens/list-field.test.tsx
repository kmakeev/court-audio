import { describe, it, expect, vi } from 'vitest';
import { useState } from 'react';
import { fireEvent, render, screen } from '@testing-library/react';
import { ListField, parseNumberList, splitList } from './settings-common';

// R-007: поля «через запятую» правят сырой текст и парсят его только на blur —
// каретка не прыгает, запятую/пробел в середине набрать можно.

/** Обёртка: держит массив в состоянии, как экран «Настройки». */
function Harness({
  initial,
  parse,
  onCommit,
}: {
  initial: string[] | number[];
  parse: (raw: string) => string[] | number[];
  onCommit?: (v: string[] | number[]) => void;
}) {
  const [value, setValue] = useState<string[] | number[]>(initial);
  return (
    <ListField
      label="Роли говорящих"
      value={value as never}
      parse={parse as never}
      onCommit={(v) => {
        setValue(v as string[] | number[]);
        onCommit?.(v as string[] | number[]);
      }}
    />
  );
}

describe('ListField (поля «через запятую», R-007)', () => {
  it('сырой текст не реформатируется на каждый ввод — запятая набирается', () => {
    render(<Harness initial={['judge', 'clerk']} parse={splitList} />);
    const input = screen.getByLabelText('Роли говорящих') as HTMLInputElement;

    // Хвостовая запятая (начало нового элемента) сохраняется в буфере, не
    // отбрасывается фильтром пустых сегментов.
    fireEvent.change(input, { target: { value: 'judge, clerk,' } });
    expect(input.value).toBe('judge, clerk,');

    // Пробел в середине не «съедается» trim — оператор печатает свободно.
    fireEvent.change(input, { target: { value: 'judge, clerk, wit ' } });
    expect(input.value).toBe('judge, clerk, wit ');
  });

  it('парсит и коммитит массив только на blur', () => {
    const committed = vi.fn();
    render(<Harness initial={['judge']} parse={splitList} onCommit={committed} />);
    const input = screen.getByLabelText('Роли говорящих') as HTMLInputElement;

    fireEvent.change(input, { target: { value: 'judge, clerk, room' } });
    // Пока поле в фокусе — модель не трогаем.
    expect(committed).not.toHaveBeenCalled();

    fireEvent.blur(input);
    expect(committed).toHaveBeenCalledTimes(1);
    expect(committed).toHaveBeenCalledWith(['judge', 'clerk', 'room']);
    // После blur буфер нормализуется к каноничному виду.
    expect(input.value).toBe('judge, clerk, room');
  });

  it('каретка остаётся на месте вставки (не прыгает в конец)', () => {
    render(<Harness initial={['a', 'b']} parse={splitList} />);
    const input = screen.getByLabelText('Роли говорящих') as HTMLInputElement;

    // Вставляем запятую после «a»: значение «a,, b», каретка после первой «,».
    fireEvent.change(input, { target: { value: 'a,, b' } });
    input.setSelectionRange(2, 2);
    // Значение сохранено как есть (без реформата) → каретка не сброшена в конец.
    expect(input.value).toBe('a,, b');
    expect(input.selectionStart).toBe(2);
  });

  it('числовой список (скорости): парсит на blur через parseNumberList', () => {
    const committed = vi.fn();
    render(<Harness initial={[1]} parse={parseNumberList} onCommit={committed} />);
    const input = screen.getByLabelText('Роли говорящих') as HTMLInputElement;

    fireEvent.change(input, { target: { value: '0.5, 1.0, 2' } });
    fireEvent.blur(input);
    expect(committed).toHaveBeenCalledWith([0.5, 1.0, 2]);
  });
});
