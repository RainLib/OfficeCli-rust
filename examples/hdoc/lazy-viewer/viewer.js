const bundleInput = document.querySelector('#bundle');
const revisionInput = document.querySelector('#revision');
const cacheInput = document.querySelector('#cache');
const loadButton = document.querySelector('#load');
const status = document.querySelector('#status');
const frame = document.querySelector('#viewer');

const INDEX_PAGE_SIZE = 128;
const MAX_INDEX_PAGES = 10_000;
const MAX_RESIDENT_CHUNKS = 64;

function normalizedBase(value) {
  const url = new URL(value, window.location.href);
  if (!url.pathname.endsWith('/')) url.pathname += '/';
  return url;
}

async function fetchChecked(url, kind) {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${kind}: ${response.status} ${response.statusText}`);
  return response;
}

function revisionHref(revision) {
  return `revisions/${String(revision).padStart(20, '0')}.json`;
}

function indexHref(prefix, page) {
  return `${prefix}/${String(page).padStart(6, '0')}.json`;
}

function escapeCss(value) {
  return value.replace(/<\/style/gi, '<\\/style');
}

function escapeAttribute(value) {
  return String(value)
    .replaceAll('&', '&amp;')
    .replaceAll('"', '&quot;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;');
}

async function sha256(value) {
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(value));
  return [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, '0')).join('');
}

class LazyHcdViewer {
  constructor(base, revision, residentLimit) {
    this.base = base;
    this.requestedRevision = revision;
    this.residentLimit = Math.max(2, Math.min(MAX_RESIDENT_CHUNKS, residentLimit));
    this.assets = new Map();
    this.slots = new Map();
    this.loadedIndexPages = new Set();
    this.resident = 0;
    this.generation = crypto.randomUUID();
  }

  resolve(href) {
    return new URL(href, this.base);
  }

  async open() {
    this.manifest = await (await fetchChecked(this.resolve('manifest.json'), 'manifest')).json();
    if (this.manifest.schemaVersion !== 'hcd/1') throw new Error(`不支持 ${this.manifest.schemaVersion}`);
    if (this.manifest.indexPageCount > MAX_INDEX_PAGES) throw new Error('indexPageCount 超过前端安全上限');
    this.revision = this.requestedRevision ?? this.manifest.revision;
    if (this.revision > this.manifest.revision) throw new Error(`revision ${this.revision} 超过 head ${this.manifest.revision}`);
    this.indexPrefix = this.manifest.indexPrefix;
    let assetIndexHref = 'assets/index.json';
    const record = await (await fetchChecked(this.resolve(revisionHref(this.revision)), 'revision')).json();
    assetIndexHref = record.assetIndexHref || assetIndexHref;
    if (this.revision !== this.manifest.revision) {
      this.indexPrefix = record.indexPrefix;
    }
    const [styles, assets] = await Promise.all([
      fetchChecked(this.resolve(this.manifest.stylesHref), 'styles').then((response) => response.text()),
      fetchChecked(this.resolve(assetIndexHref), 'assets').then((response) => response.json()),
    ]);
    for (const asset of assets) this.assets.set(asset.hash, this.resolve(asset.href).toString());
    this.createDocument(styles);
    if (this.manifest.indexPageCount > 0) await this.loadIndexPage(0);
    this.observeSentinel();
    this.updateStatus();
  }

  createDocument(styles) {
    const doc = frame.contentDocument;
    doc.open();
    doc.write(`<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><style>
      html{background:#eef1f5}body{box-sizing:border-box;max-width:max-content;min-width:min(100%,960px);margin:24px auto;padding:24px;background:#fff;color:#111;box-shadow:0 3px 18px #0002}
      .hcd-lazy-placeholder{display:block;box-sizing:border-box;min-height:480px;margin:0 0 12px;background:linear-gradient(100deg,#f4f6f8 30%,#e9edf1 45%,#f4f6f8 60%);background-size:300% 100%;animation:hcd-pulse 1.5s infinite;border:1px solid #d7dce2;border-radius:3px}
      .hcd-lazy-error{padding:18px;color:#9f1239;background:#fff1f2;border:1px solid #fda4af}
      #hcd-lazy-tail{height:2px}
      @keyframes hcd-pulse{0%{background-position:100% 0}100%{background-position:0 0}}
      ${escapeCss(styles)}
    </style></head><body data-hcd-profile="${escapeAttribute(this.manifest.profile)}" data-hcd-source-format="${escapeAttribute(this.manifest.source.format)}" data-hcd-revision="${this.revision}" data-hcd-text-hitboxes="on" data-hcd-image-hitboxes="on"><div id="hcd-lazy-tail"></div></body></html>`);
    doc.close();
    this.doc = doc;
    this.tail = doc.querySelector('#hcd-lazy-tail');
    this.observer = new frame.contentWindow.IntersectionObserver(
      (entries) => this.onIntersection(entries),
      { root: null, rootMargin: '120% 0px 120% 0px' },
    );
  }

  async loadIndexPage(pageNumber) {
    if (pageNumber >= this.manifest.indexPageCount || this.loadedIndexPages.has(pageNumber)) return;
    this.loadedIndexPages.add(pageNumber);
    const page = await (await fetchChecked(this.resolve(indexHref(this.indexPrefix, pageNumber)), `index page ${pageNumber}`)).json();
    if (page.revision !== this.revision || page.page !== pageNumber) throw new Error(`index page ${pageNumber} revision 不匹配`);
    for (const descriptor of page.chunks) {
      const placeholder = this.doc.createElement('section');
      placeholder.className = 'hcd-lazy-placeholder';
      placeholder.dataset.hcdSequence = String(descriptor.sequence);
      placeholder.dataset.hcdChunkId = descriptor.chunkId;
      placeholder.setAttribute('aria-label', `HCD chunk ${descriptor.sequence}`);
      this.doc.body.insertBefore(placeholder, this.tail);
      this.slots.set(descriptor.sequence, {
        descriptor,
        node: placeholder,
        height: 480,
        loaded: false,
        visible: false,
        touched: 0,
      });
      this.observer.observe(placeholder);
    }
    this.nextIndexPage = pageNumber + 1;
  }

  observeSentinel() {
    this.tailObserver = new frame.contentWindow.IntersectionObserver(async (entries) => {
      if (!entries.some((entry) => entry.isIntersecting)) return;
      const next = this.nextIndexPage ?? 0;
      if (next >= this.manifest.indexPageCount) return;
      try {
        await this.loadIndexPage(next);
        this.updateStatus();
      } catch (error) {
        this.fail(error);
      }
    }, { rootMargin: '200% 0px' });
    this.tailObserver.observe(this.tail);
  }

  onIntersection(entries) {
    for (const entry of entries) {
      const sequence = Number(entry.target.dataset.hcdSequence);
      const slot = this.slots.get(sequence);
      if (!slot) continue;
      slot.visible = entry.isIntersecting;
      if (entry.isIntersecting) {
        slot.touched = performance.now();
        void this.loadChunk(slot).catch((error) => this.showChunkError(slot, error));
      }
    }
    this.evict();
  }

  async loadChunk(slot) {
    if (slot.loaded || slot.loading) return slot.loading;
    const generation = this.generation;
    slot.loading = (async () => {
      const [html, mapText] = await Promise.all([
        fetchChecked(this.resolve(slot.descriptor.htmlHref), `chunk ${slot.descriptor.sequence}`).then((response) => response.text()),
        fetchChecked(this.resolve(slot.descriptor.mapHref), `map ${slot.descriptor.sequence}`).then((response) => response.text()),
      ]);
      if (generation !== this.generation) return;
      const [actualHash, actualMapHash] = await Promise.all([sha256(html), sha256(mapText)]);
      if (actualHash !== slot.descriptor.htmlHash) {
        throw new Error(`chunk ${slot.descriptor.sequence} hash 不匹配`);
      }
      if (actualMapHash !== slot.descriptor.mapHash) {
        throw new Error(`map ${slot.descriptor.sequence} hash 不匹配`);
      }
      const sourceMap = JSON.parse(mapText);
      if (sourceMap.chunkId !== slot.descriptor.chunkId) {
        throw new Error(`map ${slot.descriptor.sequence} chunkId 不匹配`);
      }
      const rewritten = html.replace(/asset:\/\/sha256\/([0-9a-f]{64})/g, (source, hash) => this.assets.get(hash) ?? source);
      const template = this.doc.createElement('template');
      template.innerHTML = rewritten;
      for (const entry of sourceMap.entries) {
        const matches = template.content.querySelectorAll(`[data-hcd-id="${CSS.escape(entry.nodeId)}"]`);
        if (matches.length !== 1) throw new Error(`nodeId ${entry.nodeId} 出现 ${matches.length} 次`);
        const nodeHash = await sha256(matches[0].textContent ?? '');
        if (nodeHash !== entry.nodeHash) throw new Error(`nodeId ${entry.nodeId} hash 不匹配`);
        if (entry.source.nodeKind === 'image') await verifyImageNode(matches[0], entry.nodeId);
      }
      const roots = [...template.content.children];
      let node;
      if (roots.length === 1) {
        node = roots[0];
      } else {
        node = this.doc.createElement('section');
        node.className = 'hcd-chunk';
        node.append(template.content);
      }
      node.dataset.hcdSequence = String(slot.descriptor.sequence);
      node.dataset.hcdChunkId ||= slot.descriptor.chunkId;
      slot.node.replaceWith(node);
      this.observer.unobserve(slot.node);
      slot.node = node;
      slot.loaded = true;
      slot.touched = performance.now();
      this.resident += 1;
      this.observer.observe(node);
      requestAnimationFrame(() => {
        const measured = node.getBoundingClientRect().height;
        if (measured > 0) slot.height = measured;
      });
      this.evict();
      this.updateStatus();
    })().finally(() => { slot.loading = null; });
    return slot.loading;
  }

  evict() {
    if (this.resident <= this.residentLimit) return;
    const candidates = [...this.slots.values()]
      .filter((slot) => slot.loaded && !slot.visible && !slot.loading)
      .sort((left, right) => left.touched - right.touched);
    for (const slot of candidates) {
      if (this.resident <= this.residentLimit) break;
      const placeholder = this.doc.createElement('section');
      placeholder.className = 'hcd-lazy-placeholder';
      placeholder.dataset.hcdSequence = String(slot.descriptor.sequence);
      placeholder.dataset.hcdChunkId = slot.descriptor.chunkId;
      placeholder.style.minHeight = `${Math.max(1, Math.ceil(slot.height))}px`;
      slot.node.replaceWith(placeholder);
      this.observer.unobserve(slot.node);
      slot.node = placeholder;
      slot.loaded = false;
      slot.visible = false;
      this.resident -= 1;
      this.observer.observe(placeholder);
    }
    this.updateStatus();
  }

  showChunkError(slot, error) {
    slot.node.className = 'hcd-lazy-error';
    slot.node.textContent = error instanceof Error ? error.message : String(error);
  }

  updateStatus() {
    status.textContent = `revision ${this.revision} · ${this.resident}/${this.residentLimit} 驻留 · ${this.slots.size}/${this.manifest.chunkCount} 分片已索引`;
  }

  fail(error) {
    status.textContent = error instanceof Error ? error.message : String(error);
  }

  close() {
    this.generation = crypto.randomUUID();
    this.observer?.disconnect();
    this.tailObserver?.disconnect();
  }
}

function canonicalNumber(value) {
  if (Object.is(value, -0) || value === 0) return '0';
  return Number(value).toFixed(6).replace(/0+$/, '').replace(/\.$/, '');
}

async function verifyImageNode(node, nodeId) {
  const declared = node.dataset.hcdVisualHash;
  if (!declared) throw new Error(`image node ${nodeId} 没有 visualHash`);
  const asset = node.dataset.hcdAssetHash || 'none';
  const values = ['hcdX', 'hcdY', 'hcdWidth', 'hcdHeight'].map((name) => node.dataset[name]);
  let geometry = 'none';
  if (values.some((value) => value !== undefined)) {
    if (values.some((value) => value === undefined)) throw new Error(`image node ${nodeId} 几何字段不完整`);
    const unit = node.dataset.hcdGeometryUnit;
    if (unit !== 'emu' && unit !== 'pt') throw new Error(`image node ${nodeId} 几何单位无效`);
    const numbers = values.map(Number);
    if (numbers.some((value) => !Number.isFinite(value))) throw new Error(`image node ${nodeId} 几何值无效`);
    geometry = `${numbers.map(canonicalNumber).join(',')},${unit}`;
  }
  const actual = await sha256(`officecli-hcd-image/1\0asset=${asset}\0geometry=${geometry}`);
  if (actual !== declared) throw new Error(`image node ${nodeId} visualHash 不匹配`);
}

let activeViewer;

async function load() {
  activeViewer?.close();
  const params = new URLSearchParams(window.location.search);
  const bundle = bundleInput.value.trim() || params.get('bundle') || '/hcd/';
  const revision = revisionInput.value === '' ? undefined : Number(revisionInput.value);
  const cache = Number(cacheInput.value) || 12;
  const next = new URL(window.location.href);
  next.searchParams.set('bundle', bundle);
  if (revision === undefined) next.searchParams.delete('revision');
  else next.searchParams.set('revision', String(revision));
  next.searchParams.set('cache', String(cache));
  window.history.replaceState(null, '', next);
  status.textContent = '正在读取 manifest…';
  activeViewer = new LazyHcdViewer(normalizedBase(bundle), revision, cache);
  try {
    await activeViewer.open();
  } catch (error) {
    activeViewer.fail(error);
  }
}

const params = new URLSearchParams(window.location.search);
if (params.has('bundle')) bundleInput.value = params.get('bundle');
if (params.has('revision')) revisionInput.value = params.get('revision');
if (params.has('cache')) cacheInput.value = params.get('cache');
loadButton.addEventListener('click', load);
load();
