"use client";
import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
import * as React from "react";
import { cn } from "@/lib/utils";
import { ScrollArea as ScrollAreaPrimitive } from "radix-ui";
function ScrollArea({ className, children, ...props }) {
    return (_jsxs(ScrollAreaPrimitive.Root, { "data-slot": "scroll-area", className: cn("relative", className), ...props, children: [_jsx(ScrollAreaPrimitive.Viewport, { "data-slot": "scroll-area-viewport", className: "size-full rounded-[inherit] transition-[color,box-shadow] outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:outline-1", children: children }), _jsx(ScrollBar, {}), _jsx(ScrollAreaPrimitive.Corner, {})] }));
}
function ScrollBar({ className, orientation = "vertical", ...props }) {
    return (_jsx(ScrollAreaPrimitive.ScrollAreaScrollbar, { "data-slot": "scroll-area-scrollbar", orientation: orientation, className: cn("flex touch-none p-px transition-colors select-none", orientation === "vertical" &&
            "h-full w-2.5 border-l border-l-transparent", orientation === "horizontal" &&
            "h-2.5 flex-col border-t border-t-transparent", className), ...props, children: _jsx(ScrollAreaPrimitive.ScrollAreaThumb, { "data-slot": "scroll-area-thumb", className: "relative flex-1 rounded-full bg-border" }) }));
}
export { ScrollArea, ScrollBar };
