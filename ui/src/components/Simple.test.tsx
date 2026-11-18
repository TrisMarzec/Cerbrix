import { render, screen } from '@testing-library/preact';

import { NOW } from '../test/fixtures';
import { Banner } from './Banner';
import { Empty } from './Empty';
import { TimeText } from './TimeText';

describe('Banner', () => {
  it('is an alert for a warning and a status for info', () => {
    const { rerender } = render(<Banner kind="warning">Out of sync</Banner>);
    expect(screen.getByRole('alert')).toHaveTextContent('Out of sync');
    rerender(<Banner kind="info">Managed by Git</Banner>);
    expect(screen.getByRole('status')).toHaveTextContent('Managed by Git');
  });
});

describe('Empty', () => {
  it('shows its sentence', () => {
    render(<Empty>No hosts yet.</Empty>);
    expect(screen.getByText('No hosts yet.')).toBeInTheDocument();
  });
});

describe('TimeText', () => {
  it('shows the short time with the full time as title', () => {
    const iso = new Date(2026, 10, 17, 14, 5).toISOString();
    render(<TimeText iso={iso} now={NOW} />);
    const el = screen.getByText('14:05');
    expect(el.tagName).toBe('TIME');
    expect(el).toHaveAttribute('datetime', iso);
    expect(el).toHaveAttribute('title', iso);
  });
});
