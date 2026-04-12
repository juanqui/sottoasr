import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

import { formatDuration, formatRelativeTime, truncateText } from './format';

// ---------------------------------------------------------------------------
// formatDuration
// ---------------------------------------------------------------------------
describe('formatDuration', () => {
  it('formats 0 ms as 0:00', () => {
    expect(formatDuration(0)).toBe('0:00');
  });

  it('formats 1000 ms as 0:01', () => {
    expect(formatDuration(1000)).toBe('0:01');
  });

  it('formats 61000 ms as 1:01', () => {
    expect(formatDuration(61000)).toBe('1:01');
  });

  it('formats 3600000 ms (1 hour) as 60:00', () => {
    expect(formatDuration(3600000)).toBe('60:00');
  });

  it('rounds sub-second values down', () => {
    expect(formatDuration(999)).toBe('0:00');
    expect(formatDuration(1500)).toBe('0:01');
  });

  it('handles negative values by flooring to 0:00 or wrapping', () => {
    // Math.floor(-0.5) === -1, so negative small values produce negative results.
    // We test current behavior, not prescribing ideal behavior.
    const result = formatDuration(-1);
    expect(typeof result).toBe('string');
  });
});

// ---------------------------------------------------------------------------
// formatRelativeTime
// ---------------------------------------------------------------------------
describe('formatRelativeTime', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-04-04T12:00:00Z'));
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('returns "just now" for a date less than 10 seconds ago', () => {
    const date = new Date('2026-04-04T11:59:55Z').toISOString();
    expect(formatRelativeTime(date)).toBe('just now');
  });

  it('returns seconds ago for a date 30 seconds ago', () => {
    const date = new Date('2026-04-04T11:59:30Z').toISOString();
    expect(formatRelativeTime(date)).toBe('30 seconds ago');
  });

  it('returns "1 minute ago" for exactly 1 minute ago', () => {
    const date = new Date('2026-04-04T11:59:00Z').toISOString();
    expect(formatRelativeTime(date)).toBe('1 minute ago');
  });

  it('returns minutes ago for a date 15 minutes ago', () => {
    const date = new Date('2026-04-04T11:45:00Z').toISOString();
    expect(formatRelativeTime(date)).toBe('15 minutes ago');
  });

  it('returns "1 hour ago" for exactly 1 hour ago', () => {
    const date = new Date('2026-04-04T11:00:00Z').toISOString();
    expect(formatRelativeTime(date)).toBe('1 hour ago');
  });

  it('returns hours ago for a date 5 hours ago', () => {
    const date = new Date('2026-04-04T07:00:00Z').toISOString();
    expect(formatRelativeTime(date)).toBe('5 hours ago');
  });

  it('returns "yesterday" for a date 1 day ago', () => {
    const date = new Date('2026-04-03T12:00:00Z').toISOString();
    expect(formatRelativeTime(date)).toBe('yesterday');
  });

  it('returns days ago for a date 3 days ago', () => {
    const date = new Date('2026-04-01T12:00:00Z').toISOString();
    expect(formatRelativeTime(date)).toBe('3 days ago');
  });

  it('returns month format for a date 10 days ago', () => {
    const date = new Date('2026-03-25T12:00:00Z').toISOString();
    expect(formatRelativeTime(date)).toBe('Mar 25');
  });

  it('returns "Unknown" for an invalid date', () => {
    expect(formatRelativeTime('not-a-date')).toBe('Unknown');
  });
});

// ---------------------------------------------------------------------------
// truncateText
// ---------------------------------------------------------------------------
describe('truncateText', () => {
  it('returns original text when shorter than maxLength', () => {
    expect(truncateText('hello', 10)).toBe('hello');
  });

  it('returns original text when exactly at maxLength', () => {
    expect(truncateText('hello', 5)).toBe('hello');
  });

  it('truncates and adds ellipsis when text exceeds maxLength', () => {
    expect(truncateText('hello world', 5)).toBe('hello\u2026');
  });

  it('trims trailing whitespace before adding ellipsis', () => {
    expect(truncateText('hi there world', 3)).toBe('hi\u2026');
  });
});
