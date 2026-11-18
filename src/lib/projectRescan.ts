import type { PortCandidate, ProjectDirInspection, ProjectDraft } from '../types';

type RunConfig = Pick<ProjectDraft, 'startCommand' | 'type'> & { port: string };
export type RescanField = keyof RunConfig;

export type RescanRow = {
  [Field in RescanField]: {
    field: Field;
    before: RunConfig[Field];
    after: RunConfig[Field];
    changed: boolean;
    source?: string;
    confidence?: PortCandidate['confidence'];
  };
}[RescanField];

export function diffProjectInspection(current: RunConfig, inspection: ProjectDirInspection) {
  const command = inspection.commands[0] || current.startCommand;
  const port = inspection.port?.toString() ?? current.port;
  const portCandidate = inspection.portCandidates.find(candidate => candidate.port === inspection.port);
  const portSource = inspection.portSource ?? portCandidate?.source;
  const sourceFiles = (portCandidate?.source ?? portSource ?? '').split('、')
    .filter(source => source && source !== '框架默认' && source !== '推荐空闲端口' && !source.endsWith('默认端口'));
  const rows: RescanRow[] = [
    { field: 'startCommand', before: current.startCommand, after: command, changed: current.startCommand.trim() !== command.trim() },
    { field: 'port', before: current.port, after: port, changed: inspection.port !== null &&
      (!/^\d+$/.test(current.port.trim()) || Number(current.port) !== inspection.port), source: portSource, confidence: portCandidate?.confidence },
    { field: 'type', before: current.type, after: inspection.projectType, changed: current.type !== inspection.projectType },
  ];
  return { rows, sourceFiles: [...new Set(sourceFiles)] };
}

export function applyInspectionDiff(rows: RescanRow[], selected: ReadonlySet<RescanField>): Partial<RunConfig> {
  const patch: Partial<RunConfig> = {};
  for (const row of rows) {
    if (!row.changed || !selected.has(row.field)) continue;
    if (row.field === 'type') patch.type = row.after;
    else patch[row.field] = row.after;
  }
  return patch;
}
