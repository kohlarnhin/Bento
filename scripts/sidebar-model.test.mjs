import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fuzzyMatch, matchesSidebarFilter, moveProjectOrder, projectAvatar, searchCreationActions, searchProjects, sidebarDensity, sidebarNavigation, sidebarSections, substringMatch } from '../src/lib/sidebarModel.ts';

const project = (id, extra = {}) => ({ id, name: id, path: '~/work/' + id, port: null, groupId: null, pinned: false, archived: false, ...extra });
const groups = [{ id: 'work', name: '工作', sortOrder: 2, collapsed: false }, { id: 'personal', name: '个人', sortOrder: 1, collapsed: true }];

test('Sidebar density counts only unarchived projects and changes after 10', () => {
  const ten = Array.from({ length: 10 }, (_, index) => project(`p${index}`));
  const archived = project('old', { archived: true });
  assert.equal(sidebarDensity([]), 'comfortable');
  assert.equal(sidebarDensity(ten), 'comfortable');
  assert.equal(sidebarDensity([...ten, project('extra')]), 'compact');
  assert.equal(sidebarDensity([...ten, archived]), 'comfortable');
  assert.equal(sidebarDensity([archived]), 'comfortable');
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

test('Avatar colours survive reordering and Unicode initials remain intact', () => {
  assert.deepEqual(projectAvatar(' AppHub '), projectAvatar('AppHub'));
  assert.equal(projectAvatar('𠀀项目').initial, '𠀀');
  assert.equal(projectAvatar('  ').initial, '?');
  assert.match(projectAvatar('apphub').color, /^#[a-f0-9]{6}$/);
  assert.ok(new Set(['apphub', 'blog', 'api-gateway', 'docs', 'shop-admin'].map(name => projectAvatar(name).color)).size >= 3);
});

test('Filters count projects, include starting services and combine with search upstream', () => {
  const p = project('a');
  const attention = new Set(['a']);
  const changes = new Set(['b']);
  assert.ok(matchesSidebarFilter(p, 'all', {}, attention, changes));
  assert.ok(matchesSidebarFilter(p, 'running', { a: 'starting' }, attention, changes));
  assert.ok(matchesSidebarFilter(p, 'attention', {}, attention, changes));
  assert.equal(matchesSidebarFilter(p, 'changes', {}, attention, changes), false);
  assert.equal(matchesSidebarFilter(p, 'running', { a: 'error' }, attention, changes), false);
});
