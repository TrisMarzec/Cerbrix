//! Validation of dashboards and displays.

use std::collections::{BTreeMap, BTreeSet};

use cerbrix_common::{
    DashboardFile, DisplayFile, GRID_COLUMNS, GRID_ROWS, Panel, Show, validate_name,
};

use super::Issues;
use super::compile::{CompiledHost, Document, DocumentKind, duplicates, param_problems};

const MIN_ROTATION_SECS: u64 = 5;
const MAX_ROTATION_SECS: u64 = 3600;
/// A dashboard has 12 × 6 cells; more panels than cells cannot fit without overlap.
pub(super) const MAX_PANELS: usize = GRID_COLUMNS as usize * GRID_ROWS as usize;

/// The id of a dashboard or display: its file name without `.json`.
fn stem(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(".json").unwrap_or(name)
}

fn to_body<T: serde::Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}

fn check_id(kind: &str, file: &str, issues: &mut Issues) {
    let id = stem(file);
    if validate_name(id).is_err() {
        issues.add(Some(file), || {
            format!(
                "{kind} id {id:?} (the file name without .json) is invalid: use 1 to 63 lowercase letters, digits, '_' or '-', starting with a letter or digit"
            )
        });
    }
}

/// Checks all dashboards and displays, appends issues and returns their documents.
pub(super) fn check(
    dashboards: &[(String, DashboardFile)],
    displays: &[(String, DisplayFile)],
    hosts: &[CompiledHost],
    issues: &mut Issues,
) -> Vec<Document> {
    let mut documents = Vec::new();

    duplicates(
        "dashboard id",
        dashboards.iter().map(|(f, _)| (stem(f), f.as_str())),
        issues,
    );
    duplicates(
        "dashboard name",
        dashboards
            .iter()
            .map(|(f, d)| (d.name.as_str(), f.as_str())),
        issues,
    );
    duplicates(
        "display id",
        displays.iter().map(|(f, _)| (stem(f), f.as_str())),
        issues,
    );
    duplicates(
        "display name",
        displays.iter().map(|(f, d)| (d.name.as_str(), f.as_str())),
        issues,
    );

    for (file, dashboard) in dashboards {
        check_id("dashboard", file, issues);
        check_dashboard(file, dashboard, hosts, issues);
        documents.push(Document {
            kind: DocumentKind::Dashboard,
            id: stem(file).to_owned(),
            source_file: file.clone(),
            body: to_body(dashboard),
        });
    }

    let dashboard_ids: BTreeSet<&str> = dashboards.iter().map(|(f, _)| stem(f)).collect();
    for (file, display) in displays {
        check_id("display", file, issues);
        check_display(file, display, &dashboard_ids, issues);
        documents.push(Document {
            kind: DocumentKind::Display,
            id: stem(file).to_owned(),
            source_file: file.clone(),
            body: to_body(display),
        });
    }
    documents
}

fn check_dashboard(
    file: &str,
    dashboard: &DashboardFile,
    hosts: &[CompiledHost],
    issues: &mut Issues,
) {
    let count = dashboard.panels.as_ref().map_or(0, Vec::len);
    if count > MAX_PANELS {
        // Not checked further: the file is wrong as a whole.
        issues.add(Some(file), || {
            format!(
                "the dashboard has {count} panels: the grid has {GRID_COLUMNS} × {GRID_ROWS} cells, so at most {MAX_PANELS} panels fit"
            )
        });
        return;
    }
    let mut rects: Vec<(usize, [u16; 4])> = Vec::new();
    for (index, panel) in dashboard.effective_panels().iter().enumerate() {
        let n = index + 1;
        let at = panel.placement();
        // Widened so that 250 + 250 cannot overflow.
        let (x, y, w, h) = (
            u16::from(at.x),
            u16::from(at.y),
            u16::from(at.w),
            u16::from(at.h),
        );
        let mut inside = true;
        if w == 0 || h == 0 {
            inside = false;
            issues.add(Some(file), || {
                format!("panel {n}: width and height must be at least 1 (got w={w}, h={h})")
            });
        }
        if x + w > u16::from(GRID_COLUMNS) {
            inside = false;
            issues.add(Some(file), || {
                format!(
                    "panel {n}: x + w is {} but the grid has {GRID_COLUMNS} columns",
                    x + w
                )
            });
        }
        if y + h > u16::from(GRID_ROWS) {
            inside = false;
            issues.add(Some(file), || {
                format!(
                    "panel {n}: y + h is {} but the grid has {GRID_ROWS} rows",
                    y + h
                )
            });
        }
        if inside {
            rects.push((n, [x, y, x + w, y + h]));
        }
        if let Panel::Graph {
            host, item, params, ..
        }
        | Panel::Value {
            host, item, params, ..
        } = panel
        {
            let series = Series {
                panel: n,
                host,
                item,
                params: params.as_ref(),
            };
            check_series_panel(file, &series, dashboard, hosts, issues);
        }
    }
    // The first overlap found is reported; one is enough to fix the layout.
    let overlap = rects.iter().enumerate().find_map(|(i, (a, ra))| {
        rects[i + 1..]
            .iter()
            .find(|(_, rb)| ra[0] < rb[2] && rb[0] < ra[2] && ra[1] < rb[3] && rb[1] < ra[3])
            .map(|(b, _)| (*a, *b))
    });
    if let Some((a, b)) = overlap {
        issues.add(Some(file), || format!("panels {a} and {b} overlap"));
    }
}

/// The data reference of a graph or value panel.
struct Series<'a> {
    panel: usize,
    host: &'a str,
    item: &'a str,
    params: Option<&'a BTreeMap<String, String>>,
}

fn check_series_panel(
    file: &str,
    series: &Series<'_>,
    dashboard: &DashboardFile,
    hosts: &[CompiledHost],
    issues: &mut Issues,
) {
    let Series {
        panel: n,
        host,
        item,
        params,
    } = *series;
    let Some(found) = hosts.iter().find(|h| h.name == host) else {
        issues.add(Some(file), || {
            format!("panel {n}: host {host:?} does not exist")
        });
        return;
    };
    if !dashboard.hosts.selects(&found.name, &found.groups) {
        issues.add(Some(file), || {
            format!("panel {n}: host {host:?} is not selected by this dashboard's \"hosts\"")
        });
    }
    if !found.effective.items.iter().any(|i| i.key == item) {
        issues.add(Some(file), || {
            format!("panel {n}: host {host:?} does not collect item {item:?}")
        });
    }
    if let Some(params) = params
        && !issues.is_full()
    {
        for problem in param_problems(item, params) {
            issues.add(Some(file), || format!("panel {n}: {problem}"));
        }
    }
}

fn check_display(
    file: &str,
    display: &DisplayFile,
    dashboard_ids: &BTreeSet<&str>,
    issues: &mut Issues,
) {
    let targets: &[String] = match &display.show {
        Show::Fixed { dashboard } => std::slice::from_ref(dashboard),
        Show::Rotate { rotate, every } => {
            if rotate.is_empty() {
                issues.add(Some(file), || {
                    "\"rotate\" is empty: list at least one dashboard".to_owned()
                });
            }
            let secs = every.as_secs();
            if !(MIN_ROTATION_SECS..=MAX_ROTATION_SECS).contains(&secs) {
                issues.add(Some(file), || {
                    format!("\"every\" is {secs} s: a rotation interval must be between 5s and 1h")
                });
            }
            rotate
        }
    };
    for target in targets {
        if issues.is_full() {
            return;
        }
        if !dashboard_ids.contains(target.as_str()) {
            issues.add(Some(file), || {
                format!(
                    "dashboard {target:?} does not exist (dashboards are referenced by file name without .json)"
                )
            });
        }
    }
    if let Some(allowed) = &display.dashboards {
        for name in allowed {
            if issues.is_full() {
                return;
            }
            if !dashboard_ids.contains(name.as_str()) {
                issues.add(Some(file), || {
                    format!("\"dashboards\" lists {name:?}, which does not exist")
                });
            }
        }
        // A set, so that long lists on both sides are not compared pairwise.
        let allowed: BTreeSet<&str> = allowed.iter().map(String::as_str).collect();
        for target in targets {
            if issues.is_full() {
                return;
            }
            if !allowed.contains(target.as_str()) {
                issues.add(Some(file), || {
                    format!("dashboard {target:?} is shown but not in the \"dashboards\" list")
                });
            }
        }
    }
}
