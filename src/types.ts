export interface EnvVar { key: string; value: string }
export interface ExtraCommand { name: string; command: string }
export interface ProjectGroup { id: string; name: string; sortOrder: number; collapsed: boolean }

export interface Service {
  id: string;
  name: string;
  dir: string;
  port: number | null;
  key: string;
  kind?: string;
  openWeb: boolean;
  startCommand?: string;
}

export type ProjectLaunchConfig = Pick<ProjectDraft, 'path' | 'startCommand' | 'port' | 'services' | 'injectPorts' | 'env'>;
export interface ProjectLaunchPlan {
  mode: 'single' | 'shared' | 'independent';
  steps: {
    serviceId: string | null;
    dir: string;
    command: string;
    description: string;
    placeholderHint: boolean;
    portEnv: EnvVar[];
  }[];
}
export interface ScannedService extends Service { portCandidate: PortCandidate | null }
export interface StartScript { command: string; source: string }
export interface ServicePortCheck { serviceId: string; variable: string; references: string[] }
export interface PortStandardCheck { checkedFiles: string[]; services: ServicePortCheck[] }
export interface StandardScript { relativePath: string; script: string; preserved: string[]; notes: string[] }

export interface ProjectConfig {
  id: string;
  name: string;
  displayName?: string;
  description?: string;
  path: string;
  port?: number | null;
  services: Service[];
  injectPorts: boolean;
  startCommand: string;
  type?: 'web' | 'app' | string;
  env: EnvVar[];
  commands: ExtraCommand[];
  autoOpen: boolean;
  groupId: string | null;
  pinned: boolean;
  archived: boolean;
}

export interface ProjectDraft {
  name: string;
  displayName?: string;
  description?: string;
  path: string;
  port?: number | null;
  services: Service[];
  injectPorts: boolean;
  startCommand: string;
  type: 'web' | 'app';
  env: EnvVar[];
  commands: ExtraCommand[];
  autoOpen: boolean;
  groupId?: string | null;
}

export interface ProjectDirInspection {
  path: string;
  exists: boolean;
  isDir: boolean;
  name: string;
  projectType: 'web' | 'app';
  packageManager: string | null;
  commands: string[];
  port: number | null;
  portSource: string | null;
  portCandidates: PortCandidate[];
  existingProject: { id: string; name: string } | null;
  services: ScannedService[];
  startScripts: StartScript[];
  standard?: PortStandardCheck | null;
}

export interface PortCandidate {
  port: number;
  source: string;
  confidence: 'high' | 'medium' | 'low';
}

export interface PortConflictCheck {
  projectConflict: { id: string; name: string } | null;
  inUse: { owners: PortOwner[] } | null;
  reserved: string[];
  suggestion: number | null;
}

export interface PortConflict {
  port: number;
  projectIds: string[];
  projectNames: string[];
}

export interface ProjectsResponse {
  projects: ProjectConfig[];
  groups: ProjectGroup[];
  conflicts: PortConflict[];
  configPath: string;
}

export type ProjectStatus = 'stopped' | 'starting' | 'running' | 'error';

export interface RunningProjectInfo {
  projectId: string;
  pid: number;
  port?: number | null;
}

export interface ProjectRuntimeStatus {
  projectId: string;
  port?: number | null;
  actualPort?: number | null;
  status: ProjectStatus;
  isPortInUse: boolean;
  isManaged: boolean;
  external: boolean;
  pid?: number | null;
  exitCode?: number | null;
  exitSignal?: number | null;
  startedAt?: number | null;
  revision: number;
  services: ServiceRuntimeStatus[];
}

export interface ServiceRuntimeStatus {
  serviceId: string;
  port: number | null;
  actualPort?: number | null;
  listening: boolean;
  owner?: { pid: number | null; inProject: boolean } | null;
}

export interface PortOwner {
  pid: number;
  command: string;
  cwd: string | null;
  inProject: boolean;
}

export interface LogEntry {
  id: string;
  projectId: string;
  text: string;
  stream: 'stdout' | 'stderr' | 'system';
  timestamp: number;
}

export type ProjectView = 'overview' | 'terminal' | 'git';

export interface ProjectSelection {
  projectId: string;
  view: ProjectView;
}

export interface TerminalSessionInfo {
  projectId: string;
  slot: number;
  sessionId: string;
  alive: boolean;
  attention: boolean;
  title?: string;
  restoredActive?: boolean;
  completion?: TerminalCompletion | null;
  exitCode?: number | null;
  pid?: number | null;
}

export interface TerminalCompletion {
  command: string;
  elapsedMs: number;
  exitCode: number;
  endedAt: number;
}

export type TerminalAttention = Pick<TerminalSessionInfo, 'projectId' | 'slot' | 'sessionId' | 'attention'>;
export type TerminalCompletionEvent = Pick<TerminalSessionInfo, 'projectId' | 'slot' | 'sessionId' | 'completion'>;

export interface Snippet {
  id: number;
  name: string;
  command: string;
}

export type SnippetChange = { action: 'add'; name: string; command: string }
  | ({ action: 'edit' } & Snippet)
  | { action: 'delete'; id: number };

export interface TerminalOutput {
  projectId: string;
  slot: number;
  sessionId: string;
  seq: number;
  data: string;
}

export interface TerminalOpenResult {
  session: TerminalSessionInfo;
  snapshot: { data: string; seq: number };
}

export interface GitInfo {
  /** null while HEAD is detached. */
  branch: string | null;
  head: string | null;
  initial: boolean;
  upstream: string | null;
  ahead: number;
  behind: number;
  changes: number;
}

export interface DashboardGit extends GitInfo {
  files: { path: string; status: string; added: number | null; removed: number | null }[];
  latest: { subject: string; timestamp: number } | null;
}

export interface DashboardCpuPoint { at: number; value: number }
export interface DashboardUsage { cpu: number; rssKib: number; process: string | null }
export interface DashboardHealth { port: number; path: string; code: number | null; elapsedMs: number }
export type DashboardListener = 'idle' | 'listening' | 'occupied';
export interface DashboardServiceSample {
  id: string;
  port: number | null;
  actualPort?: number | null;
  state: DashboardListener;
  usage: DashboardUsage | null;
  unexpectedPorts?: number[];
}
export interface DashboardConflict {
  serviceId: string;
  port: number;
  owner: PortOwner | null;
  identity: string | null;
  suggestion: number | null;
}
export interface DashboardRun {
  startedAt: number;
  endedAt: number;
  exitCode: number | null;
  signal: number | null;
  peakKib: number | null;
  errors: number;
  external: boolean;
  services: Record<string, { port: number | null; process: string | null; health: DashboardHealth | null }>;
  releasedPorts: number[];
}
export interface DashboardSnapshot {
  sampledAt: number;
  startedAt: number | null;
  external: boolean;
  services: DashboardServiceSample[];
  total: DashboardUsage | null;
  history: Record<string, DashboardCpuPoint[]>;
  conflicts: DashboardConflict[];
  lastRun: DashboardRun | null;
}
