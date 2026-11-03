//! Trasporto SQL bounded fino all'applicazione UI, senza reader thread.
use futures::{Stream, StreamExt, channel::oneshot};
use lume_sql::{QueryEvent, QueryEvents, QueryOrigin};
use std::{
    pin::Pin,
    sync::{Arc, Mutex},
};

/// Evento consumabile una volta, ack consumabile una volta. Clone di Message
/// non duplica i batch né permette una seconda applicazione della stessa riga.
#[derive(Clone)]
pub struct Delivery {
    event: Arc<Mutex<Option<QueryEvent>>>,
    ack: Arc<Mutex<Option<oneshot::Sender<bool>>>>,
    origin: Option<QueryOrigin>,
}
impl std::fmt::Debug for Delivery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqlDelivery").finish_non_exhaustive()
    }
}
impl Delivery {
    pub(crate) fn is_current(&self) -> bool {
        self.origin.as_ref().is_none_or(QueryOrigin::is_current)
    }
    pub(crate) fn take(&self) -> Option<QueryEvent> {
        self.event.lock().ok()?.take()
    }
    pub(crate) fn acknowledge(&self, accepted: bool) {
        if let Ok(mut slot) = self.ack.lock()
            && let Some(sender) = slot.take()
        {
            let _ = sender.send(accepted);
        }
    }
}

pub(crate) fn deliveries(events: QueryEvents) -> impl Stream<Item = Delivery> {
    let origin = events.origin();
    acknowledged(Box::pin(events), Some(origin))
}
struct State {
    source: Pin<Box<dyn Stream<Item = QueryEvent> + Send>>,
    previous: Option<oneshot::Receiver<bool>>,
    terminal: bool,
    eof: bool,
    origin: Option<QueryOrigin>,
}
fn terminal(event: &QueryEvent) -> bool {
    matches!(
        event,
        QueryEvent::Completed { .. } | QueryEvent::Stopped { .. } | QueryEvent::Failed(_)
    )
}
fn acknowledged(
    source: Pin<Box<dyn Stream<Item = QueryEvent> + Send>>,
    origin: Option<QueryOrigin>,
) -> impl Stream<Item = Delivery> {
    futures::stream::unfold(
        State {
            source,
            previous: None,
            terminal: false,
            eof: false,
            origin,
        },
        |mut state| async move {
            if let Some(ack) = state.previous.take()
                && !matches!(ack.await, Ok(true))
            {
                return None;
            }
            if state.eof {
                return None;
            }
            let event = match state.source.next().await {
                Some(event) => event,
                None if state
                    .origin
                    .as_ref()
                    .is_some_and(|origin| !origin.is_current()) =>
                {
                    return None;
                }
                None if !state.terminal => {
                    state.eof = true;
                    QueryEvent::Failed("Lo stream SQL è terminato senza esito finale: risultato incompleto, eventuali righe ricevute sono parziali".into())
                }
                None => return None,
            };
            state.terminal |= terminal(&event);
            let (sender, receiver) = oneshot::channel();
            state.previous = Some(receiver);
            let delivery = Delivery {
                event: Arc::new(Mutex::new(Some(event))),
                ack: Arc::new(Mutex::new(Some(sender))),
                origin: state.origin.clone(),
            };
            Some((delivery, state))
        },
    )
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use futures::FutureExt;
    pub(crate) fn stream(
        receiver: tokio::sync::mpsc::Receiver<QueryEvent>,
    ) -> Pin<Box<dyn Stream<Item = Delivery> + Send>> {
        let source = futures::stream::unfold(receiver, |mut receiver| async move {
            receiver.recv().await.map(|event| (event, receiver))
        });
        Box::pin(acknowledged(Box::pin(source), None))
    }
    fn collect(mut stream: Pin<Box<dyn Stream<Item = Delivery> + Send>>) -> Vec<QueryEvent> {
        futures::executor::block_on(async {
            let mut events = Vec::new();
            while let Some(packet) = stream.next().await {
                events.push(packet.take().unwrap());
                packet.acknowledge(true);
            }
            events
        })
    }
    #[test]
    fn eof_reale_dopo_batch_senza_terminale_non_diventa_successo() {
        let (sender, receiver) = tokio::sync::mpsc::channel(2);
        sender
            .try_send(QueryEvent::Started {
                columns: Vec::new(),
            })
            .unwrap();
        sender
            .try_send(QueryEvent::Batch(lume_sql::QueryBatch::new(vec![
                Vec::new(),
            ])))
            .unwrap();
        drop(sender);
        let received = collect(stream(receiver));
        assert_eq!(received.len(), 3);
        assert!(
            matches!(received.last(),Some(QueryEvent::Failed(text)) if text.contains("incompleto"))
        );
    }
    #[test]
    fn esito_reale_non_duplica_errori_o_successi_alla_chiusura() {
        for event in [
            QueryEvent::Completed {
                elapsed: std::time::Duration::ZERO,
            },
            QueryEvent::Stopped {
                rows: 0,
                accounted_bytes: 0,
            },
            QueryEvent::Failed("errore reale".into()),
        ] {
            let (sender, receiver) = tokio::sync::mpsc::channel(2);
            sender.try_send(event.clone()).unwrap();
            drop(sender);
            assert_eq!(collect(stream(receiver)), vec![event]);
        }
    }
    #[test]
    fn destinatario_chiuso_non_riceve_fallback_e_libera_receiver() {
        let (sender, receiver) = tokio::sync::mpsc::channel(2);
        sender
            .try_send(QueryEvent::Started {
                columns: Vec::new(),
            })
            .unwrap();
        let mut stream = stream(receiver);
        let packet = futures::executor::block_on(stream.next()).unwrap();
        packet.acknowledge(false);
        assert!(futures::executor::block_on(stream.next()).is_none());
        assert!(sender.is_closed());
    }
    #[test]
    fn senza_ack_non_si_drena_il_secondo_evento_nella_coda_iced() {
        let (sender, receiver) = tokio::sync::mpsc::channel(2);
        sender
            .try_send(QueryEvent::Started {
                columns: Vec::new(),
            })
            .unwrap();
        sender
            .try_send(QueryEvent::BudgetReached {
                rows: 1,
                accounted_bytes: 0,
            })
            .unwrap();
        let mut stream = stream(receiver);
        let first = futures::executor::block_on(stream.next()).unwrap();
        assert!(stream.next().now_or_never().is_none());
        sender
            .try_send(QueryEvent::Completed {
                elapsed: std::time::Duration::ZERO,
            })
            .unwrap();
        assert!(
            sender
                .try_send(QueryEvent::Stopped {
                    rows: 0,
                    accounted_bytes: 0
                })
                .is_err()
        );
        first.acknowledge(true);
        let next = futures::executor::block_on(stream.next()).unwrap();
        assert!(matches!(
            next.take(),
            Some(QueryEvent::BudgetReached { .. })
        ));
        next.acknowledge(false);
        assert!(futures::executor::block_on(stream.next()).is_none());
    }
    #[test]
    fn packet_clonato_non_duplica_payload_o_ack() {
        let (sender, receiver) = tokio::sync::mpsc::channel(2);
        sender
            .try_send(QueryEvent::Started {
                columns: Vec::new(),
            })
            .unwrap();
        let mut stream = stream(receiver);
        let packet = futures::executor::block_on(stream.next()).unwrap();
        let copy = packet.clone();
        assert!(packet.take().is_some());
        assert!(copy.take().is_none());
        copy.acknowledge(false);
        packet.acknowledge(true);
        assert!(futures::executor::block_on(stream.next()).is_none());
        assert!(sender.is_closed());
    }
    #[test]
    fn drop_task_e_packet_senza_ack_rilasciano_receiver() {
        let (sender, receiver) = tokio::sync::mpsc::channel(2);
        sender
            .try_send(QueryEvent::Started {
                columns: Vec::new(),
            })
            .unwrap();
        let mut first_stream = stream(receiver);
        let packet = futures::executor::block_on(first_stream.next()).unwrap();
        drop(packet);
        assert!(futures::executor::block_on(first_stream.next()).is_none());
        assert!(sender.is_closed());
        let (sender, receiver) = tokio::sync::mpsc::channel(2);
        let stream = stream(receiver);
        drop(stream);
        assert!(sender.is_closed());
    }
}
