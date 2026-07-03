import { describe, expect, it } from 'vitest';
import { humanizeError } from './errors';

// Словарь человекочитаемых ошибок ядра (этап 10.6, мелочи трения).

describe('humanizeError', () => {
  it('переводит известные ошибки ядра в понятный текст', () => {
    expect(humanizeError('Требуется вход оператора: авторизуйтесь перед началом записи')).toContain(
      'Войдите как оператор',
    );
    expect(humanizeError('не задан адрес сервера ex_system (настройки → выгрузка)')).toContain(
      'адрес сервера ex_system',
    );
    expect(humanizeError('неверный PIN')).toContain('Неверный PIN');
  });

  it('распознаёт сетевые ошибки reqwest', () => {
    expect(humanizeError('error sending request: operation timed out')).toContain('не отвечает');
    expect(humanizeError('tcp connect error: Connection refused')).toContain('Нет связи');
  });

  it('регистронезависим', () => {
    expect(humanizeError('НЕВЕРНЫЙ PIN')).toContain('Неверный PIN');
  });

  it('неизвестную ошибку отдаёт как есть', () => {
    expect(humanizeError('что-то совсем экзотическое пошло не так')).toBe(
      'что-то совсем экзотическое пошло не так',
    );
  });

  it('обрабатывает Error и не-строки', () => {
    expect(humanizeError(new Error('неверный PIN'))).toContain('Неверный PIN');
    expect(humanizeError(undefined)).toBe('неизвестная ошибка');
  });
});
