// @vitest-environment node
//
// Смоук-тест автономного HTML-плеера экспортной копии (этап 10.2). Читает
// РЕАЛЬНЫЙ файл шаблона `src-tauri/src/export/player_template.html` (единый
// источник истины — не дублируем разметку в фикстуре), подставляет данные
// тем же способом, что и Rust-рендер (`export::html::render`), и исполняет
// встроенный JS в отдельном `JSDOM`-окне (`runScripts: 'dangerously'`) —
// глобальное jsdom-окружение vitest не исполняет инлайновые `<script>` при
// установке `innerHTML`, поэтому нужен полноценный, изолированный документ.
// `@vitest-environment node` — чтобы не заводить второй jsdom поверх
// глобального окружения компонентных тестов.
import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { JSDOM } from 'jsdom';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const TEMPLATE_PATH = path.resolve(
  __dirname,
  '../../src-tauri/src/export/player_template.html',
);
const DATA_PLACEHOLDER = '__EXPORT_PLAYER_DATA__';

interface PlayerTrackView {
  file: string;
  label: string;
  role: string;
}

interface MarkerState {
  id: string;
  category: string;
  comment: string | null;
  offset_samples: number;
  offset_ms: number;
  operator_id: string;
  at_unix_ms: number;
}

interface RoleSpanState {
  id: string;
  role: string;
  start_offset_samples: number;
  start_offset_ms: number;
  end_offset_samples: number | null;
  end_offset_ms: number | null;
  operator_id: string;
  at_unix_ms: number;
}

interface PlayerData {
  session_id: string;
  started_at_unix_ms: number;
  adjudication_ref: string | null;
  tracks: PlayerTrackView[];
  markers: MarkerState[];
  role_spans: RoleSpanState[];
  duration_ms: number;
  seek_step_seconds: number;
  playback_rates: number[];
}

function fixtureData(over: Partial<PlayerData> = {}): PlayerData {
  return {
    session_id: 'session-1700000000000',
    started_at_unix_ms: 1_700_000_000_000,
    adjudication_ref: '№ 1-123/2026',
    tracks: [
      { file: 'audio/judge.wav', label: 'Судья', role: 'judge' },
      { file: 'audio/defense.wav', label: 'Защита', role: 'defense' },
    ],
    markers: [
      {
        id: 'm1',
        category: 'Инцидент',
        comment: 'шум в зале',
        offset_samples: 44_100,
        offset_ms: 1_000,
        operator_id: 'op-1',
        at_unix_ms: 1_700_000_001_000,
      },
    ],
    role_spans: [
      {
        id: 'r1',
        role: 'judge',
        start_offset_samples: 88_200,
        start_offset_ms: 2_000,
        end_offset_samples: 132_300,
        end_offset_ms: 3_000,
        operator_id: 'op-1',
        at_unix_ms: 1_700_000_002_000,
      },
    ],
    duration_ms: 125_000,
    seek_step_seconds: 15,
    playback_rates: [0.5, 1, 2],
    ...over,
  };
}

/** Та же подстановка, что делает Rust `export::html::render`. */
function renderTemplate(data: PlayerData): string {
  const template = readFileSync(TEMPLATE_PATH, 'utf-8');
  const json = JSON.stringify(data).replace(/<\//g, '<\\/');
  return template.replace(DATA_PLACEHOLDER, json);
}

/** Смонтировать шаблон в исполняемом JSDOM-окне и застабить медиа-методы
 * (jsdom не умеет декодировать/проигрывать аудио — стандартный обход). */
function mountPlayer(data: PlayerData) {
  const html = renderTemplate(data);
  const dom = new JSDOM(html, { runScripts: 'dangerously', resources: 'usable' });
  const { window } = dom;

  window.HTMLMediaElement.prototype.play = function (this: HTMLMediaElement) {
    Object.defineProperty(this, 'paused', { value: false, configurable: true });
    this.dispatchEvent(new window.Event('play'));
    return Promise.resolve();
  };
  window.HTMLMediaElement.prototype.pause = function (this: HTMLMediaElement) {
    Object.defineProperty(this, 'paused', { value: true, configurable: true });
    this.dispatchEvent(new window.Event('pause'));
  };
  window.HTMLMediaElement.prototype.load = function () {};

  return dom;
}

describe('автономный HTML-плеер экспортной копии (шаблон)', () => {
  it('содержит единственный <audio> с относительным src первой дорожки', () => {
    const dom = mountPlayer(fixtureData());
    const audios = dom.window.document.querySelectorAll('audio');
    expect(audios.length).toBe(1);
    expect(audios[0]?.getAttribute('src')).toBe('audio/judge.wav');
  });

  it('клик по play/pause переключает состояние и вызывает play()/pause() у <audio>', () => {
    const dom = mountPlayer(fixtureData());
    const { document } = dom.window;
    const audio = document.getElementById('audio') as HTMLAudioElement;
    const playBtn = document.getElementById('play-btn') as HTMLButtonElement;

    expect(playBtn.textContent).toContain('Играть');
    playBtn.click();
    expect(audio.paused).toBe(false);
    expect(playBtn.textContent).toContain('Пауза');

    playBtn.click();
    expect(audio.paused).toBe(true);
    expect(playBtn.textContent).toContain('Играть');
  });

  it('кнопка перемотки вперёд сдвигает currentTime на seek_step_seconds', () => {
    const dom = mountPlayer(fixtureData({ seek_step_seconds: 15 }));
    const { document } = dom.window;
    const audio = document.getElementById('audio') as HTMLAudioElement;
    const fwdBtn = document.getElementById('seek-fwd-btn') as HTMLButtonElement;

    expect(audio.currentTime).toBe(0);
    fwdBtn.click();
    expect(audio.currentTime).toBe(15);
  });

  it('привязка к делу рендерится как читаемая строка, а не сырой JSON', () => {
    const dom = mountPlayer(
      fixtureData({
        adjudication_ref: JSON.stringify({
          kind: 'manual',
          raw_number: '№ 1-777/2026',
          raw_fio: 'Иванов И.И.',
        }),
      }),
    );
    const hint = dom.window.document.getElementById('case-hint');
    expect(hint?.textContent).toBe('№ 1-777/2026, Иванов И.И.');
    expect(hint?.textContent).not.toContain('"kind"');
  });

  it('привязка к делу без номера/ФИО показывает исходную строку (легаси-формат)', () => {
    const dom = mountPlayer(fixtureData({ adjudication_ref: '№ 5-1/2026 (произвольная строка)' }));
    const hint = dom.window.document.getElementById('case-hint');
    expect(hint?.textContent).toBe('№ 5-1/2026 (произвольная строка)');
  });

  it('отсутствие привязки к делу показывает заглушку', () => {
    const dom = mountPlayer(fixtureData({ adjudication_ref: null }));
    const hint = dom.window.document.getElementById('case-hint');
    expect(hint?.textContent).toBe('Без привязки к делу');
  });

  it('выбор скорости из playback_rates через кастомный список выставляет audio.playbackRate', () => {
    const dom = mountPlayer(fixtureData({ playback_rates: [0.5, 1, 2] }));
    const { document } = dom.window;
    const audio = document.getElementById('audio') as HTMLAudioElement;
    const rateDropdown = document.getElementById('rate-dropdown') as HTMLElement;
    const trigger = rateDropdown.querySelector('.dropdown-trigger') as HTMLButtonElement;

    expect(trigger.textContent).toBe('1×');
    trigger.click();
    const options = rateDropdown.querySelectorAll('.dropdown-option');
    expect(options.length).toBe(3);
    (options[2] as HTMLLIElement).click(); // "2×"
    expect(audio.playbackRate).toBe(2);
    expect(trigger.textContent).toBe('2×');
    // Список закрывается сразу после выбора.
    expect((rateDropdown.querySelector('.dropdown-list') as HTMLElement).hidden).toBe(true);
  });

  it('открытый список скорости стилизован (не системная тема — нет native <select>/<option>)', () => {
    const dom = mountPlayer(fixtureData());
    expect(dom.window.document.querySelectorAll('select').length).toBe(0);
    const trigger = dom.window.document.querySelector('#rate-dropdown .dropdown-trigger');
    expect(trigger).not.toBeNull();
  });

  it('Escape закрывает открытый список скорости', () => {
    const dom = mountPlayer(fixtureData());
    const { document, KeyboardEvent } = dom.window;
    const rateDropdown = document.getElementById('rate-dropdown') as HTMLElement;
    (rateDropdown.querySelector('.dropdown-trigger') as HTMLButtonElement).click();
    expect((rateDropdown.querySelector('.dropdown-list') as HTMLElement).hidden).toBe(false);

    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
    expect((rateDropdown.querySelector('.dropdown-list') as HTMLElement).hidden).toBe(true);
  });

  it('открытие списка дорожки закрывает уже открытый список скорости', () => {
    const dom = mountPlayer(fixtureData());
    const { document } = dom.window;
    const rateList = document.querySelector('#rate-dropdown .dropdown-list') as HTMLElement;
    const trackTrigger = document.querySelector('#track-dropdown .dropdown-trigger') as HTMLButtonElement;

    (document.querySelector('#rate-dropdown .dropdown-trigger') as HTMLButtonElement).click();
    expect(rateList.hidden).toBe(false);

    trackTrigger.click();
    expect(rateList.hidden).toBe(true);
  });

  it('клик по строке оглавления выставляет currentTime = offset_ms/1000 и запускает play()', () => {
    const dom = mountPlayer(fixtureData());
    const { document } = dom.window;
    const audio = document.getElementById('audio') as HTMLAudioElement;
    const firstItemBtn = document.querySelector('#toc li button') as HTMLButtonElement;

    expect(firstItemBtn).not.toBeNull();
    firstItemBtn.click();
    // Первая запись оглавления после сортировки по offset — метка m1 (1000мс).
    expect(audio.currentTime).toBe(1);
    expect(audio.paused).toBe(false);
  });

  it('при нескольких файлах переключатель дорожки (кастомный список) меняет audio.src', () => {
    const dom = mountPlayer(fixtureData());
    const { document } = dom.window;
    const audio = document.getElementById('audio') as HTMLAudioElement;
    const trackDropdown = document.getElementById('track-dropdown') as HTMLElement;

    expect(trackDropdown.style.display).not.toBe('none');
    const trigger = trackDropdown.querySelector('.dropdown-trigger') as HTMLButtonElement;
    trigger.click();
    const options = trackDropdown.querySelectorAll('.dropdown-option');
    expect(options.length).toBe(2);
    expect(options[1]?.textContent).toBe('Защита');
    (options[1] as HTMLLIElement).click();
    expect(audio.getAttribute('src')).toBe('audio/defense.wav');
  });

  it('трек-переключатель скрыт при одной дорожке в пакете', () => {
    const dom = mountPlayer(
      fixtureData({ tracks: [{ file: 'audio/mix.wav', label: 'Микс всех дорожек', role: 'mix' }] }),
    );
    const trackDropdown = dom.window.document.getElementById('track-dropdown') as HTMLElement;
    expect(trackDropdown.style.display).toBe('none');
  });

  it('кнопка play/pause той же высоты, что остальные кнопки ряда, и не меняет ширину при переключении', () => {
    // jsdom не считает раскладку (getComputedStyle не даёт реальных box-
    // размеров без layout-движка), поэтому проверяем источник CSS:
    // - высота задаётся ОБЩИМ правилом `.btn` (единая для play/pause и
    //   кнопок перемотки) и не переопределяется ни `#play-btn`, ни
    //   `.btn.primary` — иначе play/pause был бы выше соседних кнопок;
    // - минимальная ширина закреплена за `#play-btn`, чтобы разная длина
    //   текста «Играть»/«Пауза» не двигала соседние элементы ряда.
    const dom = mountPlayer(fixtureData());
    const { document } = dom.window;
    const styleText = Array.from(document.querySelectorAll('style'))
      .map((s) => s.textContent || '')
      .join('\n');

    const baseBtnRule = styleText.match(/\.btn\s*\{([^}]*)\}/);
    expect(baseBtnRule?.[1]).toMatch(/height\s*:/);

    const playBtnRule = styleText.match(/#play-btn\s*\{([^}]*)\}/);
    expect(playBtnRule).not.toBeNull();
    expect(playBtnRule?.[1]).toMatch(/min-width\s*:/);
    expect(playBtnRule?.[1]).not.toMatch(/height\s*:/);

    const primaryRule = styleText.match(/\.btn\.primary\s*\{([^}]*)\}/);
    expect(primaryRule?.[1]).not.toMatch(/height\s*:/);

    const playBtn = document.getElementById('play-btn') as HTMLButtonElement;
    expect(playBtn.classList.contains('btn')).toBe(true);
    playBtn.click();
    expect(playBtn.classList.contains('primary')).toBe(false);
  });

  it('хоткей «пробел» и стрелки работают так же, как кнопки транспорта', () => {
    const dom = mountPlayer(fixtureData({ seek_step_seconds: 15 }));
    const { document, KeyboardEvent } = dom.window;
    const audio = document.getElementById('audio') as HTMLAudioElement;

    document.dispatchEvent(new KeyboardEvent('keydown', { key: ' ' }));
    expect(audio.paused).toBe(false);

    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight' }));
    expect(audio.currentTime).toBe(15);
  });

  it('не содержит внешних ссылок/скриптов (офлайн-гарантия)', () => {
    const html = renderTemplate(fixtureData());
    expect(html).not.toContain('http://');
    expect(html).not.toContain('https://');
    expect(html).not.toMatch(/<script\s+src=/);
    expect(html).not.toMatch(/<link\s+href=/);
  });
});
