import { BlockHead, Card, EmptyState } from '../design';

// Экран «Сессии» — заглушка этапа 00. Список записанных сессий и их статус
// выгрузки наполняется на этапах 04+.
export function SessionsScreen() {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 20, maxWidth: 880 }}>
      <Card>
        <BlockHead
          numeral="02"
          title="Сессии"
          hint="История записей станции и статус их выгрузки"
        />
      </Card>
      <EmptyState
        icon="icon-step-case"
        title="Записанных сессий пока нет"
        description="После реализации записи (этап 04) здесь появится список сессий с длительностью, размером и статусом выгрузки в ex_system."
      />
    </div>
  );
}
