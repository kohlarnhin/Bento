import type { ProjectConfig, ProjectGroup, ProjectStatus } from '../types.ts';

export function projectInitial(name: string) {
  return Array.from(name.trim().normalize('NFC'))[0]?.toLocaleUpperCase() || '?';
}

export function sidebarDensity(projects: readonly Pick<ProjectConfig, 'archived'>[]): 'comfortable' | 'compact' {
  return projects.filter(project => !project.archived).length > 10 ? 'compact' : 'comfortable';
}

export type SidebarFilter = 'all' | 'running' | 'attention' | 'changes';
export function matchesSidebarFilter(project: ProjectConfig, filter: SidebarFilter, statuses: Record<string, ProjectStatus>, attention: ReadonlySet<string>, changes: ReadonlySet<string>) {
  if (filter === 'running') return ['running', 'starting'].includes(statuses[project.id]);
  if (filter === 'attention') return attention.has(project.id);
  if (filter === 'changes') return changes.has(project.id);
  return true;
}

export interface SidebarSection {
  id: string;
  name: string;
  group?: ProjectGroup;
  projects: ProjectConfig[];
}

export function sidebarSections(projects: ProjectConfig[], groups: ProjectGroup[]): SidebarSection[] {
  const active = projects.filter(project => !project.archived);
  const regular = active.filter(project => !project.pinned);
  const known = new Set(groups.map(group => group.id));
  const sections: SidebarSection[] = [];
  const pinned = active.filter(project => project.pinned);
  if (pinned.length) sections.push({ id: 'pinned', name: '置顶', projects: pinned });
  for (const group of [...groups].sort((a, b) => a.sortOrder - b.sortOrder || a.id.localeCompare(b.id))) {
    sections.push({ id: group.id, name: group.name, group, projects: regular.filter(project => project.groupId === group.id) });
  }
  // Unknown group IDs from older external writers remain reachable as ungrouped.
  sections.push({ id: 'ungrouped', name: groups.length ? '未分组' : '项目', projects: regular.filter(project => !project.groupId || !known.has(project.groupId)) });
  const archived = projects.filter(project => project.archived);
  if (archived.length) sections.push({ id: 'archived', name: '已归档', projects: archived });
  return sections;
}

export function sidebarNavigation(projects: ProjectConfig[], groups: ProjectGroup[]) {
  return sidebarSections(projects, groups).filter(section => section.id !== 'archived').flatMap(section => section.projects);
}

export function focusRailSections(projects: ProjectConfig[], groups: ProjectGroup[], statuses: Record<string, ProjectStatus>, selectedId?: string) {
  const sections = sidebarSections(projects, groups);
  const active = sections.filter(section => section.id !== 'archived');
  const running = active.flatMap(section => section.projects.filter(project =>
    statuses[project.id] === 'running' || statuses[project.id] === 'starting'));
  const runningIds = new Set(running.map(project => project.id));
  const selectedArchive = sections.find(section => section.id === 'archived')?.projects.filter(project => project.id === selectedId) ?? [];
  return [
    { id: 'archived', name: '已归档', projects: selectedArchive },
    { id: 'running', name: '运行中', projects: running },
    ...active.map(section => ({ ...section, projects: section.projects.filter(project => !runningIds.has(project.id)) })),
  ].filter(section => section.projects.length > 0);
}

export function moveProjectOrder(projects: ProjectConfig[], id: string, groupId: string | null, beforeId: string | null, afterId?: string | null) {
  const remaining = projects.filter(project => project.id !== id);
  if (!projects.some(project => project.id === id)) return projects.map(project => project.id);
  let index = beforeId ? remaining.findIndex(project => project.id === beforeId) : -1;
  if (index < 0 && afterId) {
    const after = remaining.findIndex(project => project.id === afterId);
    if (after >= 0) index = after + 1;
  }
  if (index < 0) {
    const last = remaining.map(project => project.groupId === groupId).lastIndexOf(true);
    index = last < 0 ? remaining.length : last + 1;
  }
  const ids = remaining.map(project => project.id);
  ids.splice(index, 0, id);
  return ids;
}

export interface FuzzyMatch { score: number; indices: number[] }

/** Subsequence match with word-boundary, contiguous and early-match bonuses.
 * Indices are Unicode code-point offsets, shared by the highlight renderer. */
export function fuzzyMatch(text: string, query: string): FuzzyMatch | null {
  const source = Array.from(text);
  const target = Array.from(query.trim().toLowerCase()).filter(character => !/\s/u.test(character));
  if (!target.length) return { score: 0, indices: [] };
  let best: FuzzyMatch | null = null;
  for (let start = 0; start < source.length; start++) {
    if (source[start].toLowerCase() !== target[0]) continue;
    const indices: number[] = [];
    let cursor = start;
    for (const character of target) {
      while (cursor < source.length && source[cursor].toLowerCase() !== character) cursor++;
      if (cursor === source.length) break;
      indices.push(cursor++);
    }
    if (indices.length !== target.length) continue;
    let score = 100 - indices[0] - (indices.at(-1)! - indices[0] + 1 - target.length) * 3;
    indices.forEach((index, position) => {
      if (index === 0 || /[\s/_.\-]/u.test(source[index - 1])) score += 12;
      if (position > 0 && index === indices[position - 1] + 1) score += 8;
    });
    if (!best || score > best.score) best = { score, indices };
  }
  return best;
}

/** A contiguous, case-insensitive match. Folded characters retain their original
 * code-point offsets so highlights also work with emoji and expanding case folds. */
export function substringMatch(text: string, query: string, preferLastSegment = false): (FuzzyMatch & { lastSegment: boolean }) | null {
  const target = Array.from(query.trim().toLowerCase());
  if (!target.length) return { score: 0, indices: [], lastSegment: false };
  const source = Array.from(text);
  const folded = source.flatMap((character, index) => Array.from(character.toLowerCase()).map(value => ({ value, index })));
  let end = source.length;
  while (end > 0 && /[/\\]/u.test(source[end - 1])) end--;
  let lastSegmentStart = end;
  while (lastSegmentStart > 0 && !/[/\\]/u.test(source[lastSegmentStart - 1])) lastSegmentStart--;
  let best: (FuzzyMatch & { lastSegment: boolean }) | null = null;
  for (let start = 0; start <= folded.length - target.length; start++) {
    if (!target.every((character, index) => folded[start + index].value === character)) continue;
    const indices = [...new Set(folded.slice(start, start + target.length).map(character => character.index))];
    const lastSegment = preferLastSegment && indices[0] >= lastSegmentStart && indices.at(-1)! < end;
    const score = 100 - (lastSegment ? indices[0] - lastSegmentStart : indices[0]);
    if (!best || Number(lastSegment) > Number(best.lastSegment) || (lastSegment === best.lastSegment && score > best.score)) best = { score, indices, lastSegment };
  }
  return best;
}

function nameMatch(text: string, query: string) {
  const continuous = substringMatch(text, query);
  if (continuous) return { ...continuous, rank: continuous.indices[0] === 0 ? 0 : 1 };
  const subsequence = fuzzyMatch(text, query);
  return subsequence ? { ...subsequence, rank: 2 } : null;
}

export function searchProjects(projects: ProjectConfig[], query: string) {
  return projects.flatMap((project, index) => {
    if (!query.trim()) return [{ project, name: [], original: [], path: [], port: [], rank: 0, score: 0, index }];
    const name = nameMatch(project.displayName || project.name, query);
    const original = project.displayName ? nameMatch(project.name, query) : null;
    const path = substringMatch(project.path, query, true);
    const port = substringMatch(project.port?.toString() ?? '', query);
    // Keep categories separate: a long name can never fall below a path match.
    // Port substrings follow names; basename hits precede other path substrings.
    const candidates = [name, original, port && { ...port, rank: 3 }, path && { ...path, rank: path.lastSegment ? 4 : 5 }]
      .filter((match): match is FuzzyMatch & { rank: number } => !!match)
      .sort((a, b) => a.rank - b.rank || b.score - a.score);
    const best = candidates[0];
    if (!best) return [];
    return [{ project, name: name?.indices ?? [], original: original?.indices ?? [], path: path?.indices ?? [], port: port?.indices ?? [], rank: best.rank, score: best.score, index }];
  }).sort((a, b) => Number(a.project.archived) - Number(b.project.archived) || a.rank - b.rank || b.score - a.score || a.index - b.index);
}

export function searchCreationActions(query: string) {
  const text = query.trim().toLowerCase().replace(/\s+/gu, '');
  return [
    { id: 'add-project' as const, label: '添加项目…', aliases: ['添加项目', '新建项目', 'addproject', 'newproject'] },
    { id: 'add-group' as const, label: '添加分组', aliases: ['添加分组', '新建分组', 'addgroup', 'newgroup'] },
  ].filter(action => !text || action.aliases.some(alias => alias.includes(text)))
    .map(action => ({ id: action.id, label: action.label, indices: substringMatch(action.label, query)?.indices ?? [] }));
}
