/**
 * 「现在」的空态（#empty:now；brief §四：空态照 v13 原型）。
 * 第一次打开、什么都还没有时不放占位卡片，只放：问候 + 一句说明 + 三种关心的分组 + 三条起手示例 + 两个入口。
 */

import { go } from '@/app/navigate';

const GROUPS = [
  { tone: 'var(--wait)', name: '等你决定', desc: '要你点头的事' },
  { tone: 'var(--fail)', name: '出了问题', desc: 'Kind 处理不了的才来找你' },
  { tone: 'var(--run)', name: '你交代的事', desc: '正在推进的' },
];

const PROMPTS = ['帮我把这周的会议纪要整理成一页', '每天早上 9 点给我一份 AI 简报', '比较三款记账 App，选一款给爸妈用'];

export function EmptyNow() {
  return (
    <div className="h-full overflow-y-auto" data-scene-page="empty-now">
      <div className="mx-auto w-full max-w-[860px] px-6 py-10">
        <h1 className="text-6 font-semibold leading-8 text-ink">下午好</h1>
        <p className="mt-1 text-3 text-ink-2">还没有事情。</p>

        <div className="mt-14 flex flex-col items-center text-center">
          <span
            className="block size-12 rounded-full"
            style={{ background: 'radial-gradient(circle at 35% 30%, var(--brand), var(--brand-soft))' }}
          />
          <h2 className="mt-4 text-4 font-medium text-ink">这里会放你要关心的三件事</h2>
          <div className="mt-5 grid w-full grid-cols-1 gap-3 sm:grid-cols-3">
            {GROUPS.map((group) => (
              <div key={group.name} className="rounded-card border border-dashed border-line-2 px-4 py-4">
                <span className="mx-auto mb-1.5 block size-[9px] rounded-full" style={{ backgroundColor: group.tone }} />
                <div className="text-3 font-medium text-ink">{group.name}</div>
                <div className="mt-0.5 text-1 text-ink-3">{group.desc}</div>
              </div>
            ))}
          </div>
          <p className="mt-4 text-2 text-ink-3">现在都还没有。交给 Kind 一件事，它就会出现在这里。</p>
          <div className="mt-5 flex flex-wrap justify-center gap-2">
            {PROMPTS.map((prompt) => (
              <button
                key={prompt}
                type="button"
                onClick={() => go('kind')}
                className="rounded-full border border-line px-3 py-1 text-2 text-ink-2 transition-colors hover:bg-hover"
              >
                「{prompt}」
              </button>
            ))}
          </div>
          <div className="mt-6 flex gap-2">
            <button
              type="button"
              onClick={() => go('kind')}
              className="rounded-btn bg-ink px-3 py-1.5 text-2 text-ink transition-opacity hover:opacity-90"
              style={{ color: 'var(--content)' }}
            >
              和 Kind 说
            </button>
            <button
              type="button"
              onClick={() => go('newproj')}
              className="rounded-btn border border-line-2 px-3 py-1.5 text-2 text-ink-2 transition-colors hover:bg-hover"
            >
              新建项目
            </button>
          </div>
        </div>

        <p className="mt-16 border-t border-line pt-2 text-1 leading-4 text-ink-3">
          现在（空态）：第一次打开时不放假数据；三个分组就是 AIOS 里你需要关心的三件事。
        </p>
      </div>
    </div>
  );
}
