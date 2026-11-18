import { useEffect, useMemo, useRef, useState } from 'react';
import { FolderPlus, Play, Plus, Search, Square, Terminal } from 'lucide-react';
import type { ProjectConfig, ProjectStatus } from '../types';
import { searchCreationActions, searchProjects } from '../lib/sidebarModel';
import { cn } from '../lib/utils';
import { ProjectAvatar } from './ProjectAvatar';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from './ui/dialog';
import { Input } from './ui/input';

function Highlight({ text, indices }: { text: string; indices: number[] }) {
  const matched = new Set(indices);
  const characters = Array.from(text);
  const runs: { start: number; text: string; matched: boolean }[] = [];
  characters.forEach((character, index) => {
    const previous = runs.at(-1);
    if (previous && previous.matched === matched.has(index)) previous.text += character;
    else runs.push({ start: index, text: character, matched: matched.has(index) });
  });
  return <>{runs.map(run => run.matched
    ? <mark key={run.start} className="bg-transparent font-semibold text-attention">{run.text}</mark> : run.text)}</>;
}

export function QuickSwitcher({ projects, statuses, onClose, onCreate, onCreateGroup, onSelect, onToggle, onNewTerminal }: {
  projects: ProjectConfig[];
  statuses: Record<string, ProjectStatus>;
  onClose: () => void;
  onCreate: () => void;
  onCreateGroup: () => void;
  onSelect: (project: ProjectConfig) => void;
  onToggle: (project: ProjectConfig) => void;
  onNewTerminal: (project: ProjectConfig) => void;
}) {
  const [query, setQuery] = useState('');
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [targetId, setTargetId] = useState<string | null>(null);
  const composing = useRef(false);
  const list = useRef<HTMLDivElement>(null);
  const matches = useMemo(() => searchProjects(projects, query), [projects, query]);
  const creationActions = useMemo(() => searchCreationActions(query), [query]);
  const creationFirst = query.trim().length > 0 && creationActions.length > 0;
  const target = matches.find(match => match.project.id === targetId)?.project ?? matches[0]?.project;
  const running = target && ['running', 'starting'].includes(statuses[target.id]);
  const name = target?.displayName || target?.name;
  const actions = [
    ...(target && !creationFirst ? [
      { id: 'toggle', label: `${running ? '停止' : '启动'} ${name}`, icon: running ? Square : Play, key: '⌘↵', run: () => onToggle(target), indices: [] as number[] },
      { id: 'terminal', label: `在 ${name} 中新建终端`, icon: Terminal, key: '⌘T', run: () => onNewTerminal(target), indices: [] as number[] },
    ] : []),
    ...creationActions.map(action => ({ ...action, icon: action.id === 'add-project' ? Plus : FolderPlus, key: action.id === 'add-project' ? '⌘N' : '', run: action.id === 'add-project' ? onCreate : onCreateGroup })),
  ];
  const count = matches.length + actions.length;
  const projectOffset = creationFirst ? actions.length : 0;
  const actionOffset = creationFirst ? 0 : matches.length;
  const activeIndex = Math.min(selectedIndex, Math.max(0, count - 1));
  const choose = (index: number) => {
    setSelectedIndex(index);
    if (index >= projectOffset && index < projectOffset + matches.length) setTargetId(matches[index - projectOffset].project.id);
  };
  const activate = () => {
    if (activeIndex >= projectOffset && activeIndex < projectOffset + matches.length) onSelect(matches[activeIndex - projectOffset].project);
    else actions[activeIndex - actionOffset]?.run();
  };
  useEffect(() => { list.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' }); }, [activeIndex]);
  const projectResults = ((matches.length > 0 || !actions.length) && <div role="group" aria-label="项目">
          <p className="px-4 pb-1 pt-3 text-[11px] font-medium text-muted-foreground">项目 <span className="ml-1 tabular-nums opacity-60">{matches.length}</span></p>
          {!matches.length && <p role="status" className="px-4 py-8 text-center text-sm text-muted-foreground">没有匹配的项目</p>}
          {matches.map((match, index) => {
            const project = match.project;
            const status = statuses[project.id] || 'stopped';
            const optionIndex = projectOffset + index;
            return <button key={project.id} role="option" tabIndex={-1} id={`quick-option-${optionIndex}`} aria-selected={activeIndex === optionIndex}
              data-quick-project={project.id} className={cn('mx-1.5 flex w-[calc(100%-0.75rem)] items-center gap-3 rounded-lg px-3 py-2 text-left outline-none', activeIndex === optionIndex && 'bg-attention/10')}
              onMouseDown={event => event.preventDefault()} onMouseMove={() => { if (activeIndex !== optionIndex) choose(optionIndex); }} onClick={() => onSelect(project)}>
              <ProjectAvatar project={project} status={status} className="size-7 rounded-[7px] text-xs" />
              <span className="min-w-0 flex-1">
                <span className="flex items-center gap-2 text-[13px]"><span className="truncate"><Highlight text={project.displayName || project.name} indices={match.name} /></span>
                  {project.displayName && project.displayName !== project.name && <span className="max-w-[45%] truncate font-mono text-[11px] text-muted-foreground" data-quick-original-name><Highlight text={project.name} indices={match.original} /></span>}
                  {project.archived && <span className="shrink-0 rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">已归档</span>}</span>
                <span className="mt-0.5 flex min-w-0 gap-1 font-mono text-[11px] text-muted-foreground/75" title={project.path}>
                  <span className="min-w-0 truncate"><Highlight text={project.path} indices={match.path} /></span>
                  {project.port != null && <span className="shrink-0">· <Highlight text={String(project.port)} indices={match.port} /></span>}
                  <span className="shrink-0">{status === 'running' ? '· 运行中' : status === 'starting' ? '· 启动中' : status === 'error' ? '· 运行异常' : ''}</span>
                </span>
              </span>
              {activeIndex === optionIndex && <kbd className="shrink-0 rounded border bg-background px-1.5 py-0.5 text-[10px] text-muted-foreground">↵ 打开</kbd>}
            </button>;
          })}
        </div>);
  const actionResults = (actions.length > 0 && <div role="group" aria-label="操作">
          <p className="px-4 pb-1 pt-3 text-[11px] font-medium text-muted-foreground">操作</p>
          {actions.map((action, i) => {
            const index = actionOffset + i;
            return <button key={action.id} id={`quick-option-${index}`} role="option" aria-selected={activeIndex === index} tabIndex={-1}
              data-quick-action={action.id} onMouseDown={event => event.preventDefault()} onMouseMove={() => { if (activeIndex !== index) choose(index); }} onClick={action.run}
              className={cn('mx-1.5 flex w-[calc(100%-0.75rem)] items-center gap-3 rounded-lg px-3 py-2 text-left text-[13px] outline-none', activeIndex === index && 'bg-attention/10')}>
              <span className="grid size-7 shrink-0 place-items-center rounded-[7px] bg-muted text-muted-foreground"><action.icon className="size-3.5" /></span>
              <span className="min-w-0 flex-1 truncate"><Highlight text={action.label} indices={action.indices} /></span>{action.key && <kbd className="rounded border bg-background px-1.5 py-0.5 text-[10px] text-muted-foreground">{action.key}</kbd>}
            </button>;
          })}
        </div>);
  return <Dialog open onOpenChange={open => { if (!open) onClose(); }}>
    <DialogContent hideClose overlayClassName="bg-foreground/25 backdrop-blur-[3px]"
      className="max-w-[600px] gap-0 overflow-hidden rounded-xl border-border/70 p-0 shadow-2xl" data-quick-switcher
      onEscapeKeyDown={event => { if (composing.current || event.isComposing || event.keyCode === 229) event.preventDefault(); }}
      onKeyDown={event => {
        if (composing.current || event.nativeEvent.isComposing || event.keyCode === 229) return;
        const command = event.metaKey && !event.altKey && !event.ctrlKey && !event.shiftKey;
        if (command && event.key.toLowerCase() === 'n') {
          event.preventDefault(); event.stopPropagation();
          if (!event.repeat) onCreate();
        } else if (command && (event.key === 'Enter' || event.key.toLowerCase() === 't')) {
          event.preventDefault(); event.stopPropagation();
          if (!event.repeat && target) (event.key === 'Enter' ? onToggle : onNewTerminal)(target);
        } else if (!event.metaKey && !event.ctrlKey && !event.altKey && ['ArrowUp', 'ArrowDown'].includes(event.key)) {
          event.preventDefault(); event.stopPropagation();
          if (count) choose((activeIndex + (event.key === 'ArrowDown' ? 1 : -1) + count) % count);
        } else if (event.key === 'Enter' && !event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey) {
          event.preventDefault(); event.stopPropagation();
          if (!event.repeat) activate();
        }
      }}>
      <DialogHeader className="sr-only"><DialogTitle>快速切换项目</DialogTitle><DialogDescription>搜索名称、路径或端口，使用方向键选择项目或操作。</DialogDescription></DialogHeader>
      <div className="flex items-center gap-3 border-b px-4 py-3">
        <Search className="size-[18px] shrink-0 text-muted-foreground" />
        <Input autoFocus role="combobox" aria-label="快速切换项目" aria-expanded="true" aria-autocomplete="list" aria-controls="quick-switcher-results"
          aria-activedescendant={count ? `quick-option-${activeIndex}` : undefined} data-quick-switcher-input
          placeholder="搜索项目名称、路径或端口…" value={query}
          className="h-8 min-w-0 border-0 bg-transparent px-0 text-base shadow-none focus-visible:ring-0"
          onCompositionStart={() => { composing.current = true; }} onCompositionEnd={() => { composing.current = false; }}
          onChange={event => { setQuery(event.target.value); setSelectedIndex(0); setTargetId(null); }} />
        <button onClick={onClose} className="rounded border px-1.5 py-0.5 text-[10px] text-muted-foreground hover:bg-accent" aria-label="关闭快速切换">esc</button>
      </div>
      <div ref={list} id="quick-switcher-results" role="listbox" aria-label="项目与操作" className="max-h-[min(58vh,460px)] overflow-y-auto pb-2">
        {creationFirst ? <>{actionResults}{projectResults}</> : <>{projectResults}{actionResults}</>}
      </div>
      <footer className="flex flex-wrap gap-x-5 gap-y-1 border-t bg-muted/40 px-4 py-3 text-[10px] text-muted-foreground">
        <span>↑↓ 选择</span><span>↵ 打开</span><span>⌘↵ 启动 / 停止</span><span>⌘T 新终端</span><span>⌘N 添加</span><span>Esc 关闭</span>
      </footer>
    </DialogContent>
  </Dialog>;
}
