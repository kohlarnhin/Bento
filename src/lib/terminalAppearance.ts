import type { ITheme } from '@xterm/xterm';

export const DEFAULT_TERMINAL_FONT_SIZE = 14;
export const TERMINAL_FALLBACK_FONT_FAMILY = 'ui-monospace, Menlo, monospace';
export const TERMINAL_FONT_FAMILY = '"JetBrains Mono", ' + TERMINAL_FALLBACK_FONT_FAMILY;
export const TERMINAL_LINE_HEIGHT = 1.25;

// Apple system colors and lighter ANSI variants share the terminal's dark surface.
export const TERMINAL_THEME = {
  background: '#1C1C1E', foreground: '#E5E5E7',
  cursor: '#E5E5E7', cursorAccent: '#1C1C1E', selectionBackground: '#FFFFFF30',
  black: '#2B2D30', red: '#FF817A', green: '#34C759', yellow: '#FFD60A',
  blue: '#409CFF', magenta: '#DA8FFF', cyan: '#70D7FF', white: '#C7C7CC',
  brightBlack: '#8E8E93', brightRed: '#FF9F99', brightGreen: '#66D681', brightYellow: '#FFE35C',
  brightBlue: '#70B5FF', brightMagenta: '#E5AAFF', brightCyan: '#9AE3FF', brightWhite: '#F5F5F7',
  scrollbarSliderBackground: '#4B4B4E',
  scrollbarSliderHoverBackground: '#626265',
  scrollbarSliderActiveBackground: '#626265',
} satisfies ITheme;
