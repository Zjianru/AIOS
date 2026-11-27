/** 图片发出那一刻是「原图 / 一行说明 / 由谁转述」；历史不跟着后来的设置变。 */
export type ImageSentAs =
  | { kind: 'omitted' }
  | { kind: 'paraphrased'; model: string };

const KEY = 'aios:image-sent-as';

function readMap(): Record<string, ImageSentAs> {
  try {
    return JSON.parse(localStorage.getItem(KEY) ?? '{}') as Record<string, ImageSentAs>;
  } catch {
    return {};
  }
}

export function markImageSent(id: string, sentAs: ImageSentAs): void {
  const map = readMap();
  map[id] = sentAs;
  try {
    localStorage.setItem(KEY, JSON.stringify(map));
  } catch {
    // 本地记录只服务历史展示；写不进去不挡发送。
  }
}

export function parseImageSentAs(value: string | null | undefined): ImageSentAs | undefined {
  if (!value) return undefined;
  if (value === 'omitted') return { kind: 'omitted' };
  if (value.startsWith('paraphrased:')) return { kind: 'paraphrased', model: value.slice('paraphrased:'.length) };
  return undefined;
}

export function imageSentAs(id: string): ImageSentAs | undefined {
  return readMap()[id];
}

export function imageOmittedLine(id: string): string {
  return '(image omitted: model does not support images; attachments: ' + id + ')';
}

export const NO_VISION_TEXT =
  '现在没有看图能力：这个模型看不了图，去「设置 › 模型与连接 › 看图」填一个能看图的模型。';
