"use client";
import { jsx as _jsx } from "react/jsx-runtime";
import { Toaster as Sonner } from "sonner";
import * as React from "react";
/**
 * shadcn 的 sonner 组件默认从 next-themes 取主题；这个原型是 Vite 应用，
 * 主题在 html[data-theme] 上，所以这里只改「从哪儿读主题」，其余照原样。
 */
function useTheme() {
    const [theme, setTheme] = React.useState(() => document.documentElement.dataset.theme || "dark");
    React.useEffect(() => {
        const observer = new MutationObserver(() => {
            setTheme(document.documentElement.dataset.theme || "dark");
        });
        observer.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
        return () => observer.disconnect();
    }, []);
    return theme;
}
const Toaster = ({ ...props }) => {
    const theme = useTheme();
    return (_jsx(Sonner, { theme: theme, className: "toaster group", style: {
            // v14 的 token 名是 --raised / --ink / --line（shadcn 默认的 --popover 这套没有映射成 CSS 变量）
            "--normal-bg": "var(--raised)",
            "--normal-text": "var(--ink)",
            "--normal-border": "var(--line)",
        }, ...props }));
};
export { Toaster };
