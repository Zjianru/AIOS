import { clsx } from 'clsx';
import { extendTailwindMerge } from 'tailwind-merge';
/**
 * 字号是自定义的 text-1 … text-6（index.css 的 --text-*）。
 * 不告诉 tailwind-merge，它会把 text-2 当成颜色，和 text-ink-2 一起出现时把字号删掉。
 */
const twMerge = extendTailwindMerge({
    extend: {
        classGroups: {
            'font-size': [{ text: ['1', '2', '3', '4', '5', '6'] }],
        },
    },
});
export function cn(...inputs) {
    return twMerge(clsx(inputs));
}
