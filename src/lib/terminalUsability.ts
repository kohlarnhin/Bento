export const MIN_TERMINAL_FONT_SIZE = 12;
export const MAX_TERMINAL_FONT_SIZE = 20;

export function clampTerminalFontSize(size: number): number {
  return Math.max(MIN_TERMINAL_FONT_SIZE, Math.min(MAX_TERMINAL_FONT_SIZE, size));
}

export function normalizeTerminalTitle(raw: string): string {
  const title = raw.replace(/[\r\n\t]/g, ' ').replace(/[\p{Cc}\p{Cf}]/gu, '')
    .replace(/\s+/gu, ' ').trim().replace(/^[^\p{L}\p{N}~/]+/u, '');
  // Check before stripping punctuation so a shell's path cannot become a title.
  const path = title.replace(/^[^\s:@]+@[^\s:]+:/u, '');
  if (/^[~/]/u.test(path)) return '';
  return [...title.replace(/^[^\p{L}\p{N}]+/u, '')].slice(0, 80).join('');
}

// Fixed trailing windows: continuous changes cannot starve the final title.
export function createTitlePublisher(publish: (title: string) => void) {
  let published = '';
  let pending = '';
  let timer: ReturnType<typeof setTimeout> | undefined;
  return {
    update(raw: string) {
      pending = normalizeTerminalTitle(raw);
      if (pending === published) {
        clearTimeout(timer);
        timer = undefined;
      } else if (timer === undefined) {
        timer = setTimeout(() => {
          timer = undefined;
          published = pending;
          publish(published);
        }, 500);
      }
    },
    reset() {
      clearTimeout(timer);
      timer = undefined;
      published = pending = '';
    },
  };
}

interface AttentionTarget {
  projectId: string;
  slot: number;
  attention?: boolean;
  attentionAt?: number;
}

export function terminalsNeedingAttention<T extends AttentionTarget>(terminals: T[]): T[] {
  return terminals.filter(terminal => terminal.attention).sort((a, b) =>
    (b.attentionAt ?? 0) - (a.attentionAt ?? 0)
      || a.projectId.localeCompare(b.projectId) || a.slot - b.slot,
  );
}

export function firstOpenedTerminalSlot(projectId: string, terminals: readonly {
  projectId: string;
  slot: number;
  sessionId?: string;
}[]): number | undefined {
  let first: number | undefined;
  for (const terminal of terminals) {
    if (terminal.projectId === projectId && terminal.sessionId && (first === undefined || terminal.slot < first)) {
      first = terminal.slot;
    }
  }
  return first;
}
