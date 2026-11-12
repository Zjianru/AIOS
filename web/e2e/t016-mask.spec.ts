import { test, expect } from '@playwright/test';
import { apiPost, startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

const RULE = 'deepseek/deepseek-flash';

/// 设置页在模型行上屏蔽当前模型：立即生效、发不出去、失败块给「换模型」「去设置」；
/// 解除以后又能选、能发。规则、失败块文字、选择器标记都由服务事实给。
test('设置页屏蔽当前模型：发不出去，失败块给换模型和去设置；解除后能再用', async ({ page, request }) => {
  const fixture = await startKernelFixture();
  try {
    // 先有一次选择：确认框的「新会话：会用上一次选的」才有对象。
    await apiPost(request, fixture, {
      method: 'session-model-select', connection: 'deepseek', model: 'deepseek-flash',
    });
    await page.goto(urlOf(fixture));
    // 先聊一句：历史里有一轮，后面才能看「照常显示」。
    await page.getByPlaceholder('和 Kind 说…').fill('先说一句');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.getByText('（T007 e2e 假回放）').first()).toBeVisible({ timeout: 30_000 });

    // 设置 › 模型与连接：在 deepseek-flash 那一行点屏蔽，确认。
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
    await page.getByRole('button', { name: '编辑', exact: true }).first().click();
    const editor = page.locator('[data-settings="editor"]');
    await expect(editor).toBeVisible();
    await editor.getByText('自定义设置').click();
    await editor.getByRole('button', { name: '屏蔽 deepseek-flash' }).click();
    const confirm = page.locator('[data-settings="mask-confirm"]');
    await expect(confirm).toBeVisible();
    // 命中和谁在用都由服务算：这一行现在匹配 1 个，Kind 对话下一轮发不出去，新会话用上一次选的。
    await expect(confirm).toContainText(RULE);
    await expect(confirm).toContainText('现在匹配 1 个');
    await expect(confirm.getByText('Kind 对话')).toBeVisible();
    await expect(confirm).toContainText('下一轮发不出去');
    await expect(confirm.getByText('新会话')).toBeVisible();
    await expect(confirm).toContainText('会用上一次选的');
    await confirm.getByRole('button', { name: '屏蔽', exact: true }).click();

    // 立即生效：规则出现在「已屏蔽的模型」，行上标出已屏蔽。
    const section = page.locator('[data-settings="masked"]');
    await expect(section).toContainText(RULE);
    await expect(editor.locator('[data-settings="row-masked"]')).toBeVisible();

    // 回 Kind：当前模型被屏蔽时，选择器按钮上标「已屏蔽」。
    await page.getByRole('button', { name: 'Kind', exact: true }).click();
    const picker = page.locator('[data-kind="model-picker"]');
    await expect(picker).toContainText('已屏蔽', { timeout: 20_000 });
    // 历史那一轮照常显示（屏蔽不是删除）。
    await expect(page.getByText('（T007 e2e 假回放）').first()).toBeVisible();

    // 再发一句：在发出去之前被拒，失败块写模型名和命中的规则。
    await page.getByPlaceholder('和 Kind 说…').fill('还能发吗');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const failure = page.locator('[data-kind="failure"]');
    await expect(failure).toBeVisible({ timeout: 30_000 });
    await expect(failure).toContainText('deepseek-flash');
    await expect(failure).toContainText('规则「' + RULE + '」');
    await expect(failure.getByRole('button', { name: '换模型' })).toBeVisible();

    // 换模型：打开选择器，那一行灰掉、标「已屏蔽」、点不了。
    await failure.getByRole('button', { name: '换模型' }).click();
    const item = page.getByRole('menuitem', { name: /DeepSeek-V4\.1-Flash/ });
    await expect(item).toBeVisible({ timeout: 20_000 });
    await expect(item).toContainText('已屏蔽');
    await expect(item).toHaveAttribute('data-disabled', '');
    await page.keyboard.press('Escape');

    // 去设置 › 模型与连接：解除屏蔽，回到 Kind 又能发。
    await failure.getByRole('button', { name: '去设置 ›' }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
    const maskSection = page.locator('[data-settings="masked"]');
    await expect(maskSection).toContainText(RULE);
    await maskSection.getByRole('button', { name: '解除屏蔽' }).click();
    await expect(maskSection).not.toContainText(RULE);

    await page.getByRole('button', { name: 'Kind', exact: true }).click();
    const pickerAfter = page.locator('[data-kind="model-picker"]');
    await expect(pickerAfter).not.toContainText('已屏蔽', { timeout: 20_000 });
    await page.getByPlaceholder('和 Kind 说…').fill('解除了再发');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.getByText('（T007 e2e 假回放）')).toHaveCount(2, { timeout: 30_000 });
    // 只有被拦的那一轮有失败块；解除后的这一轮正常完成。
    await expect(page.locator('[data-kind="failure"]')).toHaveCount(1);
  } finally {
    await fixture.close();
  }
});

/// 屏蔽以后，以前用它的那一轮和用量行照常显示原来的模型名（D67：不藏起来）。
test('屏蔽以后历史和用量行照常显示原来的模型名', async ({ page, request }) => {
  const fixture = await startKernelFixture({ usageTurn: true });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('花点 token');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.getByText('（T007 e2e 假回放）').first()).toBeVisible({ timeout: 30_000 });

    const processLabel = 'DeepSeek-V4.1-Flash';
    const process = page.locator('[data-kind="process"]').first();
    await process.hover();
    const usage = page.locator('[data-kind="turn-usage"]').first();
    await expect(usage).toContainText(processLabel);

    // 屏蔽这一轮用的模型（deepseek-flash 是日志里记的模型 id）。
    // 不刷新页面：设置变化事件到达后，选择器按钮自己标出「已屏蔽」。
    await apiPost(request, fixture, { method: 'disabled-models-set', models: ['deepseek-flash'] });
    await expect(page.locator('[data-kind="model-picker"]')).toContainText('已屏蔽', { timeout: 20_000 });
    // 历史还在、模型名照常显示。
    await page.locator('[data-kind="process"]').first().hover();
    await expect(page.locator('[data-kind="turn-usage"]').first()).toContainText(processLabel);
    await page.reload();
    await expect(page.getByText('（T007 e2e 假回放）').first()).toBeVisible({ timeout: 30_000 });
    await expect(page.locator('[data-kind="model-picker"]')).toContainText('已屏蔽');
    await page.locator('[data-kind="process"]').first().hover();
    await expect(page.locator('[data-kind="turn-usage"]').first()).toContainText(processLabel);
  } finally {
    await fixture.close();
  }
});

/// 跑着的时候去设置页屏蔽当前模型：确认框写「这一轮还在跑，会立即停下」；
/// 确认后那一轮停下、出失败块，已经流出来的字还在。
test('跑着的时候屏蔽当前模型：确认框写着会立即停下，确认后那一轮停下出失败块', async ({ page, request }) => {
  const fixture = await startKernelFixture({ slowMs: 15_000 });
  try {
    await apiPost(request, fixture, {
      method: 'session-model-select', connection: 'deepseek', model: 'deepseek-flash',
    });
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('跑一轮长的');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    // 已经流出来的字出现时，这一轮还在跑。
    await expect(page.getByText('（T007 e2e 假回放）').first()).toBeVisible({ timeout: 30_000 });
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
    await page.getByRole('button', { name: '编辑', exact: true }).first().click();
    const editor = page.locator('[data-settings="editor"]');
    await expect(editor).toBeVisible();
    await editor.getByText('自定义设置').click();
    await editor.getByRole('button', { name: '屏蔽 deepseek-flash' }).click();
    const confirm = page.locator('[data-settings="mask-confirm"]');
    await expect(confirm).toBeVisible();
    await expect(confirm).toContainText('这一轮还在跑，会立即停下');
    await expect(confirm.getByText('Kind 对话')).toBeVisible();
    await expect(confirm.getByText('新会话')).toBeVisible();
    await confirm.getByRole('button', { name: '屏蔽', exact: true }).click();

    await page.getByRole('button', { name: 'Kind', exact: true }).click();
    const failure = page.locator('[data-kind="failure"]');
    await expect(failure).toBeVisible({ timeout: 20_000 });
    await expect(failure).toContainText('模型在设置里被屏蔽了');
    await expect(failure.getByRole('button', { name: '换模型' })).toBeVisible();
    await expect(page.getByText('（T007 e2e 假回放）').first()).toBeVisible();
  } finally {
    await fixture.close();
  }
});

/// 跟着厂商走时的小标签上也有屏蔽按钮，打开的是同一个确认框。
test('厂商模型小标签上的屏蔽按钮打开同一个确认框', async ({ page, request }) => {
  const fixture = await startKernelFixture();
  try {
    // models: [] = 跟着厂商走，编辑卡片里显示厂商模型小标签。
    await apiPost(request, fixture, {
      method: 'connection-update', id: 'deepseek', endpoint: fixture.fakeEndpoint,
      model: 'deepseek-flash', models: [],
    });
    await page.goto(urlOf(fixture));
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
    await page.getByRole('button', { name: '编辑', exact: true }).first().click();
    const editor = page.locator('[data-settings="editor"]');
    await expect(editor).toBeVisible();
    await editor.getByText('自定义设置').click();
    const tag = editor.locator('[data-settings="vendor-model"]').first();
    await expect(tag).toBeVisible();
    await expect(tag.getByRole('button', { name: /^屏蔽 / })).toBeVisible();
    await tag.getByRole('button', { name: /^屏蔽 / }).click();
    const confirm = page.locator('[data-settings="mask-confirm"]');
    await expect(confirm).toBeVisible();
    await expect(confirm).toContainText('现在匹配 1 个');
  } finally {
    await fixture.close();
  }
});

/// 写通配规则：输入框提示从已知模型里选；先过确认框（列出命中的模型）再保存；
/// 每条规则下面写着「现在匹配 n 个：…」；一条都不命中的写着「现在不匹配任何模型；
/// 出现了也会被拒」；解除后那一行消失。匹配数只读服务的 disabledHits，页面不自己算。
test('写通配规则：确认框列出命中，保存后每条写着现在匹配几个，解除后那一行消失', async ({ page, request }) => {
  const fixture = await startKernelFixture();
  try {
    // 跟着厂商走：可选的是内置目录里未下线的 deepseek 模型（两条标了 deprecated 的不算），
    // `*flash*` 只命中 deepseek-flash 一个。
    await apiPost(request, fixture, {
      method: 'connection-update', id: 'deepseek', endpoint: fixture.fakeEndpoint,
      model: 'deepseek-flash', models: [],
    });
    await page.goto(urlOf(fixture));
    await page.getByRole('button', { name: '设置', exact: true }).click();
    const section = page.locator('[data-settings="masked"]');
    await expect(section).toBeVisible({ timeout: 20_000 });

    // 写规则的入口：提示从已知模型里选（datalist），也能随便写通配。
    const input = section.locator('[data-settings="mask-input"]');
    await expect(input).toBeVisible({ timeout: 3_000 });
    await expect(section.locator('option[value="deepseek/deepseek-flash"]')).toHaveCount(1);

    // 写 `*flash*`：先过确认框，确认框列出命中的模型。
    await input.fill('*flash*');
    await section.getByRole('button', { name: '屏蔽…' }).click();
    const confirm = page.locator('[data-settings="mask-confirm"]');
    await expect(confirm).toBeVisible();
    await expect(confirm).toContainText('现在匹配 1 个');
    await expect(confirm).toContainText('deepseek/deepseek-flash');
    await confirm.getByRole('button', { name: '屏蔽', exact: true }).click();

    // 保存后那一条下面写着「现在匹配 1 个」和命中的「连接/模型」。
    const flashRow = section.locator('[data-settings="mask-row"]').filter({ hasText: '*flash*' });
    await expect(flashRow).toContainText('现在匹配 1 个');
    await expect(flashRow).toContainText('deepseek/deepseek-flash');

    // 一条都不匹配的规则照样能存；写着出现了也会被拒。
    await input.fill('*gpt-9*');
    await section.getByRole('button', { name: '屏蔽…' }).click();
    await expect(confirm).toContainText('现在不匹配任何模型');
    await confirm.getByRole('button', { name: '屏蔽', exact: true }).click();
    const missRow = section.locator('[data-settings="mask-row"]').filter({ hasText: '*gpt-9*' });
    await expect(missRow).toContainText('现在不匹配任何模型；出现了也会被拒');

    // 解除：那一行消失，另一条还在；都解除后回到空状态。
    await flashRow.getByRole('button', { name: '解除屏蔽' }).click();
    await expect(flashRow).toHaveCount(0);
    await expect(missRow).toBeVisible();
    await missRow.getByRole('button', { name: '解除屏蔽' }).click();
    await expect(missRow).toHaveCount(0);
    await expect(section).toContainText('还没有屏蔽任何模型');
  } finally {
    await fixture.close();
  }
});
