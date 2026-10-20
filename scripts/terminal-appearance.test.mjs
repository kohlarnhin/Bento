import assert from 'node:assert/strict';
import { test } from 'node:test';
import { TERMINAL_THEME } from '../src/lib/terminalAppearance.ts';

const luminance = (hex) => {
  const rgb = hex.slice(1).match(/../g).map((channel) => {
    const value = parseInt(channel, 16) / 255;
    return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  });
  return rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722;
};

test('terminal foreground and all non-black ANSI colors meet 4.5:1 contrast', () => {
  const background = luminance(TERMINAL_THEME.background);
  for (const name of [
    'foreground', 'red', 'green', 'yellow', 'blue', 'magenta', 'cyan', 'white',
    'brightBlack', 'brightRed', 'brightGreen', 'brightYellow', 'brightBlue', 'brightMagenta', 'brightCyan', 'brightWhite',
  ]) {
    const foreground = luminance(TERMINAL_THEME[name]);
    const ratio = (Math.max(foreground, background) + 0.05) / (Math.min(foreground, background) + 0.05);
    assert.ok(ratio >= 4.5, name + ' contrast is ' + ratio.toFixed(3) + ':1');
  }
});
