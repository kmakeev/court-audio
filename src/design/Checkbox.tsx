import { forwardRef, useCallback, useEffect, useId, useRef, useState } from 'react';
import type { ChangeEvent, InputHTMLAttributes, ReactNode } from 'react';

/**
 * Кастомный чекбокс в стиле дизайн-системы (а не нативный OS-вид).
 * Создан 2026-05-28 (раунд 06_4 / S.2) по аналогии с `Select.tsx`.
 *
 * Совместим с `react-hook-form` register'ом — благодаря forwardRef и
 * стандартному `<input type="checkbox">` под капотом (visually hidden,
 * но реально кликабельный и доступный для form submit / валидации).
 * Поверх — кастомный квадрат, повторяющий токены дизайн-системы
 * (`--paper-elev` / `--ink` / `--accent-deep` / `--hairline`).
 *
 * Использование:
 * ```tsx
 * // Контролируемый
 * <Checkbox checked={value} onChange={(e) => setValue(e.target.checked)}>
 *   Применить ст. 64
 * </Checkbox>
 *
 * // С RHF
 * <Checkbox {...register('is_conditional')}>Условное</Checkbox>
 * ```
 */

interface CheckboxProps extends Omit<InputHTMLAttributes<HTMLInputElement>, 'type' | 'children'> {
  /** Текст лейбла. Если нужна более сложная разметка — используйте `children`. */
  label?: ReactNode;
  /** ReactNode-лейбл (имеет приоритет над `label`). */
  children?: ReactNode;
  /** Поведение лейбла: 'inline' (по умолчанию — рядом) или 'block'. */
  layout?: 'inline' | 'block';
}

export const Checkbox = forwardRef<HTMLInputElement, CheckboxProps>(function Checkbox(
  { id, label, children, layout = 'inline', disabled, style, className, onChange, checked, defaultChecked, ...rest },
  ref,
) {
  const reactId = useId();
  const inputId = id ?? `cb-${reactId}`;
  const content = children ?? label;

  // 06.4.c (2026-05-31): визуал checkmark'а через React JSX вместо CSS
  // ::after + sibling селектора. Раньше CSS `input:checked + .cb-box`
  // не срабатывал в одном из сценариев (вероятно, StrictMode remount
  // или forwarded-ref переписывал input.checked → cascade не обновлялся),
  // и галочка оставалась невидимой при изменённом RHF state. Здесь —
  // локальный state, синкается с DOM через onChange + ref + поллинг
  // на каждый render (для programmatic setValue() через RHF).
  const innerRef = useRef<HTMLInputElement | null>(null);
  const [internalChecked, setInternalChecked] = useState<boolean>(
    !!(checked ?? defaultChecked ?? false),
  );

  // Подключаем forwarded ref + локальный — оба указывают на тот же узел.
  const setRefs = useCallback((node: HTMLInputElement | null) => {
    innerRef.current = node;
    if (typeof ref === 'function') ref(node);
    else if (ref) (ref as React.MutableRefObject<HTMLInputElement | null>).current = node;
  }, [ref]);

  // Sync visual с DOM на каждый render. RHF при `setValue('field', v)`
  // переписывает input.checked напрямую (uncontrolled mode), JSX
  // не уведомляется. Этот эффект подтягивает текущее значение.
  useEffect(() => {
    const node = innerRef.current;
    if (!node) return;
    if (node.checked !== internalChecked) {
      setInternalChecked(node.checked);
    }
  });

  // Если родитель использует controlled-mode (`checked={...}`) — синкаем.
  useEffect(() => {
    if (checked !== undefined) setInternalChecked(!!checked);
  }, [checked]);

  const handleChange = (e: ChangeEvent<HTMLInputElement>) => {
    setInternalChecked(e.target.checked);
    onChange?.(e);
  };

  return (
    <label
      htmlFor={inputId}
      className={className}
      style={{
        display: layout === 'block' ? 'flex' : 'inline-flex',
        alignItems: 'flex-start',
        gap: 8,
        cursor: disabled ? 'not-allowed' : 'pointer',
        fontSize: 13,
        color: disabled ? 'var(--muted)' : 'var(--ink)',
        lineHeight: 1.5,
        userSelect: 'none',
        ...style,
      }}
    >
      <span
        style={{
          position: 'relative',
          display: 'inline-flex',
          flexShrink: 0,
          width: 16,
          height: 16,
          marginTop: 2,
        }}
      >
        {/* Скрытый, но кликабельный нативный input — отвечает за state, RHF и a11y. */}
        <input
          ref={setRefs}
          id={inputId}
          type="checkbox"
          disabled={disabled}
          checked={checked}
          defaultChecked={checked === undefined ? defaultChecked : undefined}
          onChange={handleChange}
          style={{
            position: 'absolute',
            inset: 0,
            margin: 0,
            opacity: 0,
            cursor: disabled ? 'not-allowed' : 'pointer',
            zIndex: 2,
          }}
          {...rest}
        />
        {/* Визуальная коробка. Стилизация по `data-checked` —
            гарантирует синхронизацию с React state, не зависит от CSS
            `:checked` cascade. */}
        <span
          aria-hidden="true"
          className="cb-box"
          data-checked={internalChecked ? 'true' : 'false'}
          data-disabled={disabled ? 'true' : undefined}
          style={{
            position: 'absolute',
            inset: 0,
            background: internalChecked
              ? (disabled ? 'var(--muted)' : 'var(--accent-deep)')
              : (disabled ? 'var(--paper-soft)' : 'var(--paper-elev)'),
            border: `1px solid ${internalChecked
              ? (disabled ? 'var(--muted)' : 'var(--accent-deep)')
              : 'var(--hairline)'}`,
            pointerEvents: 'none',
            opacity: disabled ? 0.6 : 1,
          }}
        >
          {internalChecked && (
            <span
              aria-hidden="true"
              style={{
                position: 'absolute',
                top: 2,
                left: 5,
                width: 4,
                height: 8,
                borderStyle: 'solid',
                borderColor: 'var(--on-dark, #fff)',
                borderWidth: '0 1.5px 1.5px 0',
                transform: 'rotate(45deg)',
                pointerEvents: 'none',
              }}
            />
          )}
        </span>
      </span>
      {content != null && (
        <span style={{ flex: 1 }}>{content}</span>
      )}
    </label>
  );
});
