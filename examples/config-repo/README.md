# Example configuration repository

A minimal Cerbrix configuration repository. Every file is JSON; the file name
(without `.json`) is the identifier where one is needed. The JSON Schemas for
the editor live in `schemas/` at the root of the Cerbrix repository.

Layout, five directories:

- `hosts/`: one file per host. Subdirectories are free for organisation
  (here `hosts/db/db-01.json`). A host lists its groups, the templates it
  uses, its own items and its own triggers.
- `templates/`: reusable sets of items and triggers (`linux-base`). Items a
  host defines itself override the template's item with the same key and
  parameters.
- `notifications/`: who is told about problems, by minimum severity and group.
- `dashboards/`: a dashboard selects hosts by group or name and places
  panels on a 12x6 grid. The id is the file name (`production`).
- `displays/`: a display shows a dashboard and can switch to another when a
  problem of a given severity appears (`noc`).

Item keys must be built-in items. Group and template names match
`^[a-z0-9][a-z0-9_-]{0,62}$`. Do not add a `$schema` key to the files; unknown
fields are rejected.
