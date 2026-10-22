export interface EnvVar { key: string; value: string }
export interface ExtraCommand { name: string; command: string }

export interface ProjectConfig {
  id: string;
  name: string;
  displayName?: string;
  description?: string;
  path: string;
  port?: number | null;
  startCommand: string;
  type?: 'web' | 'app' | string;
  env: EnvVar[];
  commands: ExtraCommand[];
  autoOpen: boolean;
}

export interface ProjectDraft {
  name: string;
  displayName?: string;
  description?: string;
  path: string;
  port?: number | null;
  startCommand: string;
  type: 'web' | 'app';
  env: EnvVar[];
  commands: ExtraCommand[];
  autoOpen: boolean;
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
  existingProject: { id: string; name: string } | null;
}

export interface PortConflict {
  port: number;
  projectIds: string[];
  projectNames: string[];
}

export interface ProjectsResponse {
  projects: ProjectConfig[];
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
  status: ProjectStatus;
  isPortInUse: boolean;
  isManaged: boolean;
  external: boolean;
  pid?: number | null;
  exitCode?: number | null;
  exitSignal?: number | null;
  revision: number;
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

export type ProjectView = 'overview' | 'terminal';

export interface ProjectSelection {
  projectId: string;
  view: ProjectView;
}

export interface TerminalSessionInfo {
  projectId: string;
  sessionId: string;
  alive: boolean;
  attention: boolean;
  exitCode?: number | null;
  pid?: number | null;
}

export type TerminalAttention = Pick<TerminalSessionInfo, 'projectId' | 'sessionId' | 'attention'>;

export interface TerminalOutput {
  projectId: string;
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
