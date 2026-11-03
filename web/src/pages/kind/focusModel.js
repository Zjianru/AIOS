/**
 * Kind 右栏「聚焦」的数据归约（brief §四.1「第 2 期追加」第 1 条）。
 *
 * 规则（用户 2026-10-28 确认）：
 * - 只放「要你看的」和「正在动的」：等你 → 出了问题 → 在跑，每组最多 3 条，多了写「还有 N 件」；
 * - 正常推进的折成一行「其余 N 站正常推进 · 全部 ›」；
 * - 等你 / 出了问题每条带一根小地图条（这条线上第几站，所在站用状态色圈出），在跑不画；
 * - 一条要看的都没有时只显示一句「都在正常推进」，高度随内容收缩。
 *
 * 站 = 项目根任务下的下一层任务（和 v13 地铁线一致）；更深的任务挂在所属站上，
 * 所以一条叶子任务要沿 parent 往上找到它的「站」。这里只做纯函数，数据从 api 取。
 */
import { stateOf } from '@/data/selectors';
/** 摊平成叶子任务：聚焦里的一条 = 一件真的会等你 / 会出问题 / 在跑的事。 */
function leavesOf(task, tasks) {
    if (!task.children?.length)
        return [task];
    return task.children.flatMap((id) => leavesOf(tasks[id], tasks));
}
/** 沿 parent 往上找，直到父任务是项目根任务；返回「站」本身。 */
function stationOf(leaf, roots, tasks) {
    let current = leaf;
    while (current.parent && !roots.has(current.parent))
        current = tasks[current.parent];
    return current;
}
/** 「等了 N 分」：样例里只有把时长写在 note 里的（如「41 分钟前起」）。 */
function waitLabel(task) {
    const minutes = task.note?.match(/(\d+)\s*分钟前/);
    if (minutes)
        return '等了 ' + minutes[1] + ' 分';
    const hours = task.note?.match(/(\d+)\s*小时前/);
    if (hours)
        return '等了 ' + hours[1] + ' 小时';
    return '等你决定';
}
const RIGHT_OF = {
    wait: waitLabel,
    fail: () => '出了问题',
    run: (task) => task.note ?? '在做',
};
const ORDER = ['wait', 'fail', 'run'];
const GROUP_LIMIT = 3;
export function buildFocusView(roots, tasks, projects) {
    const rootIds = new Set(roots.map((root) => root.id));
    const raw = { wait: [], fail: [], run: [] };
    let totalStations = 0;
    for (const root of roots) {
        const project = projects[root.project];
        if (!project)
            continue;
        const stations = (root.children ?? []).map((id) => tasks[id]);
        totalStations += stations.length;
        stations.forEach((station) => {
            for (const leaf of leavesOf(station, tasks)) {
                const state = stateOf(leaf, tasks);
                if (state !== 'wait' && state !== 'fail' && state !== 'run')
                    continue;
                const anchor = stationOf(leaf, rootIds, tasks);
                const anchorIndex = stations.findIndex((candidate) => candidate.id === anchor.id);
                const projectView = {
                    id: project.id,
                    name: project.name,
                    code: project.code,
                    color: project.color,
                };
                raw[state].push({
                    id: leaf.id,
                    title: leaf.title,
                    state,
                    project: projectView,
                    meta: project.name +
                        ' · 第 ' +
                        (anchorIndex + 1) +
                        ' / ' +
                        stations.length +
                        ' 站' +
                        (leaf.id === anchor.id ? '' : ' · ' + anchor.title),
                    right: RIGHT_OF[state](leaf),
                    strip: state === 'run'
                        ? null
                        : { total: stations.length, index: anchorIndex + 1, color: project.color, state },
                    href: 'work?line=' + project.id + '&st=' + leaf.id,
                });
            }
        });
    }
    const groups = [];
    for (const state of ORDER) {
        const items = raw[state];
        if (!items.length)
            continue;
        groups.push({ state, items: items.slice(0, GROUP_LIMIT), more: Math.max(0, items.length - GROUP_LIMIT) });
    }
    // 「其余 N 站正常推进」按【站】算：要看的 / 在跑的东西落在的站，不算正常推进。
    const touched = new Set();
    for (const root of roots) {
        for (const stationId of root.children ?? []) {
            const station = tasks[stationId];
            for (const leaf of leavesOf(station, tasks)) {
                const state = stateOf(leaf, tasks);
                if (state === 'wait' || state === 'fail' || state === 'run')
                    touched.add(root.project + ':' + station.id);
            }
        }
    }
    const toSee = raw.wait.length + raw.fail.length;
    const running = raw.run.length;
    return {
        groups,
        toSee,
        running,
        rest: Math.max(0, totalStations - touched.size),
        allNormal: groups.length === 0,
    };
}
