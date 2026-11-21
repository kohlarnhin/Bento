import { useEffect, useState } from 'react';
import { agentDurationRefreshDelay, formatAgentDuration } from '../lib/terminalUsability';

export function AgentElapsedTime({ startAt, endAt, compact, paused }: {
  startAt: number;
  endAt?: number;
  compact?: boolean;
  paused?: boolean;
}) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (endAt !== undefined || paused) return;
    let timer: ReturnType<typeof setTimeout>;
    const refresh = () => {
      const current = Date.now();
      setNow(current);
      timer = setTimeout(refresh, agentDurationRefreshDelay(current - startAt));
    };
    refresh();
    return () => clearTimeout(timer);
  }, [startAt, endAt, paused]);

  const elapsed = (endAt ?? now) - startAt;
  const duration = formatAgentDuration(elapsed);
  return <time dateTime={`PT${Math.max(0, Math.floor(elapsed / 1_000))}S`}>
    {compact ? <><span aria-hidden="true">{duration.compact}</span><span className="sr-only">{duration.full}</span></> : duration.full}
  </time>;
}
