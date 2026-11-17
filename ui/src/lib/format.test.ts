import { age, clock, duration, severityWord, value } from './format';

const at = (y: number, mo: number, d: number, h = 0, mi = 0, s = 0) =>
  new Date(y, mo, d, h, mi, s);

describe('severityWord', () => {
  it('capitalises', () => {
    expect(severityWord('critical')).toBe('Critical');
    expect(severityWord('high')).toBe('High');
    expect(severityWord('warning')).toBe('Warning');
    expect(severityWord('info')).toBe('Info');
  });
});

describe('duration', () => {
  const start = at(2026, 10, 17, 12, 0, 0);
  const iso = start.toISOString();
  const later = (s: number) => new Date(start.getTime() + s * 1000);

  it('formats seconds, minutes, hours and days', () => {
    expect(duration(iso, later(0))).toBe('0 s');
    expect(duration(iso, later(59))).toBe('59 s');
    expect(duration(iso, later(60))).toBe('1 min');
    expect(duration(iso, later(3599))).toBe('59 min');
    expect(duration(iso, later(2 * 3600 + 14 * 60))).toBe('2 h 14 min');
    expect(duration(iso, later(3 * 86400 + 4 * 3600))).toBe('3 d 4 h');
  });

  it('leaves out a zero part', () => {
    expect(duration(iso, later(3600))).toBe('1 h');
    expect(duration(iso, later(86400))).toBe('1 d');
  });

  it('shows 0 s for a start in the future', () => {
    expect(duration(iso, later(-30))).toBe('0 s');
  });
});

describe('clock', () => {
  const now = at(2026, 10, 17, 14, 30);
  it('shows only the time today', () => {
    expect(clock(at(2026, 10, 17, 14, 5).toISOString(), now)).toBe('14:05');
    expect(clock(at(2026, 10, 17, 0, 7).toISOString(), now)).toBe('00:07');
  });
  it('adds the day and month on another day', () => {
    expect(clock(at(2026, 10, 16, 14, 5).toISOString(), now)).toBe('16 Nov 14:05');
  });
  it('adds the year in another year', () => {
    expect(clock(at(2025, 10, 16, 14, 5).toISOString(), now)).toBe('16 Nov 2025 14:05');
  });
});

describe('age', () => {
  const now = at(2026, 10, 17, 14, 30, 0);
  it('says never for null', () => {
    expect(age(null, now)).toBe('never');
  });
  it('counts back from now', () => {
    expect(age(at(2026, 10, 17, 14, 29, 48).toISOString(), now)).toBe('12 s ago');
    expect(age(at(2026, 10, 17, 14, 25, 0).toISOString(), now)).toBe('5 min ago');
    expect(age(at(2026, 10, 17, 11, 30, 0).toISOString(), now)).toBe('3 h ago');
    expect(age(at(2026, 10, 14, 14, 30, 0).toISOString(), now)).toBe('3 d ago');
    expect(age(at(2026, 10, 17, 14, 31, 0).toISOString(), now)).toBe('0 s ago');
  });
});

describe('value', () => {
  it('shows a dash for null', () => {
    expect(value('cpu.usage', null)).toBe('–');
  });
  it('formats percentages with one decimal', () => {
    expect(value('cpu.usage', 29.4078)).toBe('29.4 %');
    expect(value('memory.used_percent', 46.61)).toBe('46.6 %');
  });
  it('formats bytes in binary units', () => {
    expect(value('memory.used', 1288490188.8)).toBe('1.2 GiB');
    expect(value('disk.free', 512)).toBe('512 B');
    expect(value('disk.total', 2048)).toBe('2.0 KiB');
    expect(value('net.rx_bytes', 5 * 1024 ** 4)).toBe('5.0 TiB');
  });
  it('does not special-case rates', () => {
    expect(value('net.rx_bytes_per_sec', 1500.5)).toBe('1500.5');
  });
  it('shows at most three decimals', () => {
    expect(value('load.1m', 0.123456)).toBe('0.123');
    expect(value('load.5m', 2)).toBe('2');
    expect(value('load.5m', 2.5)).toBe('2.5');
  });
});
