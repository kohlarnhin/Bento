import { invoke } from '@tauri-apps/api/core';

// Images AI CLIs write into a project: the 图片产物 pane lists them, and image paths printed in a
// terminal can be hovered for a thumbnail or clicked to open there.

export interface ImageArtifact { path: string; relative: string; modifiedMs: number; size: number }
export interface ImageData { path: string; dataUrl: string; size: number; modifiedMs: number }
export interface ImagePreviewRequest { projectId: string; path: string; base: Promise<string | null> }

const EXTENSIONS = 'png|jpe?g|webp|gif|svg|avif|bmp';
const SEGMENT = String.raw`[\w@%+\-.\u4e00-\u9fff]+`;
const IMAGE_PATH = new RegExp(String.raw`(?:~\/|\.{1,2}\/|\/)?(?:${SEGMENT}\/)*${SEGMENT}\.(?:${EXTENSIONS})(?![\w/]|\.\w)`, 'giu');
const WEB_URL = /[a-z][\w+.-]*:\/\/\S+/giu;

/** Image paths in one line of terminal text, skipping ones that are part of a URL (web links handle those). */
export function findImagePaths(line: string): { text: string; index: number }[] {
  const urls = [...line.matchAll(WEB_URL)].map(match => [match.index ?? 0, (match.index ?? 0) + match[0].length]);
  const found: { text: string; index: number }[] = [];
  for (const match of line.matchAll(IMAGE_PATH)) {
    const index = match.index ?? 0;
    if (urls.some(([start, end]) => index < end && index + match[0].length > start)) continue;
    found.push({ text: match[0], index });
  }
  return found;
}

const IMAGE_FILE = new RegExp(String.raw`\.(?:${EXTENSIONS})$`, 'iu');
export const isImagePath = (path: string) => IMAGE_FILE.test(path);

/** The local path behind a terminal hyperlink (OSC 8): a file:// URI or a bare absolute/~ path. */
export function linkPath(uri: string): string | null {
  if (/^file:\/\//iu.test(uri)) {
    try { return decodeURIComponent(new URL(uri).pathname); } catch { return null; }
  }
  return uri.startsWith('/') || uri.startsWith('~/') ? uri : null;
}

const cache = new Map<string, Promise<ImageData>>();
const CACHE_LIMIT = 80;

/** Reads an image as a data URL; `version` (its modified time) refreshes the cache when it changes. */
export function loadImage(path: string, base?: string | null, version?: number): Promise<ImageData> {
  const key = `${base ?? ''}\u0000${path}\u0000${version ?? ''}`;
  let image = cache.get(key);
  if (image) {
    cache.delete(key);
  } else {
    image = invoke<ImageData>('read_image_file', { path, base: base ?? null });
    image.catch(() => cache.delete(key));
  }
  cache.set(key, image);
  while (cache.size > CACHE_LIMIT) cache.delete(cache.keys().next().value!);
  return image;
}

export const IMAGE_PREVIEW_EVENT = 'bento-image-preview';

export function requestImagePreview(request: ImagePreviewRequest) {
  window.dispatchEvent(new CustomEvent<ImagePreviewRequest>(IMAGE_PREVIEW_EVENT, { detail: request }));
}

export function formatBytes(size: number) {
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(size < 10 * 1024 ? 1 : 0)} KB`;
  return `${(size / 1024 / 1024).toFixed(1)} MB`;
}

export function fileName(path: string) {
  return path.replace(/[/\\]+$/u, '').split(/[/\\]/u).pop() || path;
}

let tip: HTMLDivElement | undefined;
let tipToken = 0;
const TIP_WIDTH = 260;
const TIP_HEIGHT = 230;

function placeTip(element: HTMLDivElement, event: MouseEvent) {
  const left = Math.max(8, Math.min(event.clientX + 14, window.innerWidth - TIP_WIDTH - 8));
  const below = event.clientY + 18;
  const top = below + TIP_HEIGHT > window.innerHeight - 8 ? Math.max(8, event.clientY - TIP_HEIGHT - 10) : below;
  element.style.left = `${left}px`;
  element.style.top = `${top}px`;
}

/** A thumbnail that follows a hovered image path in the terminal. */
export function showImageTip(event: MouseEvent, path: string, base: Promise<string | null>) {
  const token = ++tipToken;
  tip ??= Object.assign(document.createElement('div'), { className: 'image-link-tip' });
  const element = tip;
  element.dataset.state = 'loading';
  element.replaceChildren(Object.assign(document.createElement('span'), { textContent: '加载预览…' }));
  placeTip(element, event);
  document.body.append(element);
  void base.then(resolved => loadImage(path, resolved)).then(image => {
    if (token !== tipToken) return;
    element.dataset.state = 'ready';
    const picture = Object.assign(document.createElement('img'), { src: image.dataUrl, alt: fileName(image.path) });
    const caption = Object.assign(document.createElement('span'), { textContent: `${fileName(image.path)} · ${formatBytes(image.size)} · 点击查看` });
    element.replaceChildren(picture, caption);
  }, error => {
    if (token !== tipToken) return;
    element.dataset.state = 'error';
    element.replaceChildren(Object.assign(document.createElement('span'), { textContent: String(error) }));
  });
}

export function hideImageTip() {
  tipToken++;
  tip?.remove();
}
