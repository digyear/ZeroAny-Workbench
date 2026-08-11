// @ts-expect-error Bun provides this module when running `bun test`.
import { describe, expect, test } from 'bun:test';
import { shouldForwardDesktopTerminalData } from './terminal-utils';

const phoneOwned = new Set(['phone-terminal']);

describe('shouldForwardDesktopTerminalData', () => {
  test('forwards desktop xterm data while the desktop owns the PTY', () => {
    expect(shouldForwardDesktopTerminalData('desktop-terminal', phoneOwned)).toBe(true);
  });

  test('drops xterm replies while a phone owns the shared PTY', () => {
    expect(shouldForwardDesktopTerminalData('phone-terminal', phoneOwned)).toBe(false);
  });

  test('drops data before a backend terminal id exists', () => {
    expect(shouldForwardDesktopTerminalData(null, phoneOwned)).toBe(false);
  });
});
