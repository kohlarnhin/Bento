import { AlertTriangle } from 'lucide-react';
import type { PortConflict } from '../types';
import { Alert, AlertDescription, AlertTitle } from './ui/alert';

export function PortConflictBanner({ conflicts }: { conflicts: PortConflict[] }) {
  if (!conflicts.length) return null;
  return (
    <Alert variant="warning">
      <AlertTriangle /><AlertTitle>检测到 {conflicts.length} 处端口冲突</AlertTitle>
      <AlertDescription>
        {conflicts.map((conflict) => <p key={conflict.port}>:{conflict.port} — {conflict.projectNames.join('、')}</p>)}
        <p className="mt-1">相关项目已限制启动，请分配不同端口。</p>
      </AlertDescription>
    </Alert>
  );
}
