import { expect, test } from '@playwright/test';
import { readFile, readdir } from 'node:fs/promises';
import path from 'node:path';
import { apiPost, startD78KernelFixture } from './service-fixture';
test.skip(!process.env.MINIMAX_API_KEY, 'key');
test.setTimeout(300_000);
const MAIN='MiniMax-M3.1-Flash'; const VISION='MiniMax-M3';
const RED='iVBORw0KGgoAAAANSUhEUgAAACAAAAAgCAIAAAD8GO2jAAAAK0lEQVR4nO3NQQEAAATAQGTQP5kwSvC7BdjldMdn9XoHAAAAAAAAAAAAhy3gIwFE6inHLwAAAABJRU5ErkJggg==';
test('D78 找位置：vision_ground 带两张附件', async ({page, request})=>{
 const fixture=await startD78KernelFixture();
 try{
  const s=await (await apiPost(request,fixture,{method:'model-settings'})).json() as any;
  const c=s.settings.connections.find((x:any)=>x.id==='minimax-cn');
  await apiPost(request,fixture,{method:'connection-update',id:c.id,endpoint:c.endpoint,preset:c.preset,model:MAIN,models:[{id:MAIN,input:['text']},{id:VISION,input:['text','image']}]});
  await apiPost(request,fixture,{method:'recheck'});
  await apiPost(request,fixture,{method:'settings-update',visionModels:[{connection:c.id,model:VISION}]});
  await page.goto(fixture.baseUrl+'/?token='+encodeURIComponent(fixture.token)+'#kind');
  const input=page.locator('[data-kind="composer-input"]'); await expect(input).toBeVisible({timeout:60000});
  const t=await page.evaluateHandle((b64)=>{const bin=atob(b64);const d=new Uint8Array(bin.length);for(let i=0;i<bin.length;i++)d[i]=bin.charCodeAt(i);const dt=new DataTransfer();dt.items.add(new File([d],'red.png',{type:'image/png'}));return dt;},RED);
  await page.locator('[data-kind="composer"]').dispatchEvent('drop',{dataTransfer:t});
  await expect(page.locator('[data-kind="attachment-image"]')).toBeVisible({timeout:20000});
  await input.fill('你必须调用 vision_ground 工具找红色方块的位置（不要直接回答），然后把 bbox 告诉我。');
  await page.locator('[data-kind="send"]').click();
  await expect(page.getByRole('button',{name:/已完成|停下了|已派出/}).last()).toBeVisible({timeout:180000});
  const dir=path.join(fixture.dataDir,'sessions'); const names=(await readdir(dir)).filter(n=>n.endsWith('.jsonl'));
  let called=false; let images=0; let bbox=false;
  for(const n of names){ const txt=await readFile(path.join(dir,n),'utf8'); for(const line of txt.split('\n').filter(Boolean)){ const ev=JSON.parse(line); if(ev.type==='tool/call'&&ev.data?.name==='vision_ground') called=true; if(ev.type==='tool/result'&&ev.data?.meta){ const m=ev.data.meta; if(Array.isArray(m.images)) images=Math.max(images,m.images.length); if(m.bbox) bbox=true; } } }
  expect(called).toBe(true); expect(images).toBeGreaterThanOrEqual(2); expect(bbox).toBe(true);
 }finally{ await fixture.close(); }
});
