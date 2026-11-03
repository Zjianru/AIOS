/**
 * 路由解析。v13 用 hash 路由，名字照旧（#now、#kind、#task:B2、#settings:model）。
 * react-router 只负责 hash ↔ location；具体解析在这里——冒号在 path-to-regexp 里有含义，不能直接当路径用。
 */
export function parseRoute(hash) {
    const clean = hash.replace(/^#/, '').replace(/^\//, '');
    const qIndex = clean.indexOf('?');
    const path = qIndex >= 0 ? clean.slice(0, qIndex) : clean;
    const query = new URLSearchParams(qIndex >= 0 ? clean.slice(qIndex + 1) : '');
    const raw = path || 'now';
    const colon = raw.indexOf(':');
    const kind = colon >= 0 ? raw.slice(0, colon) : raw;
    const rawId = colon >= 0 ? raw.slice(colon + 1) : '';
    // hash 里的中文文件名是百分号编码的（#read:%E7%BB%93…），解析时还原成真文件名。
    let id = rawId;
    try {
        id = decodeURIComponent(rawId);
    }
    catch {
        id = rawId;
    }
    return { raw, kind, id, query };
}
/** 场景 key：同一场景里的多个路由共用一个 dock 根。 */
const SCENE_OF = {
    now: 'now',
    kind: 'kind',
    work: 'work',
    project: 'project',
    task: 'task',
    trace: 'trace',
    team: 'team',
    role: 'team',
    ability: 'ability',
    flow: 'ability',
    lib: 'lib',
    read: 'read',
    memory: 'memory',
    health: 'health',
    notify: 'notify',
    usage: 'usage',
    settings: 'settings',
    monitor: 'settings',
    projset: 'project',
    newproj: 'newproj',
    welcome: 'welcome',
    empty: 'empty',
};
export function sceneOf(route) {
    return SCENE_OF[route.kind] ?? route.kind;
}
/** 侧栏高亮项（v13 的 nav 标识）。 */
export function navOf(route) {
    switch (route.kind) {
        case 'project':
        case 'projset':
            return 'project:' + route.id;
        case 'task':
        case 'trace':
            return 'project';
        case 'role':
            return 'team';
        case 'flow':
            return 'ability';
        case 'read':
            return 'lib';
        case 'settings':
        case 'monitor':
            return 'settings';
        case 'health':
            return 'now';
        case 'usage':
        case 'notify':
        case 'welcome':
        case 'newproj':
        case 'empty':
            return null;
        default:
            return route.kind;
    }
}
/** 面包屑文字（第 1 期的占位页也要面包屑）。 */
/** 设置的分组（照 v13）；设置页左栏和顶栏面包屑共用。 */
export const SETTINGS_GROUPS = [
    { id: 'look', name: '外观' },
    { id: 'model', name: '模型与连接' },
    { id: 'kind', name: 'Kind' },
    { id: 'exec', name: '执行' },
    { id: 'notify', name: '通知' },
    { id: 'privacy', name: '隐私与更新' },
    { id: 'monitor', name: '监控' },
    { id: 'about', name: '关于' },
];
/** 面包屑第二段：设置分组显示名字，其余照原样显示 id。 */
export function crumbIdOf(route) {
    if (route.kind === 'settings')
        return SETTINGS_GROUPS.find((group) => group.id === route.id)?.name ?? route.id;
    return route.id;
}
export function crumbOf(route) {
    const titles = {
        now: '现在',
        kind: 'Kind',
        work: '工作',
        team: '团队',
        ability: '能力',
        lib: '资料',
        memory: '记忆',
        health: 'AIOS 健康状况',
        notify: '什么时候会通知你',
        usage: '用量与费用',
        settings: '设置',
        welcome: '欢迎引导',
        newproj: '新建项目',
        empty: '空状态预览',
        flow: '流程',
        project: '项目',
        task: '任务',
        trace: '原始记录',
        role: '角色',
        read: '阅读',
        monitor: '监控',
        projset: '项目设置',
    };
    return titles[route.kind] ?? route.kind;
}
