import { useEffect } from 'react';
import { createPortal } from 'react-dom';
import { Button } from '../design';

// Модальное подтверждение действия (этап 04). Стиль — токены PravoUI; своя
// реализация (а не вендоренный BaseModal из ex_system, который app-coupled).
// Доступность: role="dialog" + aria-modal, фокус на подтверждении, Esc/клик по
// фону — отмена, фокус-trap простой (без полного цикла — две кнопки).

interface ConfirmDialogProps {
  open: boolean;
  title: string;
  description?: string;
  confirmLabel: string;
  cancelLabel?: string;
  /** Тон подтверждения: `danger` — primary oxblood (стоп/деструктив). */
  tone?: 'danger' | 'neutral';
  onConfirm: () => void;
  onCancel: () => void;
}

export function ConfirmDialog({
  open,
  title,
  description,
  confirmLabel,
  cancelLabel = 'Отмена',
  tone = 'danger',
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        onCancel();
      }
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [open, onCancel]);

  if (!open) return null;

  return createPortal(
    <div
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onCancel();
      }}
      style={{
        position: 'fixed',
        inset: 0,
        zIndex: 1000,
        background: 'rgba(26, 24, 20, 0.5)',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        padding: 24,
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="confirm-title"
        style={{
          width: 420,
          maxWidth: '100%',
          background: 'var(--paper-elev)',
          border: '1px solid var(--hairline)',
          boxShadow: '0 16px 40px rgba(26, 24, 20, 0.28)',
          padding: '22px 24px',
        }}
      >
        <h2
          id="confirm-title"
          style={{
            margin: '0 0 8px',
            fontFamily: 'var(--serif)',
            fontWeight: 500,
            fontSize: 18,
            color: 'var(--ink)',
          }}
        >
          {title}
        </h2>
        {description && (
          <p style={{ margin: '0 0 18px', fontSize: 13, color: 'var(--ink-soft)', lineHeight: 1.5 }}>
            {description}
          </p>
        )}
        {/* alignItems: center + равная высота — иначе secondary (38) и primary
            (44) кнопки стоят на разной высоте и «прыгают» по вертикали. */}
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'flex-end', gap: 12 }}>
          <Button
            variant="secondary"
            style={{ color: 'var(--ink)', borderColor: 'var(--ink-soft)', height: 44 }}
            onClick={onCancel}
          >
            {cancelLabel}
          </Button>
          <Button
            autoFocus
            variant={tone === 'danger' ? 'primary' : 'secondary'}
            style={
              tone === 'neutral'
                ? { color: 'var(--ink)', borderColor: 'var(--ink-soft)', height: 44 }
                : { height: 44 }
            }
            onClick={onConfirm}
          >
            {confirmLabel}
          </Button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
