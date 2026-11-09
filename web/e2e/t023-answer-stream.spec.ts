import { test, expect, type Page } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

/** T023：第 1 步先流一段「工具前说明」再调 read hello.txt，第 2 步回最终答案。 */
const PRETEXT = '我先确认一下你系统里对卡片的定义，再回答你。';
const ANSWER = '查完了。真正能发出的卡片有五类：画布卡、提问卡、确认卡、待办卡，还有回答卡。';
/** 慢夹具里最终答案的分段（拼起来等于 ANSWER；重复追加时会出现同一段两次）。 */
const FINAL_CHUNKS = [
  '查完了。',
  '真正能发出的',
  '卡片有五类：',
  '画布卡、',
  '提问卡、',
  '确认卡、',
  '待办卡，',
  '还有回答卡。',
];

type Snap = { text: string | null };

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

/** 实时观察：Answer 每次重绘都是一次 DOM 变化，记下回答区最后一次渲染的正文。 */
async function installAnswerObserver(page: Page): Promise<void> {
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
}

async function snapshotsOf(page: Page): Promise<Snap[]> {
  return page.evaluate(
    () => (window as unknown as { __t023: { snapshots: Snap[] } }).__t023.snapshots,
  );
}

/** 刷新会清空页面状态；每次导航前装观察器，刷新后从第一帧记录回答区。 */
async function installAnswerObserverOnLoad(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const state: { snapshots: Snap[]; last: string } = { snapshots: [], last: '__start__' };
    (window as unknown as { __t023Reload: typeof state }).__t023Reload = state;
    const record = () => {
      const answers = document.querySelectorAll('[data-kind="assistant-answer"]');
      const lastAnswer = answers[answers.length - 1];
      const text = lastAnswer ? lastAnswer.textContent : null;
      const mark = String(text);
      if (mark === state.last) return;
      state.last = mark;
      state.snapshots.push({ text });
    };
    new MutationObserver(record).observe(document, {
      subtree: true,
      childList: true,
      characterData: true,
    });
    record();
  });
}

async function reloadSnapshotsOf(page: Page): Promise<Snap[]> {
  return page.evaluate(
    () => (window as unknown as { __t023Reload: { snapshots: Snap[] } }).__t023Reload.snapshots,
  );
}

test('工具前说明不进实时回答；结束后等于最终答案；刷新一致、无重复', async ({ page }) => {
  const fixture = await startKernelFixture({ pretextTurn: true });
  try {
    await page.goto(urlOf(fixture));
    await installAnswerObserver(page);
    await page.getByPlaceholder('和 Kind 说…').fill('你现在都能做什么卡片');
    await page.locator('[data-kind="send"]').click();

    const answer = page.locator('[data-kind="assistant-answer"]').last();
    const stop = page.locator('[data-kind="stop"]');
    // turn/end 之前回答区已经流出了最终答案；此时停止按钮还在。
    await expect(answer).toContainText(FINAL_CHUNKS[0], { timeout: 30_000 });
    await expect(stop).toBeVisible();
    const liveText = (await answer.textContent()) ?? '';
    expect(liveText).not.toContain(PRETEXT);
    expect(ANSWER.startsWith(liveText), '实时回答应等于最终答案的前缀：' + JSON.stringify(liveText)).toBe(
      true,
    );

    await expect(answer).toHaveText(ANSWER, { timeout: 30_000 });
    await expect(stop).toHaveCount(0, { timeout: 30_000 });

    const texts = (await snapshotsOf(page)).map((snap) => snap.text ?? '');
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

test('工具调用一开始就停止：turn/end(aborted) 后回答区不显示工具前说明', async ({ page }) => {
  const fixture = await startKernelFixture({ pretextSlowTurn: true });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('你现在都能做什么卡片');
    await page.locator('[data-kind="send"]').click();

    const answer = page.locator('[data-kind="assistant-answer"]');
    // 先看到工具前说明，等工具调用一开始把它清掉，就在这一刻按停止。
    await expect(answer.last()).toContainText(PRETEXT.slice(0, 6), { timeout: 30_000 });
    await expect
      .poll(async () => (await answer.allTextContents()).join(''), { timeout: 20_000 })
      .toBe('');
    await page.locator('[data-kind="stop"]').click();

    // turn/end(aborted)：停止行出现，回答区可以空，不能回退成工具前说明。
    await expect(page.locator('[data-kind="stopped"]')).toBeVisible({ timeout: 20_000 });
    const afterStop = (await answer.allTextContents()).join('');
    expect(afterStop).not.toContain(PRETEXT);
  } finally {
    await fixture.close();
  }
});

test('中途刷新：工具前说明不回到回答区，最终答案照常到达', async ({ page }) => {
  const fixture = await startKernelFixture({ pretextSlowTurn: true });
  try {
    await installAnswerObserverOnLoad(page);
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('你现在都能做什么卡片');
    await page.locator('[data-kind="send"]').click();

    const answer = page.locator('[data-kind="assistant-answer"]');
    // 工具调用一开始回答区清空（此刻日志里只有工具前说明）；就在这一刻刷新。
    await expect(answer.last()).toContainText(PRETEXT.slice(0, 6), { timeout: 30_000 });
    await expect
      .poll(async () => (await answer.allTextContents()).join(''), { timeout: 20_000 })
      .toBe('');
    await page.reload();

    // 刷新后这一轮还在跑：回答区从最终答案的流式段续上，最终等于完整答案。
    await expect(page.locator('[data-kind="assistant-answer"]').last()).toHaveText(ANSWER, {
      timeout: 30_000,
    });
    const texts = (await reloadSnapshotsOf(page)).map((snap) => snap.text ?? '');
    // 日志里只有工具前说明时，回答区不能把它显示成回答。
    expect(texts.some((text) => text.includes(PRETEXT))).toBe(false);
    // 前提：刷新落在最终答案流完之前，否则只测到结束后刷新。
    expect(
      texts.some((text) => text.length > 0 && text.length < ANSWER.length && ANSWER.includes(text)),
      '刷新后应看到最终答案的流式段：' + JSON.stringify(texts),
    ).toBe(true);
  } finally {
    await fixture.close();
  }
});

test('设置页往返后实时文字只追加一次', async ({ page }) => {
  const fixture = await startKernelFixture({ pretextSlowTurn: true });
  try {
    await page.goto(urlOf(fixture));
    await installAnswerObserver(page);
    await page.getByPlaceholder('和 Kind 说…').fill('你现在都能做什么卡片');
    await page.locator('[data-kind="send"]').click();

    // 连接已经在收帧（第一步的工具前说明流出来了）。
    await expect(page.locator('[data-kind="assistant-answer"]').last()).toContainText('我先确认一下', {
      timeout: 20_000,
    });
    // 设置页往返：KindScene 卸载再挂载。旧 socket 不关就会叠成两条连接。
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
    const mark = (await snapshotsOf(page)).length;
    await page.waitForTimeout(400);
    await page.getByRole('button', { name: 'Kind', exact: true }).click();

    const answer = page.locator('[data-kind="assistant-answer"]').last();
    await expect(answer).toHaveText(ANSWER, { timeout: 30_000 });
    await expect(page.locator('[data-kind="stop"]')).toHaveCount(0, { timeout: 30_000 });

    const textAfterReturn = (await snapshotsOf(page))
      .slice(mark)
      .map((snap) => snap.text ?? '');
    // 往返回来时这一轮还在流（实时文字是最终答案的一段，不是结束后一次性出现）。
    expect(
      textAfterReturn.some(
        (text) => text.length > 0 && text.length < ANSWER.length && ANSWER.startsWith(text),
      ),
      '设置页回来应看到还在流式的回答',
    ).toBe(true);
    // 每条 text-delta 只追加一次：同一段最终答案不得在一次渲染里出现两遍。
    for (const text of textAfterReturn) {
      for (const chunk of FINAL_CHUNKS) {
        expect(
          text.split(chunk).length - 1,
          '实时文字重复追加：' + JSON.stringify(text),
        ).toBeLessThanOrEqual(1);
      }
    }
    await expect(answer).toHaveText(ANSWER);
  } finally {
    await fixture.close();
  }
});
