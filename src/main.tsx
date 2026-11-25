import React from 'react';
import ReactDOM from 'react-dom/client';
import '@fontsource/jetbrains-mono/latin-400.css';
import '@fontsource/jetbrains-mono/latin-700.css';
import App from './App';
import { isTauri } from '@tauri-apps/api/core';
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
import { TrayPanel } from './components/TrayPanel';
import { PetWindow } from './components/PetWindow';
import './index.css';

const label = isTauri() ? getCurrentWebviewWindow().label : 'main';
const trayPanel = label === 'bento-tray-panel';
const pet = label === 'bento-pet' || label === 'bento-pet-dock';
ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
  <React.StrictMode>
    {pet ? <PetWindow /> : trayPanel ? <TrayPanel /> : <App />}
  </React.StrictMode>,
);
