/**
 * 看图工具产出的图：工具卡里的标注图/放大图，以及 vision_present 挂在回答下的图。
 * 图从附件仓库按 id 读；读不到就画一个占位，不让整轮失败。
 */

import { useEffect, useState } from 'react';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import { attachmentGet } from '@/data/service';

export interface VisionImageRef {
  id: string;
  name?: string;
}

function useImageUrl(id: string): string | null {
  const [src, setSrc] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    setSrc(null);
    void attachmentGet(id)
      .then((result) => {
        if (!alive) return;
        if (result.mime && result.data) setSrc('data:' + result.mime + ';base64,' + result.data);
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [id]);
  return src;
}

function Thumb({ image, height }: { image: VisionImageRef; height: number }) {
  const src = useImageUrl(image.id);
  return (
    <figure className="flex shrink-0 flex-col gap-1" data-kind="vision-image" data-image={image.id}>
      <Popover>
        <PopoverTrigger asChild>
          <button type="button" className="overflow-hidden rounded-card border border-line-2 bg-pane" title={image.name ?? image.id}>
            {src ? (
              <img src={src} alt={image.name ?? '工具产出的图'} className="block w-auto object-contain" style={{ height }} />
            ) : (
              <span className="grid h-[120px] w-[160px] place-items-center text-1 text-ink-3">读图…</span>
            )}
          </button>
        </PopoverTrigger>
        <PopoverContent side="top" align="start" sideOffset={8} className="w-auto max-w-[min(960px,92vw)] rounded-card border-line-2 bg-raised p-2 shadow-[var(--shadow-pop)]">
          {src ? <img src={src} alt={image.name ?? '工具产出的图'} className="max-h-[70vh] max-w-full rounded-info" /> : null}
          <div className="px-1 pt-1.5 text-1 text-ink-3">{image.name ?? image.id}</div>
        </PopoverContent>
      </Popover>
      <figcaption className="text-1 text-ink-3">{image.name ?? ''}</figcaption>
    </figure>
  );
}

/** 工具卡里的一排图；ids 来自 tool/result.meta.images / imageAttachments。 */
export function ToolImages({ ids }: { ids: string[] }) {
  if (ids.length === 0) return null;
  return (
    <div className="flex flex-wrap items-start gap-3 px-4 py-2" data-kind="tool-images">
      {ids.map((id) => (
        <Thumb key={id} image={{ id }} height={120} />
      ))}
    </div>
  );
}

/** 回答下面：vision_present 给用户看的图。 */
export function PresentedImages({ ids }: { ids: string[] }) {
  if (ids.length === 0) return null;
  return (
    <div className="mt-3 flex flex-wrap items-start gap-3" data-kind="presented-images">
      {ids.map((id) => (
        <Thumb key={id} image={{ id }} height={160} />
      ))}
    </div>
  );
}

/** 从 tool 列表里取 vision_present 的附件 id（按出现顺序去重）。 */
export function presentedImageIds(tools: { name: string; meta?: Record<string, unknown> | null }[]): string[] {
  // 只认最后一次 vision_present；模型常常把原图/猜的文件名传进来，凑不够两张时
  // 用同一轮最后一次 vision_ground 的标注+放大图兜底（页面仍只画最终那一组）。
  const idsOf = (meta?: Record<string, unknown> | null): string[] => {
    const images = meta ? (meta.images ?? meta.imageAttachments) : null;
    if (!Array.isArray(images)) return [];
    const out: string[] = [];
    for (const id of images) if (typeof id === 'string' && !out.includes(id)) out.push(id);
    return out;
  };
  let present: string[] = [];
  let ground: string[] = [];
  for (let index = tools.length - 1; index >= 0; index -= 1) {
    const tool = tools[index];
    if (present.length === 0 && tool.name === 'vision_present') present = idsOf(tool.meta);
    if (ground.length === 0 && (tool.name === 'vision_ground' || tool.name === 'vision_detect')) {
      ground = idsOf(tool.meta);
    }
  }
  // 数量够但 id 不是这一轮 ground 产出的（模型猜了文件名/旧图）：回落 ground。
  if (present.length >= 2 && (ground.length === 0 || present.every((id) => ground.includes(id)))) {
    return present;
  }
  if (ground.length > 0) return ground;
  return present;
}
