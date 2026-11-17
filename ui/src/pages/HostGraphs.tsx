import { useEffect, useRef, useState } from 'preact/hooks';

import { api, seg } from '../api/client';
import type { HostItem, HostValue, Series } from '../api/types';
import { Banner } from '../components/Banner';
import { Graph } from '../components/Graph';
import { usePolled } from '../lib/poll';

export type Range = '1h' | '6h' | '24h' | '7d';

export const RANGES: Range[] = ['1h', '6h', '24h', '7d'];
export const DEFAULT_RANGE: Range = '6h';

const RANGE_WORDS: Record<Range, string> = {
  '1h': 'hour',
  '6h': '6 hours',
  '24h': '24 hours',
  '7d': '7 days',
};

const SERIES_EVERY_MS = 60_000;

/** One graph to draw: an item key and the parameters that select one series of it. */
export interface GraphSpec {
  id: string;
  key: string;
  params: Record<string, string>;
  title: string;
}

/** "disk.used_percent · mount=/": the item key and its parameters, as text. */
export function seriesTitle(key: string, params: Record<string, string> | null): string {
  const entries = Object.entries(params ?? {}).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  if (entries.length === 0) return key;
  return `${key} · ${entries.map(([name, v]) => `${name}=${v}`).join(', ')}`;
}

function hasParams(params: Record<string, string> | null): params is Record<string, string> {
  return params !== null && Object.keys(params).length > 0;
}

function sortValue(params: Record<string, string>): string {
  return Object.entries(params)
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    .map(([, v]) => v)
    .join('\u0000');
}

/**
 * The graphs of a host: per item one without parameter, one per parameter value found in the
 * current values for an item with parameters. `waiting` lists keys of items that take a parameter
 * but have no value yet.
 */
export function graphSpecs(
  items: HostItem[],
  values: HostValue[],
): { graphs: GraphSpec[]; waiting: string[] } {
  const graphs: GraphSpec[] = [];
  const waiting: string[] = [];
  const seen = new Set<string>();
  const add = (key: string, params: Record<string, string>) => {
    const title = seriesTitle(key, params);
    if (seen.has(title)) return;
    seen.add(title);
    graphs.push({ id: title, key, params, title });
  };

  for (const item of items) {
    const narrowed = item.params;
    const found = values
      .filter(
        (v) =>
          v.key === item.key &&
          hasParams(v.params) &&
          (!hasParams(narrowed) || Object.entries(narrowed).every(([n, x]) => v.params[n] === x)),
      )
      .map((v) => v.params)
      .sort((a, b) => sortValue(a).localeCompare(sortValue(b), undefined, { numeric: true }));
    if (found.length > 0) {
      for (const params of found) add(item.key, params);
    } else if (hasParams(narrowed)) {
      if (!waiting.includes(item.key)) waiting.push(item.key);
    } else {
      add(item.key, {});
    }
  }
  return { graphs, waiting };
}

interface LoadedProps {
  host: string;
  spec: GraphSpec;
  range: Range;
}

/** Loads the series of one graph (and again every minute) and draws it. */
function LoadedGraph({ host, spec, range }: LoadedProps) {
  const series = usePolled(
    (signal) => {
      const query: Record<string, string> = { item: spec.key, range };
      for (const [name, v] of Object.entries(spec.params)) query[`p_${name}`] = v;
      return api.get<Series>(`/hosts/${seg(host)}/series`, query, signal);
    },
    [host, spec.id, range],
    SERIES_EVERY_MS,
  );

  const answer = series.data;
  if (answer === undefined) {
    const error = series.error;
    if (error === undefined) return null;
    // 400: the item needs a parameter and the host has reported no instance of it yet.
    if (error.status === 400) return <p class="graph-empty">No data in this range</p>;
    if (error.status === 503) return <p class="graph-empty">The metrics store did not answer</p>;
    return (
      <Banner kind="warning">
        Could not load this graph.{' '}
        <button type="button" class="btn" onClick={series.reload}>
          Try again
        </button>
      </Banner>
    );
  }
  return (
    <div class={series.pending ? 'graph-busy is-busy' : 'graph-busy'} aria-busy={series.pending}>
      <Graph
        points={answer.points}
        thresholds={answer.thresholds}
        unitKey={spec.key}
        stepSecs={answer.step_secs}
        label={`${spec.title} over the last ${RANGE_WORDS[range]}`}
      />
    </div>
  );
}

/** One graph with its title. Its series is requested only after it has been on screen. */
export function SeriesGraph({ host, spec, range }: LoadedProps) {
  const frame = useRef<HTMLElement>(null);
  // Without IntersectionObserver there is no way to know: load at once.
  const [visible, setVisible] = useState(() => typeof IntersectionObserver === 'undefined');

  useEffect(() => {
    const el = frame.current;
    if (visible || el === null) return undefined;
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting)) {
        setVisible(true);
        observer.disconnect();
      }
    });
    observer.observe(el);
    return () => {
      observer.disconnect();
    };
  }, [visible]);

  return (
    <section class="graph-frame" ref={frame}>
      <h3 title={spec.title}>{spec.title}</h3>
      {visible && <LoadedGraph host={host} spec={spec} range={range} />}
    </section>
  );
}
