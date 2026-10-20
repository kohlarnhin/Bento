export interface TerminalImeState {
  shiftDown: boolean;
  composing: boolean;
  pendingSend: boolean;
}

/** Only Shift itself can precede WebKit's input-before-keydown IME commit. */
export function trackBareShift(shiftDown: boolean, event: Pick<KeyboardEvent, 'type' | 'key' | 'ctrlKey' | 'altKey' | 'metaKey'>): boolean {
  if (event.type === 'keyup' || event.type === 'blur') return false;
  if (event.type !== 'keydown') return shiftDown;
  return event.key === 'Shift' && !event.ctrlKey && !event.altKey && !event.metaKey;
}

/** Recover only the direct commit that xterm rejects after a bare Shift keydown. */
export function recoverShiftedImeInput(state: TerminalImeState, event: Pick<InputEvent, 'data' | 'inputType' | 'composed' | 'isComposing'>): string | undefined {
  if (state.shiftDown && !state.composing && !state.pendingSend
      && event.inputType === 'insertText' && event.composed && !event.isComposing) {
    return event.data || undefined;
  }
  return undefined;
}

/** Keyboard/IME input and terminal reports use this transform. Paste and drop bypass it. */
export function transformTypedInput(data: string, lastPrintable: string): { data: string; lastPrintable: string } {
  // Capture mouse/focus reports and DA, DSR/CPR, and mode replies separately from user input.
  const chunks = data.split(/(\x1b\[(?:<\d+;\d+;\d+[Mm]|M[\s\S]{3}|[IO]|[?>][\d;]*c|\??[\d;]+[nR]|\??\d+;\d+\$y))/g);
  let converted = '';
  for (const [index, chunk] of chunks.entries()) {
    if (index % 2 === 1) {
      converted += chunk;
    } else if (/[\x00-\x1f\x7f-\x9f]/.test(chunk)) {
      // Handled keys and other user control sequences invalidate the cursor context.
      converted += chunk;
      lastPrintable = '';
    } else {
      for (const character of chunk) {
        const typed = character === '。' && /^[0-9]$/.test(lastPrintable) ? '.' : character;
        converted += typed;
        lastPrintable = typed;
      }
    }
  }
  return { data: converted, lastPrintable };
}

/** iTerm-style shell paths: retain Unicode and backslash-escape shell metacharacters. */
export function escapeShellPath(path: string): string {
  return path.replace(/[^\p{L}\p{N}_./:@%+=,-]/gu, (character) => {
    if (character === '\n') return "$'\\n'";
    if (character === '\r') return "$'\\r'";
    return `\\${character}`;
  });
}

export function formatDroppedPaths(paths: string[]): string {
  const absolutePaths = paths.filter((path) => path.startsWith('/'));
  return absolutePaths.length ? `${absolutePaths.map(escapeShellPath).join(' ')} ` : '';
}
