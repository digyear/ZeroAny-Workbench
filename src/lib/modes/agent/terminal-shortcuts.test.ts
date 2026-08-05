// @ts-expect-error Bun provides this module when running `bun test`.
import { describe, expect, test } from 'bun:test';
import { copyTerminalSelection, isTerminalCopyShortcut } from './terminal-shortcuts';

function key(overrides: Partial<KeyboardEvent>): KeyboardEvent {
  return {
    type: 'keydown',
    key: 'c',
    ctrlKey: false,
    metaKey: false,
    shiftKey: false,
    altKey: false,
    ...overrides,
  } as KeyboardEvent;
}

describe('isTerminalCopyShortcut', () => {
  test('uses Ctrl+Shift+C on Linux terminals', () => {
    expect(isTerminalCopyShortcut(key({ ctrlKey: true, shiftKey: true }), 'linux')).toBe(true);
  });

  test('uses Ctrl+Shift+C on Windows terminals', () => {
    expect(isTerminalCopyShortcut(key({ ctrlKey: true, shiftKey: true }), 'windows')).toBe(true);
  });

  test('uses Cmd+C on macOS terminals', () => {
    expect(isTerminalCopyShortcut(key({ metaKey: true }), 'macos')).toBe(true);
  });

  test('does not use Cmd+C on Linux or Windows terminals', () => {
    expect(isTerminalCopyShortcut(key({ metaKey: true }), 'linux')).toBe(false);
    expect(isTerminalCopyShortcut(key({ metaKey: true }), 'windows')).toBe(false);
  });

  test('does not use Ctrl+Shift+C on macOS terminals', () => {
    expect(isTerminalCopyShortcut(key({ ctrlKey: true, shiftKey: true }), 'macos')).toBe(false);
  });

  test('does not steal Ctrl+C from the running process', () => {
    expect(isTerminalCopyShortcut(key({ ctrlKey: true }), 'linux')).toBe(false);
    expect(isTerminalCopyShortcut(key({ ctrlKey: true }), 'windows')).toBe(false);
    expect(isTerminalCopyShortcut(key({ ctrlKey: true }), 'macos')).toBe(false);
  });

  test('does not accept additional modifiers', () => {
    expect(isTerminalCopyShortcut(key({ metaKey: true, shiftKey: true }), 'macos')).toBe(false);
    expect(isTerminalCopyShortcut(key({ ctrlKey: true, shiftKey: true, altKey: true }), 'linux')).toBe(false);
    expect(isTerminalCopyShortcut(key({ ctrlKey: true, shiftKey: true, metaKey: true }), 'windows')).toBe(false);
  });

  test('handles shifted key casing and only keydown events', () => {
    expect(isTerminalCopyShortcut(key({ key: 'C', ctrlKey: true, shiftKey: true }), 'linux')).toBe(true);
    expect(isTerminalCopyShortcut(key({ type: 'keyup', ctrlKey: true, shiftKey: true }), 'linux')).toBe(false);
  });
});

describe('copyTerminalSelection', () => {
  test('writes the selected terminal text to the clipboard', async () => {
    let clipboard = '';
    const copied = await copyTerminalSelection(
      { hasSelection: () => true, getSelection: () => 'Hermes output' },
      async (text) => { clipboard = text; },
    );

    expect(copied).toBe(true);
    expect(clipboard).toBe('Hermes output');
  });

  test('does nothing when there is no selection', async () => {
    let writes = 0;
    const copied = await copyTerminalSelection(
      { hasSelection: () => false, getSelection: () => '' },
      async () => { writes += 1; },
    );

    expect(copied).toBe(false);
    expect(writes).toBe(0);
  });
});
