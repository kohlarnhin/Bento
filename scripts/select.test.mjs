import assert from 'node:assert/strict';
import { test } from 'node:test';
import { selectKeyAction } from '../src/lib/selectKeyboard.ts';

const action = (key, overrides = {}) => selectKeyAction({ key, open: true, editable: false, activeIndex: 1, optionCount: 4, ...overrides });

test('Closed Select opens with arrows, Enter or Space', () => {
  assert.deepEqual(action('ArrowUp', { open: false }), { kind: 'open', backwards: true });
  for (const key of ['ArrowDown', 'Enter', ' ']) assert.deepEqual(action(key, { open: false }), { kind: 'open', backwards: false });
  assert.equal(action('Escape', { open: false }), undefined);
});

test('Arrows move the active listbox option and stay within the list', () => {
  assert.deepEqual(action('ArrowDown'), { kind: 'move', index: 2 });
  assert.deepEqual(action('ArrowUp'), { kind: 'move', index: 0 });
  assert.deepEqual(action('ArrowUp', { activeIndex: 0 }), { kind: 'move', index: 0 });
  assert.deepEqual(action('ArrowDown', { activeIndex: 3 }), { kind: 'move', index: 3 });
  assert.deepEqual(action('ArrowUp', { activeIndex: -1 }), { kind: 'move', index: 3 });
  assert.equal(action('ArrowDown', { optionCount: 0 }), undefined);
});

test('Enter chooses the active option, Escape dismisses and Tab leaves', () => {
  assert.deepEqual(action('Enter'), { kind: 'choose', index: 1 });
  assert.deepEqual(action(' ', { editable: true }), undefined);
  assert.deepEqual(action('Escape'), { kind: 'escape' });
  assert.deepEqual(action('Tab'), { kind: 'tab' });
});

test('An IME confirmation does not choose an option, and normal Enter resumes after composition', () => {
  const editable = { editable: true, activeIndex: 3 };
  assert.equal(action('Enter', { ...editable, composing: true }), undefined);
  assert.equal(action('Enter', { ...editable, keyCode: 229 }), undefined);
  assert.equal(action('ArrowDown', { ...editable, composing: true }), undefined);
  assert.deepEqual(action('Enter', { ...editable, composing: false }), { kind: 'choose', index: 3 });
});
