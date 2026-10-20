import type { ITheme } from '@xterm/xterm';

export const DEFAULT_TERMINAL_FONT_SIZE = 14;
export const TERMINAL_FONT_FAMILY = '"JetBrains Mono", "SF Mono", Menlo, "PingFang SC", monospace';
export const TERMINAL_LINE_HEIGHT = 1.2;

// Soft IDE-style colors; every ANSI foreground except black exceeds 4.5:1 on this background.
export const TERMINAL_THEME = {
  background: '#1E1F22', foreground: '#BCBEC4',
  cursor: '#CED0D6', cursorAccent: '#1E1F22', selectionBackground: '#214283B3',
  black: '#2B2D30', red: '#D98282', green: '#9BBF88', yellow: '#C8B17A',
  blue: '#7FA2D6', magenta: '#B99ACB', cyan: '#83B8BC', white: '#A9ABB2',
  brightBlack: '#858991', brightRed: '#E59A9A', brightGreen: '#B3CF9D', brightYellow: '#DDC78F',
  brightBlue: '#9BB9E8', brightMagenta: '#CFB3DE', brightCyan: '#A1CDD0', brightWhite: '#CED0D6',
  scrollbarSliderBackground: '#4B4E55',
  scrollbarSliderHoverBackground: '#62656E',
  scrollbarSliderActiveBackground: '#62656E',
} satisfies ITheme;
