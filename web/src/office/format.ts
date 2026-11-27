/** Office 格式判断与卡片文案（xlsx/docx/pptx 三种）。 */

export type OfficeKind = 'xlsx' | 'docx' | 'pptx';

const EXTENSIONS: Record<string, OfficeKind> = {
  xlsx: 'xlsx',
  xlsm: 'xlsx',
  docx: 'docx',
  pptx: 'pptx',
};

export function officeKindOf(name: string, mime?: string | null): OfficeKind | null {
  const dot = name.lastIndexOf('.');
  const extension = dot > 0 ? name.slice(dot + 1).toLowerCase() : '';
  const byName = EXTENSIONS[extension];
  if (byName) return byName;
  if (mime === 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet') return 'xlsx';
  if (mime === 'application/vnd.openxmlformats-officedocument.wordprocessingml.document') return 'docx';
  if (mime === 'application/vnd.openxmlformats-officedocument.presentationml.presentation') return 'pptx';
  return null;
}

export function officeLabel(kind: OfficeKind): string {
  return kind === 'xlsx' ? 'XLSX' : kind === 'docx' ? 'DOCX' : 'PPTX';
}

export function officeSizeLabel(bytes: number): string {
  if (bytes < 1024) return bytes + ' B';
  if (bytes < 1024 * 1024) return Math.max(1, Math.round(bytes / 1024)) + ' KB';
  return (bytes / (1024 * 1024)).toFixed(1) + ' MB';
}

export function officeKindLabel(kind: OfficeKind): string {
  return kind === 'xlsx' ? '表格' : kind === 'docx' ? 'Word 文档' : '幻灯片';
}
