import type { DashboardDetail, Panel, SyncState } from '../api/types';
import { PanelGrid } from './PanelGrid';
import { GraphPanel } from './panels/GraphPanel';
import { HostStatesPanel } from './panels/HostStatesPanel';
import { ProblemsPanel } from './panels/ProblemsPanel';
import { ValuePanel } from './panels/ValuePanel';
import { WallFrame, type WallAlarm } from './WallFrame';

interface Props {
  dashboard: DashboardDetail;
  sync?: SyncState;
  now: Date;
  alarm?: WallAlarm;
  /** The ids of the alarms that caused a switch to this dashboard; every problems panel marks them. */
  causes: string[];
  lostSince?: Date;
  back?: string;
}

/** The content of one panel by its type; a type this page does not know yet keeps its cell. */
function PanelContent({ panel, now, causes }: { panel: Panel; now: Date; causes: string[] }) {
  switch (panel.type) {
    case 'problems':
      return <ProblemsPanel panel={panel} now={now} causes={causes} />;
    case 'host-states':
      return <HostStatesPanel panel={panel} />;
    case 'graph':
      return <GraphPanel panel={panel} now={now} />;
    case 'value':
      return <ValuePanel panel={panel} now={now} />;
    default:
      // A newer server may have panel types this page does not know: they must not break it.
      return (
        <>
          <div class="wall-panel-label">Unknown panel</div>
          <div class="wall-panel-body" />
        </>
      );
  }
}

/** One dashboard on the wall: the frame with its name, and its panels on the 12 by 6 grid. */
export function DashboardView({ dashboard, sync, now, alarm, causes, lostSince, back }: Props) {
  return (
    <WallFrame title={dashboard.name} sync={sync} now={now} alarm={alarm} lostSince={lostSince} back={back}>
      <PanelGrid
        panels={dashboard.panels}
        render={(panel) => <PanelContent panel={panel} now={now} causes={causes} />}
      />
    </WallFrame>
  );
}
