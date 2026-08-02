export interface TerminalSelection {
  hasSelection(): boolean;
  getSelection(): string;
}

export function isTerminalCopyShortcut(event: KeyboardEvent): boolean {
  if (event.type !== 'keydown' || event.key.toLowerCase() !== 'c') return false;
  return event.metaKey || (event.ctrlKey && event.shiftKey);
}

export async function copyTerminalSelection(
  terminal: TerminalSelection,
  writeText: (text: string) => Promise<void>,
): Promise<boolean> {
  if (!terminal.hasSelection()) return false;
  await writeText(terminal.getSelection());
  return true;
}
