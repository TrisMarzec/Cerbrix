-- Serves the retention job, which deletes the problems resolved longest ago first.
CREATE INDEX problems_resolved ON problems (resolved_at) WHERE resolved_at IS NOT NULL;
