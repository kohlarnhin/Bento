import type { EnvVar, ProjectConfig, Service, ServiceRuntimeStatus } from '../types.ts';

export type ServiceDraft = Omit<Service, 'port'> & { port: string };

export function serviceDraft({ id, name, dir, port, key, kind, openWeb, startCommand }: Service): ServiceDraft {
  return { id, name, dir, port: port?.toString() ?? '', key: key || defaultServiceKey(kind, dir), kind, openWeb,
    ...(startCommand ? { startCommand } : {}) };
}

export function savedServices(services: ServiceDraft[]): Service[] {
  return services.map(service => ({ ...service, port: portNumber(service.port) }));
}

export function normalizedServiceDir(dir: string) {
  return dir.trim().split('/').filter(part => part && part !== '.').join('/') || '.';
}

function serviceDirName(dir: string) {
  const normalized = normalizedServiceDir(dir);
  return normalized === '.' ? 'root' : normalized.split('/').at(-1)!;
}

export function defaultServiceKey(kind: string | undefined, dir: string) {
  if (kind === 'web') return 'WEB';
  if (kind === 'api') return 'API';
  return serviceDirName(dir).replace(/[^A-Za-z0-9]/gu, '_').toUpperCase();
}

export function uniqueServiceKey(base: string, services: Pick<Service, 'key'>[]) {
  const used = new Set(services.map(service => service.key));
  let key = base;
  let suffix = 2;
  while (used.has(key)) key = base + '_' + suffix++;
  return key;
}

export function serviceEnvName(key: string) {
  return 'BENTO_PORT_' + key;
}

export function missingPortReference(variable: string) {
  return '脚本未读取 ' + variable + '，Bento 分配的端口不会生效';
}

export function portNumber(value: string) {
  const port = Number(value);
  return /^\d+$/.test(value.trim()) && port >= 1 && port <= 65535 ? port : null;
}

export function projectPorts(project: Pick<ProjectConfig, 'port' | 'services'>) {
  return project.services?.length
    ? project.services.flatMap(service => service.port == null ? [] : [service.port])
    : project.port == null ? [] : [project.port];
}

export function injectedPortVariables(project: Pick<ProjectConfig, 'env' | 'port' | 'services' | 'injectPorts'>): EnvVar[] {
  if (project.injectPorts === false) return [];
  const explicit = new Set(project.env.map(entry => entry.key));
  const entries: EnvVar[] = [];
  const inject = (key: string, value: string) => {
    if (!explicit.has(key)) entries.push({ key, value });
  };
  if (project.port != null) inject('PORT', String(project.port));
  if (project.services.length < 2) return entries;
  const ports = [];
  for (const service of project.services) {
    if (service.port == null) continue;
    inject(serviceEnvName(service.key), String(service.port));
    ports.push(service.key + ':' + service.port);
  }
  if (ports.length) inject('BENTO_PORTS', ports.join(','));
  return entries;
}

export function sameServiceStatuses(previous: ServiceRuntimeStatus[] = [], next: ServiceRuntimeStatus[] = []) {
  return previous.length === next.length && next.every((service, index) => {
    const old = previous[index];
    return old.serviceId === service.serviceId && old.port === service.port && old.listening === service.listening &&
      old.owner?.pid === service.owner?.pid && old.owner?.inProject === service.owner?.inProject;
  });
}
