import React from 'react';
import ReactDOM from 'react-dom/client';
// 副作用导入：必须先于 App，保证 App 顶层调用 getCurrentWindow() 时已有 IPC 垫片
import './utils/browserShim';
import App from './App';
import './index.css';

ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
