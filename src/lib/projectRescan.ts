import type { PortCandidate, ProjectDirInspection, ProjectDraft, ScannedService } from '../types';
import { normalizedServiceDir, portNumber, serviceDraft, uniqueServiceKey, type ServiceDraft } from './projectServices.ts';

type RunConfig = Pick<ProjectDraft, 'startCommand' | 'type'> & { port: string };
export type RescanField = keyof RunConfig | 'services';

export type RescanRow = {
  [Field in keyof RunConfig]: {
    field: Field;
    before: RunConfig[Field];
    after: RunConfig[Field];
    changed: boolean;
    source?: string;
    confidence?: PortCandidate['confidence'];
  };
}[keyof RunConfig];

export function diffProjectInspection(current: RunConfig & { services?: ServiceDraft[] }, inspection: ProjectDirInspection) {
  const currentServices = current.services ?? [];
  const discovered = inspection.services ?? [];
  const services = [...currentServices];
  const serviceChanges: { service: ScannedService; added: boolean; previousPort: string }[] = [];
  const multiService = !!(currentServices.length || discovered.length >= 2);
  if (multiService) {
    for (const scanned of discovered) {
      const index = services.findIndex(service => normalizedServiceDir(service.dir) === scanned.dir);
      if (index === -1) {
        const service = serviceDraft(scanned);
        services.push({ ...service, key: uniqueServiceKey(service.key, services), openWeb: !currentServices.length && scanned.openWeb });
        serviceChanges.push({ service: scanned, added: true, previousPort: '' });
      } else if (scanned.port != null && portNumber(services[index].port) !== scanned.port) {
        serviceChanges.push({ service: scanned, added: false, previousPort: services[index].port });
        services[index] = { ...services[index], port: String(scanned.port) };
      }
    }
  }
  const startScripts = multiService ? inspection.startScripts ?? [] : [];
  const generated = startScripts.find(script => script.command === current.startCommand.trim() &&
    /^\.\/scripts\/bento-dev(?:-\d+)?\.sh$/.test(script.command));
  const command = generated?.command || startScripts[0]?.command || inspection.commands[0] || current.startCommand;
  const port = inspection.port?.toString() ?? current.port;
  const portCandidate = inspection.portCandidates.find(candidate => candidate.port === inspection.port);
  const portSource = inspection.portSource ?? portCandidate?.source;
  const sourceFiles = (portCandidate?.source ?? portSource ?? '').split('、')
    .filter(source => source && source !== '框架默认' && source !== '推荐空闲端口' && !source.endsWith('默认端口'));
  const rows: RescanRow[] = [
    { field: 'startCommand', before: current.startCommand, after: command, changed: current.startCommand.trim() !== command.trim() },
  ];
  if (!multiService) rows.push({ field: 'port', before: current.port, after: port, changed: inspection.port !== null &&
    portNumber(current.port) !== inspection.port, source: portSource, confidence: portCandidate?.confidence });
  rows.push({ field: 'type', before: current.type, after: inspection.projectType, changed: current.type !== inspection.projectType });
  return { rows, sourceFiles: multiService ? [] : [...new Set(sourceFiles)], services, serviceChanges, startScripts };
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
