import { Fragment, useLayoutEffect, useRef, type MouseEventHandler, type ReactNode } from 'react';
import { cn } from '../../lib/utils';

interface Segment<T extends string> {
  value: T;
  label: ReactNode;
  title?: string;
  onContextMenu?: MouseEventHandler<HTMLButtonElement>;
  accessory?: ReactNode;
  after?: ReactNode;
  attributes?: { 'data-sidebar-filter'?: T; 'data-type'?: T };
}

export function SegmentedControl<T extends string>({ options, value, onChange, label, labelledBy, className, disabled }: {
  options: readonly Segment<T>[];
  value: T;
  onChange: (value: T) => void;
  label?: string;
  labelledBy?: string;
  className?: string;
  disabled?: boolean;
}) {
  const track = useRef<HTMLDivElement>(null);
  const indicator = useRef<HTMLSpanElement>(null);
  useLayoutEffect(() => {
    const group = track.current;
    const thumb = indicator.current;
    const active = group?.querySelector<HTMLButtonElement>('[aria-checked="true"]');
    if (!group || !thumb || !active) return;
    if (group.scrollWidth > group.clientWidth) active.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    const measure = () => {
      const bounds = group.getBoundingClientRect();
      const segment = active.getBoundingClientRect();
      thumb.style.transform = 'translateX(' + (segment.left - bounds.left + group.scrollLeft - group.clientLeft - 2) + 'px)';
      thumb.style.width = segment.width + 'px';
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(group);
    observer.observe(active);
    return () => observer.disconnect();
  }, [value, options.length]);

  return <div ref={track} role="radiogroup" aria-label={label} aria-labelledby={labelledBy} data-segmented-control
    className={cn('segmented-control', className)}
    onKeyDown={event => {
      const keys = ['ArrowRight', 'ArrowDown', 'ArrowLeft', 'ArrowUp', 'Home', 'End'];
      if (disabled || !keys.includes(event.key) || !(event.target instanceof HTMLButtonElement) || event.target.getAttribute('role') !== 'radio') return;
      event.preventDefault();
      const current = options.findIndex(option => option.value === value);
      const step = event.key === 'ArrowRight' || event.key === 'ArrowDown' ? 1 : -1;
      const next = event.key === 'Home' ? 0 : event.key === 'End' ? options.length - 1
        : (current + step + options.length) % options.length;
      onChange(options[next].value);
      track.current?.querySelectorAll<HTMLButtonElement>('[role="radio"]')[next]?.focus();
    }}>
    <span ref={indicator} aria-hidden="true" className="segmented-indicator" data-segmented-indicator />
    {options.map(option => {
      const button = <button type="button" role="radio" aria-checked={value === option.value}
        tabIndex={value === option.value ? 0 : -1} disabled={disabled} onClick={() => onChange(option.value)}
        title={option.title} onContextMenu={option.onContextMenu}
        className="segmented-option outline-none focus-visible:ring-2 focus-visible:ring-ring/50" {...option.attributes}>
        {option.label}
      </button>;
      return <Fragment key={option.value}>
        {option.accessory ? <div className="segmented-item">{button}{option.accessory}</div> : button}
        {option.after}
      </Fragment>;
    })}
  </div>;
}
