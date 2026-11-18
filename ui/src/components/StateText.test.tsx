import { render } from '@testing-library/preact';

import { criticalState, noDataState, warningAcknowledgedState } from '../test/fixtures';
import { StateText } from './StateText';

describe('StateText', () => {
  it('says there are no problems', () => {
    const { container } = render(<StateText state={null} />);
    expect(container).toHaveTextContent('No problems');
    expect(container.firstElementChild).toHaveClass('is-ok');
  });

  it('names severity and trigger', () => {
    const { container } = render(<StateText state={criticalState} />);
    expect(container).toHaveTextContent('Critical: Disk almost full');
    expect(container.firstElementChild).toHaveClass('sev-critical');
  });

  it('marks an acknowledged state', () => {
    const { container } = render(<StateText state={warningAcknowledgedState} />);
    expect(container).toHaveTextContent('Warning: Memory high (acknowledged)');
    expect(container.firstElementChild).toHaveClass('is-acknowledged');
  });

  it('counts the other problems', () => {
    const { container } = render(<StateText state={{ ...criticalState, open_problems: 3 }} />);
    expect(container).toHaveTextContent('Critical: Disk almost full and 2 more');
  });

  it('puts both marks in order', () => {
    const { container } = render(
      <StateText state={{ ...warningAcknowledgedState, open_problems: 2 }} />,
    );
    expect(container).toHaveTextContent('Warning: Memory high (acknowledged) and 1 more');
  });

  it('keeps a long trigger name inside the wrapping element', () => {
    const long = 'y'.repeat(200);
    const { container } = render(<StateText state={{ ...criticalState, trigger: long }} />);
    expect(container.querySelector('.state-text')).toHaveTextContent(long);
  });

  it('shows the summary for no data', () => {
    const { container } = render(<StateText state={noDataState} />);
    expect(container).toHaveTextContent('Critical: no data for 5m');
  });
});
