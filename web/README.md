# AIOS Web

T007 起，Kind 对话页面进入仓库并走项目门禁。原型标准在 `~/Documents/aios-design/app-v14`（只读），不要改原型。

## 开发与门禁

```
npm ci
npm run typecheck      # tsc -b
npm run build          # 产出 dist/（aios-dev serve 同源托管）
npm run e2e            # Playwright（接缝 5：真 serve + 假 DeepSeek 回放）
npm run check:markers  # 产品里不得出现原型专属标记
```

依赖照原型 package.json，只额外加了 Playwright。npm install 只在 `web/` 里执行。
