import { BlockHead, Card, EmptyState, Tag } from '../design';

// Экран «Запись» — заглушка этапа 00. UI записи (устройство, метры,
// старт/стоп/пауза) реализуется на этапе 04 (`promts/04_ui_capture.md`).
export function RecordScreen() {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 20, maxWidth: 880 }}>
      <Card>
        <BlockHead
          numeral="01"
          title="Запись заседания"
          hint="Захват звука, привязка к делу и выгрузка в экспертную систему"
        />
        <div style={{ display: 'flex', gap: 8, marginTop: 4 }}>
          <Tag tone="default">Этап 00 · каркас</Tag>
          <Tag tone="accent">Запись недоступна</Tag>
        </div>
      </Card>
      <EmptyState
        icon="icon-step-report"
        title="Экран записи появится на этапе 04"
        description="Здесь будут выбор устройства, индикаторы уровня и управление сессией (старт · пауза · стоп). Сейчас это каркас приложения."
      />
    </div>
  );
}
