// T092.1：任务图布局几何（最长路径分列/重心/竖直居中/lane/端口）。
import assert from 'node:assert/strict';
import { COL_GAP, NODE_H, NODE_W, PAD, layoutGraph } from '../src/pages/work/graphModel.ts';

const node = (id, depth) => ({ task: { id }, tile: { column: 'queue', word: '等待中', next: '等 Kind' }, depth, row: 0, kind: 'task' });
const edge = (from, to) => ({ id: from + '>' + to, from, to, kind: 'needs', state: 'waiting', program: false, label: '', live: null, payload: null });
const graph = (nodes, edges) => ({ id: 'g', title: 't', nodes, edges, closed: false, counts: { queue: nodes.length, doing: 0, wait: 0, done: 0 } });

function overlaps(a, b) {
  return a.x < b.x + NODE_W && b.x < a.x + NODE_W && a.y < b.y + NODE_H && b.y < a.y + NODE_H;
}

/** 把 M/C/L 路径采样成折线点，用来查线穿不穿节点。 */
function pathPoints(d) {
  const points = [];
  let cursor = null;
  for (const match of d.matchAll(/([MLC])([^MLC]*)/g)) {
    const values = match[2].trim().split(/[ ,]+/).filter(Boolean).map(Number);
    if (match[1] === 'M') {
      cursor = { x: values[0], y: values[1] };
      points.push(cursor);
    } else if (match[1] === 'L') {
      cursor = { x: values[0], y: values[1] };
      points.push(cursor);
    } else {
      const [x1, y1, x2, y2, x3, y3] = values;
      for (let step = 1; step <= 20; step += 1) {
        const t = step / 20;
        const u = 1 - t;
        points.push({
          x: u * u * u * cursor.x + 3 * u * u * t * x1 + 3 * u * t * t * x2 + t * t * t * x3,
          y: u * u * u * cursor.y + 3 * u * u * t * y1 + 3 * u * t * t * y2 + t * t * t * y3,
        });
      }
      cursor = { x: x3, y: y3 };
    }
  }
  return points;
}

// 1) 24 个节点（6 列 × 4）两两不重叠，序号 1..24 唯一。
{
  const nodes = [];
  const edges = [];
  for (let c = 0; c < 6; c += 1) for (let r = 0; r < 4; r += 1) nodes.push(node('n' + c + '-' + r, c));
  for (let c = 0; c < 5; c += 1) for (let r = 0; r < 4; r += 1) edges.push(edge('n' + c + '-' + r, 'n' + (c + 1) + '-' + r));
  const layout = layoutGraph(graph(nodes, edges));
  const list = Object.entries(layout.nodes).map(([id, at]) => ({ id, ...at }));
  assert.equal(list.length, 24);
  assert.deepEqual(new Set(list.map((item) => item.order)), new Set(Array.from({ length: 24 }, (_, i) => i + 1)));
  for (let i = 0; i < list.length; i += 1) for (let j = i + 1; j < list.length; j += 1) assert.ok(!overlaps(list[i], list[j]), list[i].id + ' 与 ' + list[j].id + ' 重叠');
  for (const item of list) assert.ok(item.x >= 0 && item.x + NODE_W <= layout.width && item.y >= 0 && item.y + NODE_H <= layout.height);
}

// 2) 跨列边不穿过中间节点：a→c 走 b 所在列的 lane，采样点不能落进 b。
{
  const nodes = [node('a', 0), node('b', 1), node('c', 2)];
  const edges = [edge('a', 'c')];
  const layout = layoutGraph(graph(nodes, edges));
  const route = layout.edges['a>c'];
  assert.ok(route, '跨列边要有路由');
  const points = pathPoints(route.d);
  const b = layout.nodes.b;
  for (const point of points) {
    const inside = point.x > b.x + 2 && point.x < b.x + NODE_W - 2 && point.y > b.y + 2 && point.y < b.y + NODE_H - 2;
    assert.ok(!inside, '边穿过了非端点节点 b：' + JSON.stringify(point));
  }
}

// 3) 同列重心顺序与竖直居中：单链三列，各节点竖向中心一致；同列节点 x 相同。
{
  const nodes = [node('a', 0), node('b', 1), node('c', 2)];
  const layout = layoutGraph(graph(nodes, [edge('a', 'b'), edge('b', 'c')]));
  assert.equal(layout.nodes.a.x, layout.nodes.b.x - (NODE_W + COL_GAP));
  assert.equal(layout.nodes.b.x, layout.nodes.c.x - (NODE_W + COL_GAP));
  const centre = (at) => at.y + NODE_H / 2;
  assert.ok(Math.abs(centre(layout.nodes.a) - centre(layout.nodes.b)) < 0.001);
  assert.ok(Math.abs(centre(layout.nodes.b) - centre(layout.nodes.c)) < 0.001);
}

// 4) 同节点多端口不重叠：a 三条出线到三个下游，起点 y 互不相同且在卡高内。
{
  const nodes = [node('a', 0), node('d', 1), node('e', 1), node('f', 1)];
  const layout = layoutGraph(graph(nodes, [edge('a', 'd'), edge('a', 'e'), edge('a', 'f')]));
  const starts = ['a>d', 'a>e', 'a>f'].map((id) => {
    const match = layout.edges[id].d.match(/^M([\d.]+),([\d.]+)/);
    return { x: Number(match[1]), y: Number(match[2]) };
  });
  assert.equal(new Set(starts.map((point) => point.y)).size, 3, '三个端口不能同一个 y');
  assert.ok(starts.every((point) => point.x === starts[0].x));
  for (const point of starts) assert.ok(point.y >= layout.nodes.a.y + 12 && point.y <= layout.nodes.a.y + NODE_H - 12);
}

// 5) 单节点无边。
{
  const layout = layoutGraph(graph([node('solo', 0)], []));
  assert.equal(Object.keys(layout.nodes).length, 1);
  assert.equal(Object.keys(layout.edges).length, 0);
  assert.equal(layout.width, PAD * 2 + NODE_W);
}

// 6) 自环/环不挂死，返回有限尺寸。
{
  const layout = layoutGraph(graph([node('a', 0), node('b', 1)], [edge('a', 'a'), edge('a', 'b'), edge('b', 'a')]));
  assert.equal(Object.keys(layout.nodes).length, 2);
  assert.ok(Number.isFinite(layout.width) && layout.width > 0);
  for (const route of Object.values(layout.edges)) assert.match(route.d, /^M[\d.]+,[\d.]+/);
}


// 7) 重心排序：同一列两个节点，按上游重心排；一个节点有两个上游时取平均。
{
  const nodes = [node('p1', 0), node('c1', 1), node('p2', 0), node('c2', 1)];
  const edges = [edge('p1', 'c1'), edge('p2', 'c1'), edge('p1', 'c2')];
  const layout = layoutGraph(graph(nodes, edges));
  assert.ok(layout.nodes.c2.y < layout.nodes.c1.y, 'c2 只连 p1（重心 0），应排在连 p1+p2（重心 0.5）的 c1 上面');
  // 同列 x 对齐、两列竖直中心一致（重心排序后仍居中）。
  assert.equal(layout.nodes.p1.x, layout.nodes.p2.x);
  assert.equal(layout.nodes.c1.x, layout.nodes.c2.x);
  const centre = (list) => (Math.min(...list.map((item) => item.y)) + Math.max(...list.map((item) => item.y + NODE_H))) / 2;
  assert.ok(Math.abs(centre([layout.nodes.p1, layout.nodes.p2]) - centre([layout.nodes.c1, layout.nodes.c2])) < 0.001);
}

console.log('graph-layout ok');
