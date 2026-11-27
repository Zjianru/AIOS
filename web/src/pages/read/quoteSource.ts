/** 阅读页引用到对话的来源形状（kind=file；id/title/version 一处拼）。 */
export interface ReadQuoteSource {
  kind: 'file';
  id: string;
  title: string;
  version: number;
}

export function readQuoteSource(fileName: string, rawVersion: string | number): ReadQuoteSource {
  const version = Number(rawVersion);
  return {
    kind: 'file',
    id: fileName,
    title: fileName,
    version: Number.isFinite(version) && version > 0 ? version : 1,
  };
}
