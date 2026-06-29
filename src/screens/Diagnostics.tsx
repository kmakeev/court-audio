import { BlockHead, Card, EmptyState } from '../design';

// Экран «Диагностика» — заглушка этапа 00. Здоровье устройства, свободное
// место, состояние watchdog и очереди выгрузки появятся на этапах 02/06.
export function DiagnosticsScreen() {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 20, maxWidth: 880 }}>
      <Card>
        <BlockHead
          numeral="04"
          title="Диагностика"
          hint="Состояние устройства, диска, watchdog и очереди выгрузки"
        />
      </Card>
      <EmptyState
        icon="icon-ai"
        title="Диагностика появится на этапах 02 и 06"
        description="Здесь будут индикаторы здоровья записи: устройство ввода, свободное место, watchdog и оффлайн-очередь выгрузки."
      />
    </div>
  );
}
