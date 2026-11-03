import { jsx as _jsx } from "react/jsx-runtime";
import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { HashRouter } from 'react-router';
import App from './App';
// 第三方样式先加载，index.css 最后：这样 @theme token 和 dockview 的覆盖都能生效
import '@xyflow/react/dist/style.css';
import 'dockview/dist/styles/dockview.css';
import './index.css';
createRoot(document.getElementById('root')).render(_jsx(StrictMode, { children: _jsx(HashRouter, { children: _jsx(App, {}) }) }));
