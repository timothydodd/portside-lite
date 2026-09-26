import { save } from "@tauri-apps/plugin-dialog";
import * as ipc from "./ipc";

/**
 * Ask where to save, then write. Returns the chosen path, or null if the
 * user cancelled.
 */
export async function saveYamlFile(defaultName: string, contents: string): Promise<string | null> {
  const path = await save({
    defaultPath: defaultName,
    filters: [{ name: "YAML", extensions: ["yaml", "yml"] }],
  });
  if (!path) return null;
  await ipc.saveTextFile(path, contents);
  return path;
}
