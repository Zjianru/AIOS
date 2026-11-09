import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

/** T023：第一步先流一段「工具前说明」再调 read hello.txt，第二步回最终答案。 */
const PRETEXT = '我先确认一下你系统里对卡片的定义，再回答你。';
const ANSWER = '查完了。真正能发出的卡片有五类：画布卡、提问卡、确认卡、待办卡，还有回答卡。';

type Snap = { text: string | null };

test('工具前说明不进实时回答；结束后等于最终答案；刷新一致、无重复', async ({ page }) => {
  const fixture = await startKernelFixture({ pretextTurn: true });
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');

    // 实时观察：Answer 每次重绘都是一次 DOM 变化，记下回答区最后一次渲染的正文。
    await page.evaluate(() => {
      const state: { snapshots: Snap[]; last: string } = { snapshots: [], last: '__start__' };
      (window as unknown as { __t023: typeof state }).__t023 = state;
      const record = () => {
        const answers = document.querySelectorAll('[data-kind="assistant-answer"]');
        const lastAnswer = answers[answers.length - 1];
        const text = lastAnswer ? lastAnswer.textContent : null;
        const mark = String(text);
        if (mark === state.last) return;
        state.last = mark;
        state.snapshots.push({ text });
      };
      new MutationObserver(record).observe(document.body, {
        subtree: true,
        childList: true,
        characterData: true,
      });
      record();
    });

    await page.getByPlaceholder('和 Kind 说…').fill('你现在都能做什么卡片');
    await page.locator('[data-kind="send"]').click();

    const answer = page.locator('[data-kind="assistant-answer"]').last();
    await expect(answer).toHaveText(ANSWER, { timeout: 30_000 });
    // 等 turn/end 把「停止」换回「发送」。
    await expect(page.locator('[data-kind="stop"]')).toHaveCount(0, { timeout: 30_000 });

    const snapshots = await page.evaluate(
      () => (window as unknown as { __t023: { snapshots: Snap[] } }).__t023.snapshots,
    );
    const texts = snapshots.map((snap) => snap.text ?? '');

    // 前提：工具前说明确实在实时阶段显示过（夹具跑对了，不是空测）。
    const saidAt = texts.findIndex((text) => text.length > 0 && PRETEXT.startsWith(text));
    expect(saidAt, '实时阶段应先看到工具前说明').toBeGreaterThanOrEqual(0);
    // 实时文字只能是「工具前说明」或「最终答案」的前缀：拼在一起或重复追加都会破坏它。
    for (const text of texts) {
      expect(
        text === '' || PRETEXT.startsWith(text) || ANSWER.startsWith(text),
        '实时回答出现了拼接或重复：' + JSON.stringify(text),
      ).toBe(true);
    }
    // 工具调用开始时这一段被清掉：说过之后，回答区回到空。
    expect(
      texts.slice(saidAt + 1).some((text) => text === ''),
      '工具调用开始后实时回答应清空当前段',
    ).toBe(true);
    // 实时阶段确实逐段流过最终答案（不是只在结束后一次性出现）。
    expect(
      texts.some((text) => text.length > 0 && text.length < ANSWER.length && ANSWER.startsWith(text)),
      '实时阶段应看到最终答案的流式前缀',
    ).toBe(true);

    // turn/end 之后等于日志里的最终 assistant text，且不含工具前说明。
    await expect(answer).toHaveText(ANSWER);
    expect(await answer.textContent()).not.toContain(PRETEXT);

    // 刷新一致：从日志重放出来的回答也是最终答案。
    await page.reload();
    await expect(page.locator('[data-kind="assistant-answer"]').last()).toHaveText(ANSWER, {
      timeout: 30_000,
    });
    expect(await page.locator('[data-kind="assistant-answer"]').last().textContent()).not.toContain(
      PRETEXT,
    );
  } finally {
    await fixture.close();
  }
});
