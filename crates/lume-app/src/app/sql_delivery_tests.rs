//! Producer async → Message → modello → ack; nessun Driver/database.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use futures::{FutureExt, StreamExt};
fn app() -> Lume {
    let (mut app, _) = Lume::new_without_store();
    let mut sql = sql_mode::SqlWorkspace::new();
    let pane = sql.pane_mut(0).unwrap();
    pane.connection = Some("a".into());
    pane.query_gen = 7;
    pane.status = sql_mode::PaneStatus::Running;
    app.sql = Some(sql);
    app
}
#[test]
fn ack_ui_solo_dopo_applicazione_o_rigetto_e_scope_non_si_confonde() {
    for (connection, generation, accepted) in [("a", 7, true), ("altra", 7, false), ("a", 8, false)]
    {
        let mut app = app();
        let (sender, receiver) = tokio::sync::mpsc::channel(2);
        sender
            .try_send(lume_sql::QueryEvent::Started {
                columns: Vec::new(),
            })
            .unwrap();
        sender
            .try_send(lume_sql::QueryEvent::BudgetReached {
                rows: 1,
                accounted_bytes: 0,
            })
            .unwrap();
        let mut stream = crate::sql_events::tests::stream(receiver);
        let delivery = futures::executor::block_on(stream.next()).unwrap();
        assert!(stream.next().now_or_never().is_none());
        let _ = app.update(Message::SqlQueryEvent {
            connection: connection.into(),
            pane: 0,
            generation,
            delivery,
        });
        assert_eq!(
            app.sql.as_ref().unwrap().pane(0).unwrap().results.is_some(),
            accepted
        );
        let next = futures::executor::block_on(stream.next());
        if accepted {
            let next = next.unwrap();
            assert!(matches!(
                next.take(),
                Some(lume_sql::QueryEvent::BudgetReached { .. })
            ));
            next.acknowledge(false);
            assert!(futures::executor::block_on(stream.next()).is_none());
        } else {
            assert!(next.is_none());
        }
        assert!(sender.is_closed());
    }
}
#[test]
fn duplicato_del_message_non_appende_il_batch_due_volte() {
    let mut app = app();
    let (sender, receiver) = tokio::sync::mpsc::channel(2);
    sender
        .try_send(lume_sql::QueryEvent::Started {
            columns: Vec::new(),
        })
        .unwrap();
    sender
        .try_send(lume_sql::QueryEvent::Batch(lume_sql::QueryBatch::new(
            vec![vec![lume_sql::Value::Int(1)]],
        )))
        .unwrap();
    let mut stream = crate::sql_events::tests::stream(receiver);
    let first = futures::executor::block_on(stream.next()).unwrap();
    let _ = app.update(Message::SqlQueryEvent {
        connection: "a".into(),
        pane: 0,
        generation: 7,
        delivery: first,
    });
    let delivery = futures::executor::block_on(stream.next()).unwrap();
    let message = Message::SqlQueryEvent {
        connection: "a".into(),
        pane: 0,
        generation: 7,
        delivery,
    };
    let duplicate = message.clone();
    let _ = app.update(message);
    let _ = app.update(duplicate);
    assert_eq!(
        app.sql
            .as_ref()
            .unwrap()
            .pane(0)
            .unwrap()
            .results
            .as_ref()
            .unwrap()
            .rows
            .len(),
        1
    );
    drop(stream);
    assert!(sender.is_closed());
}

#[tokio::test]
async fn app_consegna_e_visualizza_i_result_set_multipli_separatamente() {
    let mut app = app();
    let (sender, receiver) = tokio::sync::mpsc::channel(2);
    let mut stream = crate::sql_events::tests::stream(receiver);
    let events = [
        lume_sql::QueryEvent::Started {
            columns: vec![lume_sql::ColumnInfo {
                name: "one".into(),
                type_name: "int".into(),
            }],
        },
        lume_sql::QueryEvent::Batch(lume_sql::QueryBatch::new(vec![vec![lume_sql::Value::Int(
            1,
        )]])),
        lume_sql::QueryEvent::ResultSetStarted {
            index: 1,
            columns: vec![lume_sql::ColumnInfo {
                name: "two".into(),
                type_name: "int".into(),
            }],
        },
        lume_sql::QueryEvent::Batch(lume_sql::QueryBatch::new(vec![vec![lume_sql::Value::Int(
            2,
        )]])),
        lume_sql::QueryEvent::Completed {
            elapsed: std::time::Duration::from_millis(7),
        },
    ];
    for event in events {
        sender.send(event).await.unwrap();
        let delivery = stream.next().await.unwrap();
        let _ = app.update(Message::SqlQueryEvent {
            connection: "a".into(),
            pane: 0,
            generation: 7,
            delivery,
        });
    }
    let pane = app.sql.as_mut().unwrap().pane_mut(0).unwrap();
    assert_eq!(pane.result_set_count(), 2);
    assert_eq!(
        pane.results
            .as_ref()
            .and_then(|result| result.rows.first())
            .and_then(|row| row.first()),
        Some(&lume_sql::Value::Int(2))
    );
    assert!(pane.select_result_set(0));
    assert_eq!(
        pane.results
            .as_ref()
            .and_then(|result| result.rows.first())
            .and_then(|row| row.first()),
        Some(&lume_sql::Value::Int(1))
    );
    assert_eq!(pane.status, sql_mode::PaneStatus::Ok);
}
