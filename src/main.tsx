import React from 'react';
import ReactDOM from 'react-dom/client';
import '@fontsource/jetbrains-mono/latin-400.css';
import '@fontsource/jetbrains-mono/latin-700.css';
import App from './App';
import { isTauri } from '@tauri-apps/api/core';
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
import { TrayPanel } from './components/TrayPanel';
import './index.css';

const trayPanel = isTauri() && getCurrentWebviewWindow().label === 'bento-tray-panel';
ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
  <React.StrictMode>
    {trayPanel ? <TrayPanel /> : <App />}
  </React.StrictMode>,
);
