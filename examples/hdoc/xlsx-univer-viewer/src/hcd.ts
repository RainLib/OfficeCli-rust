export type GridChunkKind = 'cells' | 'picture' | 'chart';

export interface GridChunkAddress {
  sheetId: string;
  sheetName: string;
  sheetIndex: number;
  sheetState: 'visible' | 'hidden' | 'veryHidden';
  kind: GridChunkKind;
  rowStart?: number;
  rowEnd?: number;
  columnStart?: number;
  columnEnd?: number;
  defaultColumnWidthEmu?: number;
  defaultRowHeightEmu?: number;
}

export interface ChunkDescriptor {
  sequence: number;
  chunkId: string;
  htmlHref: string;
  mapHref: string;
  nodeCount: number;
  grid?: GridChunkAddress;
}

export interface HcdManifest {
  schemaVersion: 'hcd/1';
  documentId: string;
  profile: string;
  revision: number;
  rootHash: string;
  indexPrefix: string;
  indexPageCount: number;
  stylesHref: string;
}

export interface NodeMapEntry {
  nodeId: string;
  nodeHash: string;
  source: {
    part: string;
    nodeKind: string;
    editable: boolean;
  };
}

export interface ChunkSourceMap {
  chunkId: string;
  entries: NodeMapEntry[];
}

interface ChunkIndexPage {
  chunks: ChunkDescriptor[];
}

export interface LoadedChunk {
  descriptor: ChunkDescriptor;
  html: string;
  map: ChunkSourceMap;
}

function normalizeBaseUrl(value: string): URL {
  const url = new URL(value || '/hcd/', window.location.href);
  if (!url.pathname.endsWith('/')) url.pathname += '/';
  return url;
}

async function fetchChecked(url: URL): Promise<Response> {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${response.status} ${response.statusText}: ${url}`);
  return response;
}

export class HcdBundleClient {
  readonly baseUrl: URL;
  manifest!: HcdManifest;
  descriptors: ChunkDescriptor[] = [];

  constructor(baseUrl: string) {
    this.baseUrl = normalizeBaseUrl(baseUrl);
  }

  resolve(href: string): URL {
    return new URL(href, this.baseUrl);
  }

  async open(): Promise<void> {
    this.manifest = await (await fetchChecked(this.resolve('manifest.json'))).json() as HcdManifest;
    if (this.manifest.schemaVersion !== 'hcd/1' || this.manifest.profile !== 'grid') {
      throw new Error(`需要 hcd/1 grid bundle，实际为 ${this.manifest.schemaVersion} ${this.manifest.profile}`);
    }
    if (this.manifest.indexPageCount > 10_000) {
      throw new Error(`indexPageCount ${this.manifest.indexPageCount} 超过前端安全上限 10000`);
    }
    const pages: ChunkIndexPage[] = [];
    for (let offset = 0; offset < this.manifest.indexPageCount; offset += 8) {
      const batch = Array.from(
        { length: Math.min(8, this.manifest.indexPageCount - offset) },
        async (_, index) => {
          const page = offset + index;
          const href = `${this.manifest.indexPrefix}/${page.toString().padStart(6, '0')}.json`;
          return await (await fetchChecked(this.resolve(href))).json() as ChunkIndexPage;
        },
      );
      pages.push(...await Promise.all(batch));
    }
    this.descriptors = pages.flatMap((page) => page.chunks).sort((a, b) => a.sequence - b.sequence);
    if (this.descriptors.some((descriptor) => descriptor.grid === undefined)) {
      throw new Error('此 bundle 没有 grid 随机访问元数据，请用当前 OfficeCLI 重新执行 hdoc import');
    }
  }

  sheets(): GridChunkAddress[] {
    const found = new Map<string, GridChunkAddress>();
    for (const descriptor of this.descriptors) {
      const grid = descriptor.grid!;
      if (!found.has(grid.sheetId)) found.set(grid.sheetId, grid);
    }
    return [...found.values()].sort((a, b) => a.sheetIndex - b.sheetIndex);
  }

  cellWindows(sheetId: string, rowStart: number, rowEnd: number): ChunkDescriptor[] {
    return this.descriptors.filter(({ grid }) => grid?.sheetId === sheetId
      && grid.kind === 'cells'
      && (grid.rowStart === undefined || grid.rowEnd === undefined
        || (grid.rowEnd >= rowStart && grid.rowStart <= rowEnd)));
  }

  visuals(sheetId: string): ChunkDescriptor[] {
    return this.descriptors.filter(({ grid }) => grid?.sheetId === sheetId && grid.kind !== 'cells');
  }

  sheetDefaults(sheetId: string): { columnWidth: number; rowHeight: number } {
    const grids = this.descriptors
      .map(({ grid }) => grid)
      .filter((grid): grid is GridChunkAddress => grid?.sheetId === sheetId);
    const columnWidthEmu = grids.find(({ defaultColumnWidthEmu }) => defaultColumnWidthEmu)?.defaultColumnWidthEmu;
    const rowHeightEmu = grids.find(({ defaultRowHeightEmu }) => defaultRowHeightEmu)?.defaultRowHeightEmu;
    return {
      columnWidth: columnWidthEmu ? columnWidthEmu / 9525 : 64,
      rowHeight: rowHeightEmu ? rowHeightEmu / 9525 : 20,
    };
  }

  async readChunk(descriptor: ChunkDescriptor): Promise<LoadedChunk> {
    const [html, map] = await Promise.all([
      fetchChecked(this.resolve(descriptor.htmlHref)).then((response) => response.text()),
      fetchChecked(this.resolve(descriptor.mapHref)).then((response) => response.json() as Promise<ChunkSourceMap>),
    ]);
    return { descriptor, html, map };
  }

  async readStyles(): Promise<string> {
    return (await fetchChecked(this.resolve(this.manifest.stylesHref))).text();
  }
}
