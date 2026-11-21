import assert from 'node:assert/strict';
import { test } from 'node:test';
import { filterSidebarProjects, savedSidebarFilter, focusRailSections, fuzzyMatch, matchesSidebarFilter, moveProjectOrder, projectInitial, searchCreationActions, searchProjects, sidebarNavigation, sidebarSections, substringMatch } from '../src/lib/sidebarModel.ts';
import { sidebarContextPosition, sidebarThumb } from '../src/lib/sidebarLayout.ts';
import pinyin from 'tiny-pinyin';

const project = (id, extra = {}) => ({ id, name: id, path: '~/work/' + id, port: null, groupId: null, pinned: false, archived: false, ...extra });
const groups = [{ id: 'work', name: '工作', sortOrder: 2, collapsed: false }, { id: 'personal', name: '个人', sortOrder: 1, collapsed: true }];

test('palette finds project names and creation actions by full pinyin, initials and mixed text', () => {
  const chinese = project('docs', { displayName: '文档 API 工具' });
  const direct = project('literal', { displayName: 'wd API guide', archived: true });
  for (const query of ['wendangapi', 'wdapigj', '文档 api gj', 'wdAPI工具']) {
    const [match] = searchProjects([chinese], query);
    assert.equal(match?.project.id, 'docs', query);
    assert.deepEqual(match.name, []);
  }
  assert.deepEqual(searchProjects([chinese, direct], 'wd').map(item => item.project.id), ['literal', 'docs']);
  assert.deepEqual(searchCreationActions('tjfz').map(item => item.id), ['add-group']);
});

test('typing successive pinyin queries reuses each project spelling until its name changes', t => {
  const original = pinyin.parse;
  let conversions = 0;
  pinyin.parse = text => { conversions++; return original(text); };
  t.after(() => { pinyin.parse = original; });
  const chinese = project('work', { displayName: '账务核对' });
  for (const query of ['zh', 'zhang', 'zhangwu', 'zwhd']) assert.equal(searchProjects([chinese], query).length, 1);
  assert.equal(conversions, 1);
  chinese.displayName = '导出工具';
  assert.equal(searchProjects([chinese], 'dcgj').length, 1);
  assert.equal(conversions, 2);
});

test('Sidebar group counts include every project in a large group and keep archives separate', () => {
  const active = Array.from({ length: 11 }, (_, index) => project(`p${index}`, { groupId: 'work' }));
  const archived = project('old', { archived: true });
  const sections = sidebarSections([...active, archived], groups);
  assert.deepEqual(sections.map(section => [section.id, section.projects.length]), [
    ['personal', 0], ['work', 11], ['ungrouped', 0], ['archived', 1],
  ]);
  assert.deepEqual(sections.find(section => section.id === 'work').projects, active);
});

test('Sidebar partitions pins, ordered groups, ungrouped and archives without duplicates', () => {
  const projects = [project('a', { groupId: 'work' }), project('b', { pinned: true, groupId: 'work' }), project('c'), project('d', { groupId: 'personal' }), project('e', { pinned: true, archived: true }), project('f', { groupId: 'deleted' })];
  const sections = sidebarSections(projects, groups);
  assert.deepEqual(sections.map(section => section.id), ['pinned', 'personal', 'work', 'ungrouped', 'archived']);
  assert.deepEqual(sections.flatMap(section => section.projects).map(p => p.id), ['b', 'd', 'a', 'c', 'f', 'e']);
  assert.deepEqual(sidebarNavigation(projects, groups).map(p => p.id), ['b', 'd', 'a', 'c', 'f']);
  assert.deepEqual(groups.map(group => group.id), ['work', 'personal']);
  assert.deepEqual(sidebarSections([project('old')], []).map(s => s.name), ['项目']);
});

test('Focus rail groups running projects once and keeps the selected archived project reachable', () => {
  const projects = [project('a', { groupId: 'work' }), project('b', { pinned: true }), project('c'),
    project('d', { groupId: 'personal' }), project('e', { archived: true }), project('f', { archived: true })];
  const summary = (selectedId) => focusRailSections(projects, groups, { b: 'starting', c: 'running', a: 'error' }, selectedId)
    .map(section => [section.id, section.projects.map(p => p.id)]);
  assert.deepEqual(summary('e'), [['archived', ['e']], ['running', ['b', 'c']], ['personal', ['d']], ['work', ['a']]]);
  assert.deepEqual(summary('a'), [['running', ['b', 'c']], ['personal', ['d']], ['work', ['a']]]);
});

test('Cross-group reorder keeps every project, places empty-group drops and does not mutate source', () => {
  const projects = [project('a', { groupId: 'work' }), project('x'), project('b', { groupId: 'work' }), project('c')];
  assert.deepEqual(moveProjectOrder(projects, 'x', 'work', 'b'), ['a', 'x', 'b', 'c']);
  assert.deepEqual(moveProjectOrder(projects, 'a', null, null), ['x', 'b', 'c', 'a']);
  assert.deepEqual(moveProjectOrder(projects, 'a', 'empty', null), ['x', 'b', 'c', 'a']);
  assert.deepEqual(moveProjectOrder(projects, 'missing', null, null), ['a', 'x', 'b', 'c']);
  assert.deepEqual(projects.map(p => p.id), ['a', 'x', 'b', 'c']);
});

test('Fuzzy search ranks contiguous names first and matches paths, ports, aliases and archived projects', () => {
  const projects = [project('admin-console'), project('archived', { displayName: 'admin-legacy', archived: true }), project('shop-admin'), project('alpha-data-model'), project('plain', { path: '~/admin/项目', port: 5198 }), project('real-name', { displayName: '别名' })];
  assert.deepEqual(searchProjects(projects, ' adm ').map(m => m.project.id), ['admin-console', 'shop-admin', 'alpha-data-model', 'plain', 'archived']);
  assert.deepEqual(searchProjects(projects, '5198').map(m => m.project.id), ['plain']);
  assert.equal(searchProjects(projects, 'realname')[0].project.id, 'real-name');
  assert.deepEqual(searchProjects(projects, 'realname')[0].original, [0, 1, 2, 3, 5, 6, 7, 8]);
  assert.equal(searchProjects(projects, 'zzz').length, 0);
  assert.equal(searchProjects(projects, '').length, projects.length);
  assert.deepEqual(fuzzyMatch('A𠀀中-项目', '𠀀项')?.indices, [1, 4]);
  assert.deepEqual(fuzzyMatch('admin-console', 'ac')?.indices, [0, 6]);
});

test('adm cannot match scattered letters in mobile-web path; path hits are contiguous', () => {
  const mobile = project('mobile-web', { path: '~/Software/apphub/.codex-runs/20261117-sidebar/ui/fixtures/mobile-web' });
  assert.deepEqual(searchProjects([mobile], 'adm'), []);
  assert.equal(substringMatch(mobile.path, 'adm'), null);
  const match = searchProjects([mobile], 'MOBILE')[0];
  assert.equal(Array.from(mobile.path).slice(match.path[0], match.path.at(-1) + 1).join(''), 'mobile');
  assert.deepEqual(match.path, Array.from({ length: 6 }, (_, i) => match.path[0] + i));
});

test('Search ranks name prefix, name substring, name subsequence, then basename and other path substrings', () => {
  const projects = [
    project('parent', { path: '/adm/tools/plain' }),
    project('basename', { path: '/tools/admin-folder' }),
    project('alpha-data-model', { path: '/tmp/x' }),
    project('shop-admin', { path: '/tmp/y' }),
    project('admin-console', { path: '/tmp/z' }),
    project('alias', { displayName: 'Admin 别名', path: '/tmp/a' }),
  ];
  assert.deepEqual(searchProjects(projects, 'adm').map(match => match.project.id), ['admin-console', 'alias', 'shop-admin', 'alpha-data-model', 'basename', 'parent']);
  const longName = project('x'.repeat(600) + 'adm', { path: '/tmp/long' });
  assert.equal(searchProjects([projects[1], longName], 'adm')[0].project.id, longName.id);
});

test('Path matching prefers its final segment, preserves spaces and supports case folding and Unicode indices', () => {
  const path = '/ADM/work/admin-console';
  const match = substringMatch(path, 'aDm', true);
  assert.equal(Array.from(path).slice(match.indices[0], match.indices.at(-1) + 1).join(''), 'adm');
  assert.ok(match.lastSegment);
  assert.equal(substringMatch('/my app/site', 'myapp'), null);
  assert.deepEqual(substringMatch('𠀀/项目 Demo/', '项目 demo', true)?.indices, [2, 3, 4, 5, 6, 7, 8]);
  assert.deepEqual(substringMatch('İ/ADMIN', 'admin', true)?.indices, [2, 3, 4, 5, 6]);
  assert.ok(substringMatch('C:\\work\\ADMIN\\', 'adm', true)?.lastSegment);
});

test('Archived matches retain labels in data and sort after all active matches with stable ties', () => {
  const projects = [project('admin-old', { archived: true }), project('shop-admin'), project('admin-current')];
  assert.deepEqual(searchProjects(projects, 'adm').map(match => match.project.id), ['admin-current', 'shop-admin', 'admin-old']);
  assert.deepEqual(searchProjects(projects, '').map(match => match.project.id), ['shop-admin', 'admin-current', 'admin-old']);
  const tied = [project('a', { displayName: 'Admin' }), project('b', { displayName: 'Admin' })];
  assert.deepEqual(searchProjects(tied, 'adm').map(match => match.project.id), ['a', 'b']);
});

test('Creation actions support Chinese/English keywords, partial queries and literal highlight ranges', () => {
  for (const query of ['', '添加', '新建', 'ADD', ' new ', 'ad']) assert.deepEqual(searchCreationActions(query).map(action => action.id), ['add-project', 'add-group']);
  assert.deepEqual(searchCreationActions('new group').map(action => action.id), ['add-group']);
  assert.deepEqual(searchCreationActions('添加项目').map(action => action.id), ['add-project']);
  assert.deepEqual(searchCreationActions('adm'), []);
  assert.deepEqual(searchCreationActions('添加').map(action => action.indices), [[0, 1], [0, 1]]);
  assert.deepEqual(searchCreationActions('new').map(action => action.indices), [[], []]);
});

test('Monochrome project initials preserve Unicode and ignore leading whitespace', () => {
  assert.equal(projectInitial(' AppHub '), 'A');
  assert.equal(projectInitial('blog-site'), 'B');
  assert.equal(projectInitial('𠀀项目'), '𠀀');
  assert.equal(projectInitial('e\u0301diteur'), 'É');
  assert.equal(projectInitial('  '), '?');
});

test('Filters distinguish running from starting and preserve auxiliary filters', () => {
  const p = project('a');
  const attention = new Set(['a']);
  const changes = new Set(['b']);
  assert.ok(matchesSidebarFilter(p, 'all', {}, attention, changes));
  assert.ok(matchesSidebarFilter(p, 'running', { a: 'running' }, attention, changes));
  assert.equal(matchesSidebarFilter(p, 'running', { a: 'starting' }, attention, changes), false);
  assert.ok(matchesSidebarFilter(p, 'attention', {}, attention, changes));
  assert.equal(matchesSidebarFilter(p, 'changes', {}, attention, changes), false);
  assert.equal(matchesSidebarFilter(p, 'running', { a: 'error' }, attention, changes), false);
});

test('Sidebar thumb is absent for short content and follows the full scroll range', () => {
  assert.equal(sidebarThumb(400, 400, 0), undefined);
  assert.equal(sidebarThumb(400, 399, 0), undefined);
  assert.equal(sidebarThumb(0, 100, 0), undefined);
  assert.deepEqual(sidebarThumb(400, 800, -12), { height: 200, top: 0, scrollPerPixel: 2 });
  assert.deepEqual(sidebarThumb(400, 800, 200), { height: 200, top: 100, scrollPerPixel: 2 });
  assert.deepEqual(sidebarThumb(400, 800, 430), { height: 200, top: 200, scrollPerPixel: 2 });
});

test('Minimum sidebar thumb size maps back to the real scroll distance when dragged', () => {
  const thumb = sidebarThumb(400, 8000, 7600);
  assert.equal(thumb.height, 32);
  assert.equal(thumb.top, 368);
  assert.ok(Math.abs(thumb.top * thumb.scrollPerPixel - 7600) < 1e-9);
});

test('Sidebar context menu opens at the cursor and flips inside both window edges', () => {
  assert.deepEqual(sidebarContextPosition(100, 120, 180, 210, 850, 600), { left: 100, top: 120 });
  assert.deepEqual(sidebarContextPosition(840, 570, 180, 210, 850, 600), { left: 660, top: 360 });
  assert.deepEqual(sidebarContextPosition(0, 0, 180, 210, 850, 600), { left: 8, top: 8 });
});

test('Sidebar filters retain group order, combine with search and produce one row per project', () => {
  const projects = [project('web', { groupId: 'work' }), project('api', { groupId: 'work', pinned: true }), project('admin')];
  const groups = [{ id: 'work', name: '工作', sortOrder: 0, collapsed: false }];
  const statuses = { web: 'running', api: 'running', admin: 'starting' };
  const list = (filter, query = '') => filterSidebarProjects(projects, groups, filter, query, statuses, new Set(), new Set()).map(p => p.id);
  assert.deepEqual(list('all'), ['api', 'web', 'admin']);
  assert.deepEqual(list('running'), ['api', 'web']);
  assert.deepEqual(list('running', ' WEB '), ['web']);
  assert.deepEqual(list('pinned'), ['api']);
  assert.deepEqual(list('pinned', 'web'), []);
  assert.equal(savedSidebarFilter('pinned'), 'pinned');
  assert.equal(savedSidebarFilter('legacy'), 'all');
});
