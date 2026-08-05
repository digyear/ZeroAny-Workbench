import type { Platform } from '$lib/utils/platform';

export interface TerminalSelection {
  hasSelection(): boolean;
  getSelection(): string;
}

export function isTerminalCopyShortcut(event: KeyboardEvent, platform: Platform): boolean {
  if (event.type !== 'keydown' || event.key.toLowerCase() !== 'c') return false;
  if (event.altKey) return false;
  if (platform === 'macos') {
    return event.metaKey && !event.ctrlKey && !event.shiftKey;
  }
  return event.ctrlKey && event.shiftKey && !event.metaKey;
}

export async function copyTerminalSelection(
  terminal: TerminalSelection,
  writeText: (text: string) => Promise<void>,
): Promise<boolean> {
  if (!terminal.hasSelection()) return false;
  await writeText(terminal.getSelection());
  return true;
}
