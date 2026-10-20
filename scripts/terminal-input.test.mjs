import assert from 'node:assert/strict';
import { test } from 'node:test';
import { recoverShiftedImeInput, trackBareShift, transformTypedInput } from '../src/lib/terminalInput.ts';

const state = () => ({ shiftDown: false, composing: false, pendingSend: false });
const input = (data, overrides = {}) => ({ data, inputType: 'insertText', composed: true, isComposing: false, ...overrides });
const key = (ime, type, key, overrides = {}) => {
  ime.shiftDown = trackBareShift(ime.shiftDown, { type, key, ctrlKey: false, altKey: false, metaKey: false, ...overrides });
};

for (const punctuation of ['！', '？']) {
  test('Shift → input(' + punctuation + ') → keydown 229 recovers only the first press', () => {
    const ime = state();
    key(ime, 'keydown', 'Shift');
    assert.equal(recoverShiftedImeInput(ime, input(punctuation)), punctuation);
    key(ime, 'keydown', punctuation, { keyCode: 229 });
    key(ime, 'keyup', punctuation);
    // xterm accepts the second input after keyup; the wrapper must leave it alone.
    assert.equal(recoverShiftedImeInput(ime, input(punctuation)), undefined);
    key(ime, 'keydown', punctuation, { keyCode: 229 });
    assert.equal(ime.shiftDown, false);
  });

  test('Releasing and pressing Shift again recovers the next ' + punctuation, () => {
    const ime = state();
    for (let press = 0; press < 2; press++) {
      key(ime, 'keydown', 'Shift');
      assert.equal(recoverShiftedImeInput(ime, input(punctuation)), punctuation);
      key(ime, 'keydown', punctuation, { keyCode: 229 });
      key(ime, 'keyup', punctuation);
      key(ime, 'keyup', 'Shift');
    }
  });

  test('keydown 229 before input(' + punctuation + ') leaves delivery to xterm', () => {
    const ime = state();
    key(ime, 'keydown', 'Shift');
    key(ime, 'keydown', punctuation, { keyCode: 229, isComposing: true });
    assert.equal(recoverShiftedImeInput(ime, input(punctuation)), undefined);
  });
}

test('English Shift+! keypress/input stays on xterm after the character keydown', () => {
  const ime = state();
  key(ime, 'keydown', 'Shift');
  key(ime, 'keydown', '!');
  key(ime, 'keypress', '!');
  assert.equal(recoverShiftedImeInput(ime, input('!')), undefined);
});

test('keypress preserves Shift tracking, any keyup or blur clears it', () => {
  const ime = state();
  key(ime, 'keydown', 'Shift');
  key(ime, 'keypress', 'Shift');
  assert.equal(ime.shiftDown, true);
  key(ime, 'keyup', 'Unidentified');
  assert.equal(recoverShiftedImeInput(ime, input('！')), undefined);
  key(ime, 'keydown', 'Shift');
  key(ime, 'blur', '');
  assert.equal(recoverShiftedImeInput(ime, input('！')), undefined);
});

test('Shift with another modifier never arms recovery', () => {
  for (const modifier of ['ctrlKey', 'altKey', 'metaKey']) {
    const ime = state();
    key(ime, 'keydown', 'Shift', { [modifier]: true });
    assert.equal(recoverShiftedImeInput(ime, input('！')), undefined);
  }
});

test('Pinyin ni → 你 preedit and commit remain owned by xterm', () => {
  const ime = state();
  key(ime, 'keydown', 'Shift');
  ime.composing = true;
  assert.equal(recoverShiftedImeInput(ime, input('ni', { inputType: 'insertCompositionText', isComposing: true })), undefined);
  // Some IMEs report insertText with isComposing=false before compositionend.
  assert.equal(recoverShiftedImeInput(ime, input('你')), undefined);
  ime.composing = false;
  ime.pendingSend = true;
  assert.equal(recoverShiftedImeInput(ime, input('你')), undefined);
});

test('Pending composition send followed by Shift+！ cannot add a duplicate', () => {
  const ime = state();
  ime.pendingSend = true;
  key(ime, 'keydown', 'Shift');
  assert.equal(recoverShiftedImeInput(ime, input('！')), undefined);
  key(ime, 'keydown', '！', { keyCode: 229 });
  ime.pendingSend = false;
  assert.equal(recoverShiftedImeInput(ime, input('！')), undefined);
});

test('Recovery resumes after the pending send finishes', () => {
  const ime = state();
  key(ime, 'keydown', 'Shift');
  ime.pendingSend = true;
  assert.equal(recoverShiftedImeInput(ime, input('！')), undefined);
  ime.pendingSend = false;
  assert.equal(recoverShiftedImeInput(ime, input('！')), '！');
});

test('Only nonempty composed insertText outside event composition is recovered', () => {
  const ime = state();
  key(ime, 'keydown', 'Shift');
  for (const event of [
    input(''), input(null), input('！', { composed: false }), input('！', { isComposing: true }),
    input('！', { inputType: 'insertCompositionText' }), input('1。', { inputType: 'insertFromPaste' }),
    input('/tmp/路径', { inputType: 'insertFromDrop' }), input(null, { inputType: 'deleteContentBackward' }),
  ]) assert.equal(recoverShiftedImeInput(ime, event), undefined);
});

test('Recovered typed data preserves the smart period rule after a digit', () => {
  const ime = state();
  key(ime, 'keydown', 'Shift');
  const digit = transformTypedInput('1', '');
  const recovered = recoverShiftedImeInput(ime, input('。'));
  assert.equal(recovered, '。');
  assert.deepEqual(transformTypedInput(recovered, digit.lastPrintable), { data: '.', lastPrintable: '.' });
  assert.deepEqual(transformTypedInput('！', '1'), { data: '！', lastPrintable: '！' });
  assert.deepEqual(transformTypedInput('？', '1'), { data: '？', lastPrintable: '？' });
});
