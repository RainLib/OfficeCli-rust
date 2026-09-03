import {
  BooleanNumber,
  BorderStyleTypes,
  HorizontalAlign,
  type ICellData,
  type IStyleData,
  VerticalAlign,
  WrapStrategy,
} from '@univerjs/presets';
import type { LoadedChunk, NodeMapEntry } from './hcd.ts';

export interface NodeLink {
  nodeId: string;
  nodeHash: string;
  chunkId: string;
  editable: boolean;
  text: string;
  sheetId: string;
  row: number;
  column: number;
}

export interface ParsedCell {
  row: number;
  column: number;
  data: ICellData;
  link?: NodeLink;
  formula: boolean;
}

export interface ParsedVisual {
  nodeId: string;
  nodeHash?: string;
  kind: 'picture' | 'chart';
  assetUrl: string;
  row: number;
  column: number;
  toRow?: number;
  toColumn?: number;
  anchorKind: string;
  columnOffset: number;
  rowOffset: number;
  toColumnOffset: number;
  toRowOffset: number;
  absoluteX?: number;
  absoluteY?: number;
  extentWidth?: number;
  extentHeight?: number;
  width: number;
  height: number;
  alt: string;
}

export interface ParsedGridChunk {
  cells: ParsedCell[];
  merges: Array<{ startRow: number; endRow: number; startColumn: number; endColumn: number }>;
  rowHeights: Array<{ row: number; height?: number; hidden: boolean }>;
  columnWidths: Array<{ startColumn: number; count: number; width?: number; hidden: boolean }>;
  freeze?: { startRow: number; startColumn: number; xSplit: number; ySplit: number };
  showGridlines?: boolean;
  rightToLeft?: boolean;
  defaultColumnWidth?: number;
  defaultRowHeight?: number;
  visuals: ParsedVisual[];
}

const HORIZONTAL: Record<string, HorizontalAlign> = {
  left: HorizontalAlign.LEFT,
  center: HorizontalAlign.CENTER,
  right: HorizontalAlign.RIGHT,
  justify: HorizontalAlign.JUSTIFIED,
};

const VERTICAL: Record<string, VerticalAlign> = {
  top: VerticalAlign.TOP,
  middle: VerticalAlign.MIDDLE,
  bottom: VerticalAlign.BOTTOM,
};

function parseDeclarations(source: string): Map<string, string> {
  const declarations = new Map<string, string>();
  for (const declaration of source.split(';')) {
    const separator = declaration.indexOf(':');
    if (separator < 1) continue;
    declarations.set(
      declaration.slice(0, separator).trim().toLowerCase(),
      declaration.slice(separator + 1).trim(),
    );
  }
  return declarations;
}

function points(value: string | undefined): number | undefined {
  if (!value) return undefined;
  const parsed = Number.parseFloat(value);
  if (!Number.isFinite(parsed)) return undefined;
  return value.endsWith('px') ? parsed * 72 / 96 : parsed;
}

function border(value: string | undefined) {
  if (!value || value === 'none' || value === '0') return undefined;
  const parts = value.trim().split(/\s+/);
  const width = Number.parseFloat(parts[0]) || 1;
  const line = parts[1]?.toLowerCase();
  const color = parts.slice(2).join(' ') || '#000000';
  const style = line === 'double'
    ? BorderStyleTypes.DOUBLE
    : line === 'dashed'
      ? BorderStyleTypes.DASHED
      : line === 'dotted'
        ? BorderStyleTypes.DOTTED
        : width >= 3
          ? BorderStyleTypes.THICK
          : width >= 2
            ? BorderStyleTypes.MEDIUM
            : BorderStyleTypes.THIN;
  return { s: style, cl: { rgb: color } };
}

function toUniverStyle(declarations: Map<string, string>): IStyleData {
  const style: IStyleData = {};
  const fontFamily = declarations.get('font-family');
  if (fontFamily) style.ff = fontFamily.replace(/^['"]|['"]$/g, '').split(',')[0];
  const fontSize = points(declarations.get('font-size'));
  if (fontSize) style.fs = fontSize;
  if (Number.parseInt(declarations.get('font-weight') ?? '0', 10) >= 600) style.bl = BooleanNumber.TRUE;
  if (declarations.get('font-style') === 'italic') style.it = BooleanNumber.TRUE;
  const decoration = declarations.get('text-decoration') ?? '';
  if (decoration.includes('underline')) style.ul = { s: BooleanNumber.TRUE };
  if (decoration.includes('line-through')) style.st = { s: BooleanNumber.TRUE };
  const foreground = declarations.get('color');
  if (foreground) style.cl = { rgb: foreground };
  const background = declarations.get('background-color');
  if (background) style.bg = { rgb: background };
  const horizontal = declarations.get('text-align');
  if (horizontal && HORIZONTAL[horizontal]) style.ht = HORIZONTAL[horizontal];
  const vertical = declarations.get('vertical-align');
  if (vertical && VERTICAL[vertical]) style.vt = VERTICAL[vertical];
  style.tb = declarations.get('white-space')?.includes('wrap') ? WrapStrategy.WRAP : WrapStrategy.CLIP;
  const borders = {
    l: border(declarations.get('border-left')),
    r: border(declarations.get('border-right')),
    t: border(declarations.get('border-top')),
    b: border(declarations.get('border-bottom')),
  };
  if (Object.values(borders).some(Boolean)) style.bd = borders;
  return style;
}

export function parseStyleCatalog(css: string): Record<string, IStyleData> {
  const styles: Record<string, IStyleData> = {};
  const pattern = /\.hcd-xs-(\d+)\s*\{([^}]*)\}/g;
  for (const match of css.matchAll(pattern)) {
    styles[`hcd-xs-${match[1]}`] = toUniverStyle(parseDeclarations(match[2]));
  }
  return styles;
}

function parseCellReference(reference: string): { row: number; column: number } | undefined {
  const match = /^([A-Z]+)(\d+)$/.exec(reference.toUpperCase());
  if (!match) return undefined;
  let column = 0;
  for (const character of match[1]) column = column * 26 + character.charCodeAt(0) - 64;
  return { row: Number(match[2]) - 1, column: column - 1 };
}

function parseMerge(reference: string) {
  const [from, to] = reference.split(':').map(parseCellReference);
  if (!from || !to) return undefined;
  return { startRow: from.row, endRow: to.row, startColumn: from.column, endColumn: to.column };
}

function booleanAttribute(element: Element, name: string): boolean | undefined {
  const value = element.getAttribute(name);
  return value === null ? undefined : value === 'true';
}

function emuAttribute(element: HTMLElement, name: string): number | undefined {
  const value = element.getAttribute(name);
  if (value === null) return undefined;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed / 9525 : undefined;
}

export function parseGridChunk(chunk: LoadedChunk, resolveAsset: (href: string) => string): ParsedGridChunk {
  const document = new DOMParser().parseFromString(chunk.html, 'text/html');
  const section = document.querySelector<HTMLElement>('[data-hcd-sheet]');
  if (!section || !chunk.descriptor.grid) throw new Error(`无效的 XLSX HCD 分片 ${chunk.descriptor.chunkId}`);
  const entries = new Map<string, NodeMapEntry>(chunk.map.entries.map((entry) => [entry.nodeId, entry]));
  const cells: ParsedCell[] = [];
  for (const cell of document.querySelectorAll<HTMLTableCellElement>('td[data-hcd-cell]')) {
    const position = parseCellReference(cell.dataset.hcdCell ?? '');
    if (!position) continue;
    const node = cell.querySelector<HTMLElement>('[data-hcd-id]');
    const nodeId = node?.dataset.hcdId;
    const entry = nodeId ? entries.get(nodeId) : undefined;
    const text = node?.textContent ?? '';
    const styleIndex = cell.dataset.hcdStyleIndex;
    const link = nodeId && entry ? {
      nodeId,
      nodeHash: entry.nodeHash,
      chunkId: chunk.descriptor.chunkId,
      editable: entry.source.editable,
      text,
      sheetId: chunk.descriptor.grid.sheetId,
      row: position.row,
      column: position.column,
    } satisfies NodeLink : undefined;
    cells.push({
      ...position,
      data: { v: text, ...(styleIndex ? { s: `hcd-xs-${styleIndex}` } : {}) },
      link,
      formula: cell.dataset.hcdFormula === 'true',
    });
  }

  const merges = [...document.querySelectorAll<HTMLElement>('td[data-hcd-merge]')]
    .map((cell) => parseMerge(cell.dataset.hcdMerge ?? ''))
    .filter((value): value is NonNullable<typeof value> => value !== undefined);
  const rowHeights = [...document.querySelectorAll<HTMLTableRowElement>('tr[data-hcd-row]')].map((row) => ({
    row: Number(row.dataset.hcdRow) - 1,
    height: row.dataset.hcdHeightPoints ? Number(row.dataset.hcdHeightPoints) * 96 / 72 : undefined,
    hidden: row.dataset.hcdHidden === 'true',
  }));
  const columnWidths = [...document.querySelectorAll<HTMLTableColElement>('col[data-hcd-column-start]')].map((column) => {
    const start = Number(column.dataset.hcdColumnStart) - 1;
    const end = Number(column.dataset.hcdColumnEnd) - 1;
    return {
      startColumn: start,
      count: end - start + 1,
      width: column.dataset.hcdWidth ? Number(column.dataset.hcdWidth) * 7.5 : undefined,
      hidden: column.dataset.hcdHidden === 'true',
    };
  });

  const xSplit = Number(section.dataset.hcdFrozenColumns ?? 0);
  const ySplit = Number(section.dataset.hcdFrozenRows ?? 0);
  const freeze = xSplit || ySplit ? { startRow: ySplit, startColumn: xSplit, xSplit, ySplit } : undefined;
  const visuals: ParsedVisual[] = [];
  for (const visual of document.querySelectorAll<HTMLElement>('.hcd-sheet-picture[data-hcd-id],.hcd-sheet-chart[data-hcd-id]')) {
    const image = visual.querySelector<HTMLImageElement>('img[data-hcd-asset-href]');
    const grid = chunk.descriptor.grid;
    if (!image) continue;
    visuals.push({
      nodeId: visual.dataset.hcdId!,
      nodeHash: visual.dataset.hcdNodeHash,
      kind: grid.kind === 'chart' ? 'chart' : 'picture',
      assetUrl: resolveAsset(image.dataset.hcdAssetHref!),
      row: (grid.rowStart ?? 1) - 1,
      column: (grid.columnStart ?? 1) - 1,
      toRow: grid.rowEnd === undefined ? undefined : grid.rowEnd - 1,
      toColumn: grid.columnEnd === undefined ? undefined : grid.columnEnd - 1,
      anchorKind: visual.dataset.hcdAnchorKind ?? 'two-cell',
      columnOffset: emuAttribute(visual, 'data-hcd-from-column-offset-emu') ?? 0,
      rowOffset: emuAttribute(visual, 'data-hcd-from-row-offset-emu') ?? 0,
      toColumnOffset: emuAttribute(visual, 'data-hcd-to-column-offset-emu') ?? 0,
      toRowOffset: emuAttribute(visual, 'data-hcd-to-row-offset-emu') ?? 0,
      absoluteX: emuAttribute(visual, 'data-hcd-absolute-x-emu'),
      absoluteY: emuAttribute(visual, 'data-hcd-absolute-y-emu'),
      extentWidth: emuAttribute(visual, 'data-hcd-extent-width-emu'),
      extentHeight: emuAttribute(visual, 'data-hcd-extent-height-emu'),
      width: Number(visual.dataset.hcdWidthEmu ?? 0) / 9525,
      height: Number(visual.dataset.hcdHeightEmu ?? 0) / 9525,
      alt: image.alt,
    });
  }

  return {
    cells,
    merges,
    rowHeights,
    columnWidths,
    freeze,
    showGridlines: booleanAttribute(section, 'data-hcd-show-grid-lines'),
    rightToLeft: booleanAttribute(section, 'data-hcd-right-to-left'),
    defaultColumnWidth: section.dataset.hcdDefaultColumnWidth
      ? Number(section.dataset.hcdDefaultColumnWidth) * 7.5
      : undefined,
    defaultRowHeight: section.dataset.hcdDefaultRowHeightPoints
      ? Number(section.dataset.hcdDefaultRowHeightPoints) * 96 / 72
      : undefined,
    visuals,
  };
}
