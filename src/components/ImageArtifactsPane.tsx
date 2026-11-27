import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { copyText } from '../lib/lanAddress';
import { fileName, formatBytes, loadImage, type ImageArtifact, type ImageData } from '../lib/imageArtifacts';

type Range = 'hour' | 'today' | 'all';
const RANGES: { value: Range; label: string }[] = [
  { value: 'hour', label: '1 小时' },
  { value: 'today', label: '今天' },
  { value: 'all', label: '全部' },
];
const POLL_MS = 3000;

export interface ImageFocus { path: string; base: Promise<string | null>; nonce: number }

function since(range: Range) {
  if (range === 'all') return 0;
  if (range === 'hour') return Date.now() - 3600_000;
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  return today.getTime();
}

function timeLabel(ms: number) {
  const date = new Date(ms);
  const sameDay = date.toDateString() === new Date().toDateString();
  const time = date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit', hour12: false });
  return sameDay ? time : `${date.getMonth() + 1}/${date.getDate()} ${time}`;
}

function Thumb({ image, fresh, onOpen }: { image: ImageArtifact; fresh: boolean; onOpen: () => void }) {
  const button = useRef<HTMLButtonElement>(null);
  const [src, setSrc] = useState<string>();
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    const element = button.current;
    if (!element) return;
    let cancelled = false;
    const observer = new IntersectionObserver(entries => {
      if (!entries.some(entry => entry.isIntersecting)) return;
      observer.disconnect();
      loadImage(image.path, null, image.modifiedMs).then(data => { if (!cancelled) setSrc(data.dataUrl); }, () => { if (!cancelled) setFailed(true); });
    }, { rootMargin: '200px' });
    observer.observe(element);
    return () => { cancelled = true; observer.disconnect(); };
  }, [image.path, image.modifiedMs]);
  return <button ref={button} type="button" className="image-artifact" title={image.relative} onClick={onOpen} data-image-artifact>
    <span className="image-artifact-frame">
      {src ? <img src={src} alt={fileName(image.path)} draggable={false} /> : <span>{failed ? '无法预览' : ''}</span>}
      {fresh && <span className="image-artifact-new">新</span>}
    </span>
    <span className="image-artifact-name">{fileName(image.path)}</span>
    <span className="image-artifact-meta">{timeLabel(image.modifiedMs)} · {formatBytes(image.size)}</span>
  </button>;
}

function Viewer({ path, base, images, onNavigate, onClose }: {
  path: string; base: Promise<string | null> | null; images: ImageArtifact[];
  onNavigate: (path: string) => void; onClose: () => void;
}) {
  const [image, setImage] = useState<ImageData>();
  const [error, setError] = useState<string>();
  const known = images.find(entry => entry.path === path);
  const index = known ? images.indexOf(known) : -1;
  useEffect(() => {
    let cancelled = false;
    setImage(undefined);
    setError(undefined);
    void Promise.resolve(base).then(resolved => loadImage(path, resolved, known?.modifiedMs))
      .then(data => { if (!cancelled) setImage(data); }, reason => { if (!cancelled) setError(String(reason)); });
    return () => { cancelled = true; };
  }, [path, base, known?.modifiedMs]);
  // Keys only while the viewer itself has focus, so typing in the terminal beside it is untouched.
  const viewer = useRef<HTMLDivElement>(null);
  useEffect(() => { viewer.current?.focus({ preventScroll: true }); }, []);
  const keydown = (event: KeyboardEvent) => {
    if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); onClose(); }
    else if ((event.key === 'ArrowLeft' || event.key === 'ArrowRight') && index >= 0) {
      const next = images[index + (event.key === 'ArrowLeft' ? -1 : 1)];
      if (next) { event.preventDefault(); onNavigate(next.path); }
    }
  };
  const shown = image?.path ?? path;
  return <div ref={viewer} className="image-viewer" tabIndex={-1} onKeyDown={keydown} data-image-viewer>
    <div className="image-viewer-bar">
      <button type="button" onClick={onClose} aria-label="返回列表">←</button>
      <span className="image-viewer-name" title={shown}>{fileName(shown)}{image && ` · ${formatBytes(image.size)}`}</span>
      {index >= 0 && <span className="image-viewer-count">{index + 1}/{images.length}</span>}
      <button type="button" disabled={!image} onClick={() => image && copyText(image.path, '已复制图片路径')}>复制路径</button>
      <button type="button" disabled={!image} onClick={() => image && void invoke('reveal_path', { path: image.path })}>在 Finder 显示</button>
    </div>
    <div className="image-viewer-stage">
      {image ? <img src={image.dataUrl} alt={fileName(image.path)} draggable={false} />
        : <p className="terminal-log-empty">{error ?? '加载中…'}</p>}
    </div>
  </div>;
}

/** The 图片产物 split pane: recent images in the project, refreshed while it is open. */
export function ImageArtifactsPane({ projectPath, focus }: { projectPath: string; focus?: ImageFocus }) {
  const [range, setRange] = useState<Range>('today');
  const [images, setImages] = useState<ImageArtifact[]>();
  const [error, setError] = useState<string>();
  const [viewing, setViewing] = useState<{ path: string; base: Promise<string | null> | null }>();
  const seen = useRef<Map<string, number>>();
  const [fresh, setFresh] = useState<Set<string>>(new Set());

  useEffect(() => {
    let disposed = false;
    let timer = 0;
    seen.current = undefined;
    setFresh(new Set());
    setImages(undefined);
    const refresh = async () => {
      if (document.visibilityState === 'visible') {
        try {
          const next = await invoke<ImageArtifact[]>('project_images', { path: projectPath, sinceMs: since(range), limit: 300 });
          if (disposed) return;
          const previous = seen.current;
          if (previous) {
            const added = next.filter(image => previous.get(image.path) !== image.modifiedMs).map(image => image.path);
            if (added.length) setFresh(current => new Set([...current, ...added]));
          }
          seen.current = new Map(next.map(image => [image.path, image.modifiedMs]));
          setImages(current => current && current.length === next.length && current.every((image, index) =>
            image.path === next[index].path && image.modifiedMs === next[index].modifiedMs) ? current : next);
          setError(undefined);
        } catch (reason) {
          if (!disposed) setError(String(reason));
        }
      }
      if (!disposed) timer = window.setTimeout(refresh, POLL_MS);
    };
    void refresh();
    return () => { disposed = true; window.clearTimeout(timer); };
  }, [projectPath, range]);

  useEffect(() => {
    if (focus) setViewing({ path: focus.path, base: focus.base });
  }, [focus?.nonce]);

  const list = useMemo(() => images ?? [], [images]);
  const close = useCallback(() => setViewing(undefined), []);
  const navigate = useCallback((path: string) => setViewing({ path, base: null }), []);

  if (viewing) return <Viewer path={viewing.path} base={viewing.base} images={list} onNavigate={navigate} onClose={close} />;
  return <div className="image-artifacts" data-image-artifacts>
    <div className="image-artifacts-bar" role="radiogroup" aria-label="时间范围">
      {RANGES.map(option => <button key={option.value} type="button" role="radio" aria-checked={range === option.value}
        onClick={() => setRange(option.value)}>{option.label}</button>)}
      <span className="image-artifacts-count">{images ? `${images.length} 张` : ''}</span>
    </div>
    {error ? <p className="terminal-log-empty">{error}</p>
      : !images ? <p className="terminal-log-empty">正在查找图片…</p>
      : !images.length ? <p className="terminal-log-empty">{range === 'all' ? '项目里还没有图片。' : '这段时间没有新图片。'}AI 生成或修改的图片会自动出现在这里。</p>
      : <div className="image-artifacts-grid">
        {images.map(image => <Thumb key={image.path} image={image} fresh={fresh.has(image.path)}
          onOpen={() => setViewing({ path: image.path, base: null })} />)}
      </div>}
  </div>;
}
