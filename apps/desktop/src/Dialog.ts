// Native dialogs do not expand the asset protocol's cache-only scope.
import { invoke } from "@tauri-apps/api/core";
interface Options {
  directory?: boolean;
  multiple?: boolean;
  title?: string;
  defaultPath?: string;
  filters?: { name: string; extensions: string[] }[];
}
export async function open(
  options: Options,
): Promise<string | string[] | null> {
  const paths = await invoke<string[] | null>("pick_path", {
    options,
    save: false,
  });
  return options.multiple ? paths : paths?.[0] || null;
}
export async function save(options: Options): Promise<string | null> {
  return (
    (
      await invoke<string[] | null>("pick_path", { options, save: true })
    )?.[0] || null
  );
}
