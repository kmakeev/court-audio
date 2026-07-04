import React from 'react';
import ReactDOM from 'react-dom/client';
import { BrowserRouter } from 'react-router-dom';
import { App } from './App';
import { CompactOverlayRoot } from './components/CompactOverlay';
import { isOverlayWindow } from './lib/window-route';
import './styles/globals.css';

// Ветвление окна (этап 10.5): компакт-оверлей — отдельное Tauri-окно, которое
// ядро открывает с URL `index.html?window=overlay`. Там рендерим только
// компактный статус записи (без роутера/входа), иначе — полное приложение.
// Предикат — чистая функция (`lib/window-route`, юнит-тест, R-006).
const isOverlay = isOverlayWindow(window.location.search);

const root = ReactDOM.createRoot(document.getElementById('root') as HTMLElement);

root.render(
  <React.StrictMode>
    {isOverlay ? (
      <CompactOverlayRoot />
    ) : (
      <BrowserRouter>
        <App />
      </BrowserRouter>
    )}
  </React.StrictMode>,
);
