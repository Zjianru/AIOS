/**
 * 当前路由的全局副本。
 *
 * 为什么要它：dockview 的面板内容是它自己渲染、自己管的，父组件重新渲染不会让面板内容重渲染。
 * 如果路由只靠 props 往下传，同一个场景内换路由（#ability → #flow:x、#settings → #settings:model）
 * 面板内容就会是旧的。所以路由放进 store，面板组件订阅它。
 */
import { create } from 'zustand';
import { parseRoute } from './route';
function readRoute() {
    if (typeof window === 'undefined')
        return parseRoute('');
    return parseRoute(window.location.hash || '#now');
}
export const useRouteStore = create()(() => ({ route: readRoute() }));
export function syncRoute(route) {
    const current = useRouteStore.getState().route;
    // raw 不含查询串（#work 和 #work?wv=cal 的 raw 都是 work），所以 query 也要比。
    if (current.raw === route.raw && current.query.toString() === route.query.toString())
        return;
    useRouteStore.setState({ route });
}
