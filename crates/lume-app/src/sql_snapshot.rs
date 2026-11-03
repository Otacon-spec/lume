//! Conversione tra stato vivo SQL e [`lume_store::SqlConsoleSnapshot`].

use std::time::Duration;

use lume_sql::{ColumnInfo, ResultSet, Value};
use lume_store::{
    SQL_SNAPSHOT_MAX_BYTES, SQL_SNAPSHOT_MAX_ROWS, SqlCellSnapshot, SqlColumnSnapshot,
    SqlConsolePaneSnapshot, SqlConsoleSnapshot, SqlPaneStatusSnapshot, SqlResultSnapshot,
};

use crate::sql_mode::{Pane, PaneStatus, SqlWorkspace};

pub fn build_snapshot(workspace: &SqlWorkspace, editor_text: &str) -> SqlConsoleSnapshot {
    SqlConsoleSnapshot {
        editor_text: editor_text.to_owned(),
        compare: workspace.compare,
        sync_scroll: workspace.sync_scroll,
        panes: (0..2)
            .filter_map(|index| workspace.pane(index).map(pane_snapshot))
            .collect(),
    }
}

fn pane_snapshot(pane: &Pane) -> SqlConsolePaneSnapshot {
    let mut encoded_sets = Vec::new();
    let mut truncated = false;
    for index in 0..pane.result_set_count() {
        let Some(results) = pane.result_set(index) else {
            truncated = true;
            break;
        };
        match encode_results(results) {
            Ok(snapshot) => encoded_sets.push(snapshot),
            Err(()) => {
                truncated = true;
                break;
            }
        }
    }
    if serde_json::to_vec(&encoded_sets).map_or(true, |bytes| bytes.len() > SQL_SNAPSHOT_MAX_BYTES)
    {
        truncated = true;
    }
    let (status, results, additional_results) = if truncated {
        (SqlPaneStatusSnapshot::Truncated, None, Vec::new())
    } else {
        let mut encoded_sets = encoded_sets.into_iter();
        (
            pane_status(&pane.status),
            encoded_sets.next(),
            encoded_sets.collect(),
        )
    };
    SqlConsolePaneSnapshot {
        connection_id: pane.connection.clone(),
        page: pane.page,
        status,
        results,
        additional_results,
        selected_result_set: pane.selected_result_set(),
    }
}

fn pane_status(status: &PaneStatus) -> SqlPaneStatusSnapshot {
    match status {
        PaneStatus::Idle => SqlPaneStatusSnapshot::Idle,
        PaneStatus::Running => SqlPaneStatusSnapshot::Running,
        // Una pausa conserva un cursore vivo, non ripristinabile dopo restart.
        PaneStatus::Paused => SqlPaneStatusSnapshot::Truncated,
        PaneStatus::Ok => SqlPaneStatusSnapshot::Ok,
        PaneStatus::Error(message) => SqlPaneStatusSnapshot::Error(message.clone()),
    }
}

fn encode_results(results: &ResultSet) -> Result<SqlResultSnapshot, ()> {
    if results.rows.len() > SQL_SNAPSHOT_MAX_ROWS {
        return Err(());
    }
    let rows = results
        .rows
        .iter()
        .map(|row| row.iter().map(cell_snapshot).collect())
        .collect();
    let snapshot = SqlResultSnapshot {
        columns: results
            .columns
            .iter()
            .map(|column| SqlColumnSnapshot {
                name: column.name.clone(),
                type_name: column.type_name.clone(),
            })
            .collect(),
        rows,
        elapsed_ms: results.elapsed.as_millis() as u64,
    };
    let encoded = serde_json::to_string(&snapshot).map_err(|_| ())?;
    if encoded.len() > SQL_SNAPSHOT_MAX_BYTES {
        return Err(());
    }
    Ok(snapshot)
}

fn cell_snapshot(value: &Value) -> SqlCellSnapshot {
    match value {
        Value::Null => SqlCellSnapshot::Null,
        Value::Int(v) => SqlCellSnapshot::Int(*v),
        Value::Float(v) => SqlCellSnapshot::Float(*v),
        Value::Bool(v) => SqlCellSnapshot::Bool(*v),
        Value::Text(v) => SqlCellSnapshot::Text(v.clone()),
        Value::Bytes(v) => SqlCellSnapshot::Bytes(v.clone()),
    }
}

pub fn restore_workspace(snapshot: &SqlConsoleSnapshot) -> SqlWorkspace {
    let mut workspace = SqlWorkspace::new();
    workspace.compare = snapshot.compare;
    workspace.sync_scroll = snapshot.sync_scroll;
    for (index, pane) in snapshot.panes.iter().enumerate().take(2) {
        if let Some(slot) = workspace.pane_mut(index) {
            *slot = restore_pane(pane);
        }
    }
    workspace
}

fn restore_pane(pane: &SqlConsolePaneSnapshot) -> crate::sql_mode::Pane {
    use crate::sql_mode::Pane;
    let (status, result_sets) = match (&pane.status, &pane.results) {
        (SqlPaneStatusSnapshot::Truncated, _) => (
            PaneStatus::Error(crate::sql_mode::RESULT_NOT_RESTORED.to_owned()),
            None,
        ),
        (SqlPaneStatusSnapshot::Idle, _) => (PaneStatus::Idle, None),
        (SqlPaneStatusSnapshot::Running, _) => (PaneStatus::Running, None),
        (SqlPaneStatusSnapshot::Ok, results) => (
            PaneStatus::Ok,
            results
                .as_ref()
                .map(decode_results)
                .map(|first| vec![first]),
        ),
        (SqlPaneStatusSnapshot::Error(message), results) => (
            PaneStatus::Error(message.clone()),
            results
                .as_ref()
                .map(decode_results)
                .map(|first| vec![first]),
        ),
    };
    let mut restored = Pane {
        connection: pane.connection_id.clone(),
        status,
        connection_menu_open: false,
        query_gen: crate::sql_mode::NO_QUERY,
        ..Pane::default()
    };
    if let Some(mut result_sets) = result_sets {
        result_sets.extend(pane.additional_results.iter().map(decode_results));
        restored.restore_result_sets(result_sets, pane.selected_result_set);
        restored.set_page(pane.page);
    }
    restored
}

fn decode_results(snapshot: &SqlResultSnapshot) -> ResultSet {
    ResultSet {
        columns: snapshot
            .columns
            .iter()
            .map(|column| ColumnInfo {
                name: column.name.clone(),
                type_name: column.type_name.clone(),
            })
            .collect(),
        rows: snapshot
            .rows
            .iter()
            .map(|row| row.iter().map(decode_cell).collect())
            .collect(),
        elapsed: Duration::from_millis(snapshot.elapsed_ms),
    }
}

fn decode_cell(cell: &SqlCellSnapshot) -> Value {
    match cell {
        SqlCellSnapshot::Null => Value::Null,
        SqlCellSnapshot::Int(v) => Value::Int(*v),
        SqlCellSnapshot::Float(v) => Value::Float(*v),
        SqlCellSnapshot::Bool(v) => Value::Bool(*v),
        SqlCellSnapshot::Text(v) => Value::Text(v.clone()),
        SqlCellSnapshot::Bytes(v) => Value::Bytes(v.clone()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    fn result(name: &str, value: i64) -> ResultSet {
        ResultSet {
            columns: vec![ColumnInfo {
                name: name.into(),
                type_name: "INT".into(),
            }],
            rows: vec![vec![Value::Int(value)]],
            elapsed: Duration::from_millis(3),
        }
    }

    #[test]
    fn un_result_set_piccolo_sopravvive_al_giro_json() {
        let mut workspace = SqlWorkspace::new();
        workspace.pane_mut(0).unwrap().set_results(result("id", 1));
        workspace.pane_mut(0).unwrap().status = PaneStatus::Ok;
        let snapshot = build_snapshot(&workspace, "SELECT 1");
        let restored = restore_workspace(&snapshot);
        assert_eq!(
            restored
                .pane(0)
                .unwrap()
                .results
                .as_ref()
                .map(|set| set.row_count()),
            Some(1)
        );
    }

    #[test]
    fn snapshot_ripristina_tutti_i_result_set_e_quello_selezionato() {
        let mut workspace = SqlWorkspace::new();
        let pane = workspace.pane_mut(0).unwrap();
        pane.restore_result_sets(vec![result("first", 1), result("second", 2)], 1);
        pane.status = PaneStatus::Ok;

        let snapshot = build_snapshot(&workspace, "SELECT 1; SELECT 2");
        let mut restored = restore_workspace(&snapshot);
        let pane = restored.pane(0).unwrap();
        assert_eq!(pane.result_set_count(), 2);
        assert_eq!(pane.selected_result_set(), 1);
        assert_eq!(pane.results.as_ref().unwrap().columns[0].name, "second");
        let pane = restored.pane_mut(0).unwrap();
        assert!(pane.select_result_set(0));
        assert_eq!(pane.results.as_ref().unwrap().rows[0][0], Value::Int(1));
    }

    #[test]
    fn snapshot_legacy_senza_result_set_aggiuntivi_si_deserializza() {
        let legacy = serde_json::json!({
            "connection_id": null,
            "page": 0,
            "status": "Idle",
            "results": null
        });
        let pane: SqlConsolePaneSnapshot = serde_json::from_value(legacy).unwrap();
        assert!(pane.additional_results.is_empty());
        assert_eq!(pane.selected_result_set, 0);
    }

    #[test]
    fn un_result_set_enorme_viene_troncato() {
        let mut workspace = SqlWorkspace::new();
        let huge = "x".repeat(SQL_SNAPSHOT_MAX_BYTES + 1);
        workspace.pane_mut(0).unwrap().set_results(ResultSet {
            columns: vec![ColumnInfo {
                name: "txt".into(),
                type_name: "TEXT".into(),
            }],
            rows: vec![vec![Value::Text(huge)]],
            elapsed: Duration::from_millis(1),
        });
        let snapshot = build_snapshot(&workspace, "SELECT big");
        assert!(matches!(
            snapshot.panes[0].status,
            SqlPaneStatusSnapshot::Truncated
        ));
        assert!(snapshot.panes[0].results.is_none());
        let restored = restore_workspace(&snapshot);
        assert_eq!(
            restored.pane(0).unwrap().status,
            PaneStatus::Error(crate::sql_mode::RESULT_NOT_RESTORED.to_owned())
        );
        assert!(restored.pane(0).unwrap().results.is_none());
    }

    #[test]
    fn troppe_righe_non_si_salvano_in_silenzio() {
        let mut workspace = SqlWorkspace::new();
        workspace.pane_mut(0).unwrap().set_results(ResultSet {
            columns: vec![ColumnInfo {
                name: "id".into(),
                type_name: "INT".into(),
            }],
            rows: (0..=SQL_SNAPSHOT_MAX_ROWS)
                .map(|n| vec![Value::Int(n as i64)])
                .collect(),
            elapsed: Duration::from_millis(1),
        });
        workspace.pane_mut(0).unwrap().status = PaneStatus::Ok;
        let snapshot = build_snapshot(&workspace, "SELECT *");
        assert!(matches!(
            snapshot.panes[0].status,
            SqlPaneStatusSnapshot::Truncated
        ));
        let restored = restore_workspace(&snapshot);
        assert_eq!(
            restored.pane(0).unwrap().status,
            PaneStatus::Error(crate::sql_mode::RESULT_NOT_RESTORED.to_owned())
        );
    }
}
