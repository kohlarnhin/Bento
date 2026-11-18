import { forwardRef, useEffect, useImperativeHandle, useLayoutEffect, useRef, useState } from 'react';
import { ArrowDown, ArrowUp, X } from 'lucide-react';
import type { SearchAddon, ISearchResultChangeEvent } from '@xterm/addon-search';
import type { Terminal } from '@xterm/xterm';
import Button from './ui/button';
import { Input } from './ui/input';

let lastQuery = '';
const decorations = {
  matchBackground: '#444532', matchBorder: '#827443', matchOverviewRuler: '#827443',
  activeMatchBackground: '#5C4422', activeMatchBorder: '#DDB56F', activeMatchColorOverviewRuler: '#DDB56F',
};

export interface TerminalFindControls {
  navigate: (previous?: boolean) => void;
}

export const TerminalFindBar = forwardRef<TerminalFindControls, {
  search: SearchAddon;
  terminal: Terminal;
  focusRequest: number;
  onClose: () => void;
}>(function TerminalFindBar({ search, terminal, focusRequest, onClose }, ref) {
  const input = useRef<HTMLInputElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout>>();
  const [query, setQuery] = useState(lastQuery);
  const [results, setResults] = useState<ISearchResultChangeEvent>({ resultIndex: -1, resultCount: 0 });

  const find = (previous = false, incremental = false) => {
    clearTimeout(timer.current);
    if (!query) { search.clearDecorations(); terminal.clearSelection(); setResults({ resultIndex: -1, resultCount: 0 }); return; }
    const options = { caseSensitive: false, incremental, decorations };
    if (previous) search.findPrevious(query, options);
    else search.findNext(query, options);
  };
  useImperativeHandle(ref, () => ({ navigate: previous => find(previous) }));
  useLayoutEffect(() => { input.current?.focus(); input.current?.select(); }, [focusRequest]);
  useEffect(() => {
    const subscription = search.onDidChangeResults(setResults);
    return () => { subscription.dispose(); clearTimeout(timer.current); search.clearDecorations(); terminal.clearSelection(); };
  }, [search, terminal]);
  useEffect(() => {
    timer.current = setTimeout(() => find(false, true), 80);
    return () => clearTimeout(timer.current);
  }, [query, search]);

  return <div role="search" aria-label="查找终端" data-terminal-find className="absolute right-3 top-2 z-20 flex max-w-[calc(100%-24px)] items-center gap-1 rounded-md border border-terminal-border bg-terminal p-1.5 text-terminal-foreground shadow-lg"
    onKeyDown={event => {
      if (event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229) return;
      if (event.key === 'Enter') { event.preventDefault(); find(event.shiftKey); }
    }}>
    <Input ref={input} value={query} placeholder="查找" aria-label="查找" autoComplete="off" spellCheck={false}
      className="h-7 w-40 border-terminal-border bg-terminal px-2 text-xs text-terminal-foreground shadow-none placeholder:text-terminal-muted focus-visible:border-terminal-muted"
      onChange={event => { lastQuery = event.target.value; setQuery(lastQuery); }} />
    <span data-terminal-find-count role="status" className="min-w-12 shrink-0 px-1 text-center text-[11px] tabular-nums text-terminal-muted">
      {!query ? '' : results.resultCount === 0 ? '无结果' : `${results.resultIndex + 1}/${results.resultCount}`}
    </span>
    <Button size="icon-sm" variant="terminal" aria-label="上一个匹配" title="上一个匹配（Shift+Enter）" className="border-transparent" disabled={!query || !results.resultCount} onClick={() => find(true)}><ArrowUp /></Button>
    <Button size="icon-sm" variant="terminal" aria-label="下一个匹配" title="下一个匹配（Enter）" className="border-transparent" disabled={!query || !results.resultCount} onClick={() => find()}><ArrowDown /></Button>
    <Button size="icon-sm" variant="terminal" aria-label="关闭查找" title="关闭查找（Esc）" className="border-transparent" onClick={onClose}><X /></Button>
  </div>;
});
