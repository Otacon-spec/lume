//! Ponte a evento fra il ricevitore sintattico e la subscription Iced.
use iced::futures::{
    StreamExt,
    stream::{self, BoxStream},
};
use lume_syntax::{SyntaxCompletion, SyntaxLane, SyntaxReceiver};

#[derive(Clone, Debug)]
pub enum SyntaxEvent {
    Complete(SyntaxLane, SyntaxReceiver, SyntaxCompletion),
    Stopped(SyntaxLane, SyntaxReceiver),
}

impl SyntaxEvent {
    /// Verificare questo owner contro il ricevitore corrente prima di reagire.
    pub fn origin(&self) -> (SyntaxLane, &SyntaxReceiver) {
        match self {
            Self::Complete(lane, owner, _) | Self::Stopped(lane, owner) => (*lane, owner),
        }
    }
}

/// Identità della ricetta: lane + ricevitore, indipendente dalla revisione.
pub fn subscription(lane: SyntaxLane, receiver: SyntaxReceiver) -> iced::Subscription<SyntaxEvent> {
    iced::Subscription::run_with((lane, receiver), events)
}

fn events(key: &(SyntaxLane, SyntaxReceiver)) -> BoxStream<'static, SyntaxEvent> {
    let lane = key.0;
    let receiver = key.1.clone();
    stream::unfold(Some(receiver), move |state| async move {
        let receiver = state?;
        match std::future::poll_fn(|cx| receiver.poll_result(cx)).await {
            Some(completion) => Some((
                SyntaxEvent::Complete(lane, receiver.clone(), completion),
                Some(receiver),
            )),
            None => Some((SyntaxEvent::Stopped(lane, receiver), None)),
        }
    })
    .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::futures::StreamExt;
    use lume_core::{DocId, Document};
    use lume_syntax::{LanguageId, SyntaxPool, SyntaxRequest};

    #[test]
    fn stream_can_be_recreated_and_owner_shutdown_emits_terminal_event()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut pool = SyntaxPool::default();
        let receiver = pool.receiver(SyntaxLane::Primary);
        use iced::advanced::subscription::into_recipes;
        let recipe = into_recipes(subscription(SyntaxLane::Primary, receiver.clone()))
            .pop()
            .ok_or("missing recipe")?;
        let mut discarded = recipe.stream(Box::pin(iced::futures::stream::empty()));
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(discarded.as_mut().poll_next(&mut cx).is_pending());
        drop(discarded);
        let source = Document::from_str("fn main() {}\n", None, DocId(1))
            .rope()
            .clone();
        pool.submit(
            SyntaxLane::Primary,
            SyntaxRequest {
                document: 1,
                epoch: 7,
                revision: 1,
                language: LanguageId::Rust,
                window: 0..source.len_bytes(),
                source,
                edit: None,
            },
        )?;
        let expected_owner = receiver.clone();
        let recipe = into_recipes(subscription(SyntaxLane::Primary, receiver))
            .pop()
            .ok_or("missing replacement recipe")?;
        let stream = recipe.stream(Box::pin(iced::futures::stream::empty()));
        iced::futures::pin_mut!(stream);
        let event = iced::futures::executor::block_on(stream.next()).ok_or("missing completion")?;
        assert_eq!(event.origin(), (SyntaxLane::Primary, &expected_owner));
        assert!(
            matches!(event, SyntaxEvent::Complete(SyntaxLane::Primary, _, result) if result.document == 1 && result.result.is_some())
        );
        for collector in pool.shutdown() {
            assert!(collector.join().is_ok());
        }
        let terminal = iced::futures::executor::block_on(stream.next()).ok_or("missing stop")?;
        assert_eq!(terminal.origin(), (SyntaxLane::Primary, &expected_owner));
        assert!(matches!(
            terminal,
            SyntaxEvent::Stopped(SyntaxLane::Primary, _)
        ));
        let replacement = SyntaxPool::default();
        assert_ne!(
            terminal.origin().1,
            &replacement.receiver(SyntaxLane::Primary)
        );
        assert!(iced::futures::executor::block_on(stream.next()).is_none());
        Ok(())
    }
}
