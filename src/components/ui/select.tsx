import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import type { FocusEvent, KeyboardEvent } from 'react';
import { createPortal } from 'react-dom';
import { Check, ChevronDown, Loader2, Search } from 'lucide-react';
import { selectKeyAction } from '../../lib/selectKeyboard';
import { cn } from '../../lib/utils';
import { usePopupMotion } from './motion-presence';

export interface SelectOption {
  value: string;
  label: string;
  color?: string;
  count?: number;
}

interface Props {
  id: string;
  label: string;
  value: string;
  options: SelectOption[];
  onValueChange: (value: string) => void;
  disabled?: boolean;
  className?: string;
  popupClassName?: string;
  'aria-describedby'?: string;
  search?: {
    placeholder: string;
    creating: boolean;
    onCreate: (name: string) => Promise<void>;
  };
}

function Dot({ color }: { color: string }) {
  return <span aria-hidden="true" className="size-2 shrink-0 rounded-full" style={{ backgroundColor: color }} />;
}

export function Select({ id, label, value, options, onValueChange, disabled, className, popupClassName, search, ...props }: Props) {
  const trigger = useRef<HTMLButtonElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const composing = useRef(false);
  const [host, setHost] = useState<HTMLElement | null>(null);
  const [query, setQuery] = useState('');
  const [active, setActive] = useState<number>();
  const [error, setError] = useState('');
  const open = host !== null;
  const { panel: popup, attach, present } = usePopupMotion(open);
  const lastHost = useRef<HTMLElement | null>(null);
  if (host) lastHost.current = host;
  const popupHost = present ? lastHost.current : null;
  const listId = `${id}-listbox`;
  const selected = options.find(option => option.value === value);
  const name = query.trim();
  const filtered = name ? options.filter(option => option.label.toLowerCase().includes(name.toLowerCase())) : options;
  const exact = name ? filtered.findIndex(option => option.label === name) : -1;
  const createIndex = search && exact < 0 ? filtered.length : -1;
  const optionCount = filtered.length + Number(createIndex >= 0);
  const preferred = name ? (exact >= 0 ? exact : createIndex) : filtered.findIndex(option => option.value === value);
  const activeIndex = Math.min(active ?? (preferred >= 0 ? preferred : 0), optionCount - 1);
  const optionId = (index: number) => `${listId}-${index}`;
  const activeId = open && activeIndex >= 0 ? optionId(activeIndex) : undefined;

  const close = (restoreFocus = false) => {
    if (restoreFocus) trigger.current?.focus({ preventScroll: true });
    setHost(null);
    composing.current = false;
  };
  const openMenu = (backwards = false) => {
    const anchor = trigger.current;
    if (!anchor || disabled || search?.creating) return;
    anchor.focus({ preventScroll: true });
    setQuery('');
    setError('');
    setActive(!selected && backwards ? options.length + Number(!!search) - 1 : undefined);
    setHost(anchor.closest<HTMLElement>('[role="dialog"]') ?? anchor.ownerDocument.body);
  };

  useLayoutEffect(() => {
    const anchor = trigger.current;
    const panel = popup.current;
    const choices = list.current;
    if (!host || !anchor || !panel || !choices) return;
    const doc = anchor.ownerDocument;
    const win = doc.defaultView!;
    const position = () => {
      const rect = anchor.getBoundingClientRect();
      const container = host !== doc.body ? host.getBoundingClientRect() : null;
      const scale = container && host.offsetWidth ? container.width / host.offsetWidth : 1;
      const above = rect.top - 14;
      const below = doc.documentElement.clientHeight - rect.bottom - 14;
      const desired = Math.min(288, panel.offsetHeight + choices.scrollHeight - choices.clientHeight) * scale;
      const up = below < desired && above > below;
      panel.style.width = `${rect.width / scale}px`;
      panel.style.left = `${(rect.left - (container?.left ?? 0)) / scale - (container ? host.clientLeft : 0)}px`;
      panel.style.top = `${((up ? rect.top : rect.bottom) - (container?.top ?? 0)) / scale - (container ? host.clientTop : 0) + (up ? -6 : 6)}px`;
      panel.style.maxHeight = `${Math.min(288, Math.max(0, up ? above : below) / scale)}px`;
      panel.style.translate = up ? '0 -100%' : '';
      panel.style.transformOrigin = up ? 'bottom left' : 'top left';
    };
    position();
    win.addEventListener('resize', position);
    win.addEventListener('scroll', position, true);
    host.addEventListener('animationend', position);
    return () => {
      win.removeEventListener('resize', position);
      win.removeEventListener('scroll', position, true);
      host.removeEventListener('animationend', position);
    };
  }, [host, filtered.length, createIndex, error]);

  useLayoutEffect(() => {
    if (host) input.current?.focus({ preventScroll: true });
  }, [host]);

  useLayoutEffect(() => {
    const viewport = list.current;
    const item = activeId ? viewport?.ownerDocument.getElementById(activeId) : null;
    if (!viewport || !item || !viewport.contains(item)) return;
    const bounds = viewport.getBoundingClientRect();
    const rect = item.getBoundingClientRect();
    if (rect.top < bounds.top) viewport.scrollTop -= bounds.top - rect.top;
    else if (rect.bottom > bounds.bottom) viewport.scrollTop += rect.bottom - bounds.bottom;
  }, [activeId, filtered.length]);

  useEffect(() => {
    if (!host) return;
    const outside = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!trigger.current?.contains(target) && !popup.current?.contains(target)) {
        setHost(null);
        composing.current = false;
      }
    };
    host.ownerDocument.addEventListener('pointerdown', outside, true);
    return () => host.ownerDocument.removeEventListener('pointerdown', outside, true);
  }, [host]);

  const onBlur = (event: FocusEvent<HTMLElement>) => {
    if (open && !trigger.current?.contains(event.relatedTarget) && !popup.current?.contains(event.relatedTarget)) close();
  };
  const choose = async (index: number) => {
    if (disabled || search?.creating || index < 0) return;
    const option = filtered[index];
    if (option) {
      onValueChange(option.value);
      close(true);
    } else if (search && index === createIndex) {
      if (!name) { setError('请输入分组名称'); input.current?.focus(); return; }
      setError('');
      try {
        await search.onCreate(name);
        close(popup.current?.contains(host?.ownerDocument.activeElement ?? null));
      } catch (failure) {
        setError(String(failure));
      }
    }
  };
  const onKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    const action = selectKeyAction({ key: event.key, keyCode: event.keyCode,
      composing: composing.current || event.nativeEvent.isComposing, open, editable: open && !!search, activeIndex, optionCount });
    if (!action) return;
    if (action.kind === 'tab') {
      close(true);
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    if (action.kind === 'open') openMenu(action.backwards);
    if (action.kind === 'move') setActive(action.index);
    if (action.kind === 'choose') void choose(action.index);
    if (action.kind === 'escape') close(true);
  };

  return <>
    <button {...props} ref={trigger} id={id} type="button" role={open && search ? undefined : 'combobox'}
      aria-label={label} aria-haspopup="listbox" aria-expanded={open} aria-controls={open ? listId : undefined}
      aria-activedescendant={!search ? activeId : undefined} data-select-open={open ? '' : undefined}
      disabled={disabled} aria-busy={search?.creating || undefined} onBlur={onBlur} onKeyDown={onKeyDown}
      onClick={() => { if (open) close(true); else openMenu(); }}
      className={cn('h1-field flex h-9 w-full min-w-0 items-center gap-2 px-3 text-left text-[13px] outline-none disabled:cursor-not-allowed disabled:opacity-50', className)}>
      {selected?.color && <Dot color={selected.color} />}
      <span className="min-w-0 flex-1 truncate">{selected?.label ?? '—'}</span>
      <ChevronDown aria-hidden="true" className="size-3.5 shrink-0 text-muted-foreground" />
    </button>
    {popupHost && createPortal(
      <div ref={attach} data-select-open {...{ inert: open ? undefined : '' }} className={cn('h1-popup z-[60] flex max-h-72 flex-col overflow-hidden p-1.5',
        popupHost === popupHost.ownerDocument.body ? 'fixed' : 'absolute', popupClassName)}>
        {search && <div className="mb-1 flex shrink-0 items-center gap-2 border-b px-2 pb-1.5 pt-0.5">
          <Search aria-hidden="true" className="size-3.5 shrink-0 text-muted-foreground" />
          <input ref={input} role="combobox" aria-label={search.placeholder} aria-expanded="true" aria-autocomplete="list"
            aria-controls={listId} aria-activedescendant={activeId} tabIndex={-1} value={query} placeholder={search.placeholder}
            readOnly={search.creating} onBlur={onBlur} onKeyDown={onKeyDown}
            onCompositionStart={() => { composing.current = true; }} onCompositionEnd={() => { composing.current = false; }}
            onChange={event => { setQuery(event.target.value); setActive(undefined); setError(''); }}
            className="h-7 min-w-0 flex-1 select-text bg-transparent text-[13px] outline-none placeholder:text-muted-foreground" />
        </div>}
        <div id={listId} role="listbox" aria-label={label} className="flex min-h-0 flex-col" aria-busy={search?.creating || undefined}>
          <div ref={list} className="min-h-0 overflow-y-auto overscroll-contain">
            {filtered.map((option, index) => <div key={option.value} id={optionId(index)} role="option" aria-selected={option.value === value} data-active={index === activeIndex || undefined}
              aria-disabled={search?.creating || undefined} onMouseDown={event => event.preventDefault()} onClick={() => { void choose(index); }}
              className="choice-row flex min-h-8 cursor-default items-center gap-2 rounded-md px-2.5 py-1.5 text-[13px]">
              {option.color && <Dot color={option.color} />}
              <span className="min-w-0 flex-1 truncate">{option.label}</span>
              {option.count != null && <span className="tabular-nums text-[11px] text-muted-foreground">{option.count}<span className="sr-only"> 个项目</span></span>}
              {option.value === value && <Check aria-hidden="true" className="size-3.5 shrink-0" />}
            </div>)}
          </div>
          {search && <div className="mt-1 shrink-0 border-t pt-1">
            <div id={createIndex >= 0 ? optionId(createIndex) : undefined} role="option" aria-selected="false" data-active={(createIndex >= 0 && createIndex === activeIndex) || undefined}
              aria-disabled={search.creating || exact >= 0} onMouseDown={event => event.preventDefault()}
              onClick={() => { if (exact >= 0) setError('已有同名分组'); else void choose(createIndex); }}
              className={cn('choice-row flex min-h-8 cursor-default items-center justify-between gap-2 rounded-md px-2.5 py-1.5 text-[13px] font-medium',
                exact >= 0 && 'text-muted-foreground')}>
              <span className="truncate">{search.creating ? '创建中…' : name && exact < 0 ? `＋ 新建分组「${name}」` : '＋ 新建分组'}</span>
              {search.creating ? <Loader2 aria-hidden="true" className="size-3.5 shrink-0 animate-spin" /> :
                <kbd aria-hidden="true" className="rounded border bg-background px-1 text-[10px] text-muted-foreground">↵</kbd>}
            </div>
          </div>}
        </div>
        {error && <p role="alert" className="shrink-0 break-words px-2.5 py-1 text-xs text-destructive">{error}</p>}
      </div>, popupHost)}
  </>;
}
