export function buildInfo(options?: {
  env?: Record<string, string | undefined>;
  git?: () => string;
  now?: () => Date;
}): { version: string; date: string; commit: string };
