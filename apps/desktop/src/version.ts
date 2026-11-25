export interface BuildInfo {
  version: string;
  date: string;
  commit: string;
}
export function formatVersion(info: BuildInfo): string {
  return `v${info.version.replace(/^v/, "")} · ${info.date} · ${info.commit}`;
}
declare const __BUILD_INFO__: BuildInfo;
export const buildVersion = formatVersion(__BUILD_INFO__);
