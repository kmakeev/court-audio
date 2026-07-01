# Оффлайн-рантайм WebView2 (Windows)

Этот каталог — место для **фиксированной версии Microsoft Edge WebView2
Runtime**, которую Tauri бандлит в Windows-инсталлятор, чтобы установка в
**закрытом контуре суда шла без интернета** (не онлайн-бутстраппер).

Ссылка из [`../tauri.conf.json`](../tauri.conf.json):

```json
"windows": {
  "webviewInstallMode": { "type": "fixedRuntime", "path": "webview2/" }
}
```

`path` указывается **относительно `tauri.conf.json`**, поэтому распакованный
рантайм должен лежать именно здесь — в `src-tauri/webview2/`.

## Что положить

1. Скачать **Fixed Version** дистрибутив WebView2 Runtime (архив `.cab`) с
   портала Microsoft для нужной архитектуры (`x64`). Версия фиксируется
   сознательно — это build-метаданные, не рантайм-настройка (см.
   [`../../docs/configuration.md`](../../docs/configuration.md), раздел о
   границе «настройки vs упаковка» в [`../../docs/packaging.md`](../../docs/packaging.md)).
2. Распаковать `.cab` так, чтобы содержимое (папка вида
   `Microsoft.WebView2.FixedVersionRuntime.<версия>.x64/`) оказалось внутри
   этого каталога.
3. Собрать Windows-инсталлятор: `npm run tauri build`.

## Почему бинарники не в git

Рантайм WebView2 весит сотни МБ и имеет собственную лицензию Microsoft —
в репозиторий не коммитим (см. [`../../.gitignore`](../../.gitignore):
игнорируется всё в `webview2/`, кроме этого README). В CI рантайм
подкладывается шагом загрузки/секретом перед сборкой Windows-артефакта.

> Фиксированную версию согласовывать при обновлении: смена версии WebView2 —
> изменение дистрибутива, отражать в [`../../CHANGELOG.md`](../../CHANGELOG.md).
