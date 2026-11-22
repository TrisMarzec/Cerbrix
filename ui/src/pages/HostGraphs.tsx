import { useEffect, useRef, useState } from 'preact/hooks';

import { api, isApiError, seg } from '../api/client';
import type { HostItem, HostValue, Series } from '../api/types';
import { Banner } from '../components/Banner';
import { BusyRegion } from '../components/BusyRegion';
import { describeError } from '../components/describeError';
import { Graph } from '../components/Graph';
import { StaleNote } from '../components/StaleNote';
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

/** The identity of a series: the key and its sorted parameter entries, as JSON (never the display title). */
export function seriesId(key: string, params: Record<string, string>): string {
  const entries = Object.entries(params).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  return JSON.stringify([key, entries]);
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
    const id = seriesId(key, params);
    if (seen.has(id)) return;
    seen.add(id);
    graphs.push({ id, key, params, title: seriesTitle(key, params) });
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
  now: Date;
  /** Whether the last good `/values` answer has an entry for the key of this graph. */
  valuesHaveKey: boolean;
}

/** Loads the series of one graph (and again every minute) and draws it. */
function LoadedGraph({ host, spec, range, now, valuesHaveKey }: LoadedProps) {
  // Once a graph has been visible it keeps polling every 60 seconds, also after it scrolled out of
  // view. With thirty graphs that is thirty requests a minute: accepted, because every request is
  // small and the metrics store answers from its own cache.
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
    // Only the metrics store's own error: a 503 can also be a database out of reach or a timeout.
    if (isApiError(error, 'metrics_unavailable')) {
      return <p class="graph-empty">The metrics store did not answer</p>;
    }
    // 400 for an item asked for without parameters, of which the host has no value at all: the item
    // needs a parameter and no instance has reported yet.
    if (error.status === 400 && Object.keys(spec.params).length === 0 && !valuesHaveKey) {
      return <p class="graph-empty">{`No values yet for ${spec.key}`}</p>;
    }
    return (
      <Banner kind="warning">
        {`Could not load this graph: ${describeError(error)}`}{' '}
        <button type="button" class="btn" onClick={series.reload}>
          Try again
        </button>
      </Banner>
    );
  }
  return (
    <BusyRegion pending={series.pending} class="graph-busy">
      <StaleNote of={series} />
      <Graph
        points={answer.points}
        thresholds={answer.thresholds}
        unitKey={spec.key}
        stepSecs={answer.step_secs}
        now={now}
        label={`${spec.title} over the last ${RANGE_WORDS[range]}`}
      />
    </BusyRegion>
  );
}

/** One graph with its title. Its series is requested only after it has been on screen. */
export function SeriesGraph({ host, spec, range, now, valuesHaveKey }: LoadedProps) {
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
      {visible && (
        <LoadedGraph host={host} spec={spec} range={range} now={now} valuesHaveKey={valuesHaveKey} />
      )}
    </section>
  );
}
