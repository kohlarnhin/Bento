export function selectKeyAction({ key, keyCode, composing, open, editable, activeIndex, optionCount }: {
  key: string;
  keyCode?: number;
  composing?: boolean;
  open: boolean;
  editable: boolean;
  activeIndex: number;
  optionCount: number;
}) {
  if (composing || keyCode === 229) return;
  if (key === 'ArrowDown' || key === 'ArrowUp') {
    const backwards = key === 'ArrowUp';
    if (!open) return { kind: 'open', backwards } as const;
    if (!optionCount) return;
    const index = activeIndex < 0
      ? (backwards ? optionCount - 1 : 0)
      : Math.max(0, Math.min(optionCount - 1, activeIndex + (backwards ? -1 : 1)));
    return { kind: 'move', index } as const;
  }
  if (key === 'Enter' || (key === ' ' && !editable)) {
    return open ? { kind: 'choose', index: activeIndex } as const : { kind: 'open', backwards: false } as const;
  }
  if (open && key === 'Escape') return { kind: 'escape' } as const;
  if (open && key === 'Tab') return { kind: 'tab' } as const;
}
