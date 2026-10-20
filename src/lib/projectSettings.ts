import type { EnvVar, ExtraCommand } from '../types';

const reserved = new Set(['TERM', 'COLORTERM', 'SHELL', 'HOME', 'USER', 'PWD']);
export interface AdvancedErrors {
  env: { key?: string }[];
  commands: { name?: string; command?: string }[];
  commandLimit?: string;
}

export function validateAdvanced(env: EnvVar[], commands: ExtraCommand[]): AdvancedErrors {
  return {
    env: env.map(entry => ({ key: !/^[A-Za-z_][A-Za-z0-9_]*$/.test(entry.key)
      ? '名称须以字母或下划线开头，且只能包含字母、数字和下划线'
      : reserved.has(entry.key) ? `不能覆盖终端保留变量 ${entry.key}`
        : env.filter(other => other.key === entry.key).length > 1 ? `变量名称 ${entry.key} 重复` : undefined })),
    commands: commands.map(entry => ({
      name: !entry.name.trim() ? '请输入命令名称' : [...entry.name.trim()].length > 20 ? '名称不能超过 20 个字符'
        : commands.filter(other => other.name.trim() === entry.name.trim()).length > 1 ? '命令名称重复' : undefined,
      command: !entry.command.trim() ? '请输入命令' : undefined,
    })),
    commandLimit: commands.length > 10 ? '其他命令最多添加 10 个' : undefined,
  };
}

export const hasAdvancedErrors = (errors: AdvancedErrors) => !!errors.commandLimit || errors.env.some(row => !!row.key) || errors.commands.some(row => !!(row.name || row.command));

// Parse text only: no shell evaluation, variable expansion, or file access.
export function parseEnvPaste(text: string): EnvVar[] {
  const lines = text.replace(/\r\n/g, '\n').split('\n');
  const entries: EnvVar[] = [];
  for (let index = 0; index < lines.length; index++) {
    const line = lines[index].trimStart().replace(/^export\s+/, '');
    if (!line || line.startsWith('#')) continue;
    const equals = line.indexOf('=');
    if (equals < 1) continue;
    const key = line.slice(0, equals).trim();
    let raw = line.slice(equals + 1).trimStart();
    let value: string;
    const quote = raw[0];
    if (quote === '"' || quote === "'") {
      let end = -1;
      const closing = () => {
        for (let position = 1; position < raw.length; position++) {
          if (quote === '"' && raw[position] === '\\') { position++; continue; }
          if (raw[position] === quote) return position;
        }
        return -1;
      };
      while ((end = closing()) < 0 && index + 1 < lines.length) raw += '\n' + lines[++index];
      value = raw.slice(1, end < 0 ? undefined : end);
      if (quote === '"') value = value.replace(/\\([nrt"\\])/g, (_, escape: string) => ({ n: '\n', r: '\r', t: '\t', '"': '"', '\\': '\\' }[escape]!));
    } else value = raw.replace(/\s+#.*$/, '').trimEnd();
    entries.push({ key, value });
  }
  return entries;
}
