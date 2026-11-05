import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { HashRouter } from 'react-router';
import App from './App';
// 第三方样式先加载，index.css 最后：这样 @theme token 和 dockview 的覆盖都能生效
import '@xyflow/react/dist/style.css';
import 'dockview/dist/styles/dockview.css';
import './index.css';
import { reportMonitor } from './data/service';

// T011.6：页面没接住的异常报给服务记监控（内容限长；失败静默）。
window.addEventListener('error', (event) => {
  void reportMonitor('page-error', String(event.message || event.error || '页面错误'));
});
window.addEventListener('unhandledrejection', (event) => {
  void reportMonitor('page-error', String(event.reason ?? '未处理的 Promise 拒绝'));
});

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <HashRouter>
      <App />
    </HashRouter>
  </StrictMode>,
);
