import { render, screen } from '@testing-library/preact';

import { App } from './app';

describe('App', () => {
  it('renders its line of text', () => {
    render(<App />);
    expect(screen.getByText('Cerbrix')).toBeInTheDocument();
  });
});
