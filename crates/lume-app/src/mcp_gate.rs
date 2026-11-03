use std::collections::VecDeque;

use lume_mcp::{Effect, Mode, Refusal, Target, Verdict, decide};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Waiting {
    pub id: u64,
    pub conn: u64,
    pub effect: Effect,
    pub target: Target,
    pub client: String,
    pub title: String,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    Run,
    Queued,
    Refused(Refusal),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decided {
    pub id: u64,
    pub verdict: Verdict,
}

#[derive(Debug, Default)]
pub struct Gate {
    queue: VecDeque<Waiting>,
}

impl Gate {
    pub fn admit(&mut self, mode: Mode, waiting: Waiting) -> Admission {
        match decide(mode, waiting.effect, waiting.target) {
            Verdict::Allow => Admission::Run,
            Verdict::Ask => {
                self.queue.push_back(waiting);
                Admission::Queued
            }
            Verdict::Refuse(reason) => Admission::Refused(reason),
        }
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
    pub fn front(&self) -> Option<&Waiting> {
        self.queue.front()
    }
    pub fn position(&self) -> Option<(usize, usize)> {
        (!self.queue.is_empty()).then_some((1, self.queue.len()))
    }
    pub fn resolve_front(&mut self) -> Option<Waiting> {
        self.queue.pop_front()
    }

    pub fn redecide(&mut self, mode: Mode) -> Vec<Decided> {
        let mut retained = VecDeque::new();
        let mut decided = Vec::new();
        while let Some(waiting) = self.queue.pop_front() {
            let verdict = decide(mode, waiting.effect, waiting.target);
            if verdict == Verdict::Ask {
                retained.push_back(waiting);
            } else {
                decided.push(Decided {
                    id: waiting.id,
                    verdict,
                });
            }
        }
        self.queue = retained;
        decided
    }

    pub fn remove(&mut self, id: u64) -> Option<Waiting> {
        let index = self.queue.iter().position(|waiting| waiting.id == id)?;
        self.queue.remove(index)
    }

    pub fn remove_conn(&mut self, conn: u64) -> Vec<Waiting> {
        let mut removed = Vec::new();
        let mut retained = VecDeque::new();
        while let Some(waiting) = self.queue.pop_front() {
            if waiting.conn == conn {
                removed.push(waiting);
            } else {
                retained.push_back(waiting);
            }
        }
        self.queue = retained;
        removed
    }

    pub fn drain(&mut self) -> Vec<Waiting> {
        self.queue.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use lume_mcp::{Effect, Mode, Refusal, Target, Verdict};

    fn waiting(id: u64, conn: u64, effect: Effect) -> Waiting {
        Waiting {
            id,
            conn,
            effect,
            target: Target::Inside,
            client: format!("client-{conn}"),
            title: format!("request {id}"),
            lines: vec![],
        }
    }

    #[test]
    fn admitted_runs_or_queues_by_mode() {
        let mut gate = Gate::default();
        assert_eq!(
            gate.admit(Mode::NonBrave, waiting(1, 1, Effect::Read)),
            Admission::Run
        );
        assert_eq!(
            gate.admit(Mode::Auto, waiting(2, 1, Effect::Modify)),
            Admission::Run
        );
        assert_eq!(
            gate.admit(Mode::NonBrave, waiting(3, 1, Effect::Modify)),
            Admission::Queued
        );
        assert_eq!(
            gate.admit(Mode::Auto, waiting(4, 1, Effect::Execute)),
            Admission::Queued
        );
        assert_eq!(gate.len(), 2);
    }

    #[test]
    fn forbidden_targets_are_refused_in_every_mode() {
        let mut gate = Gate::default();
        for mode in Mode::ALL {
            let mut outside = waiting(1, 1, Effect::Modify);
            outside.target = Target::Outside;
            assert_eq!(
                gate.admit(mode, outside),
                Admission::Refused(Refusal::Outside)
            );
            let mut git = waiting(2, 1, Effect::Modify);
            git.target = Target::GitDir;
            assert_eq!(gate.admit(mode, git), Admission::Refused(Refusal::GitDir));
        }
        assert!(gate.is_empty());
    }

    #[test]
    fn requests_are_fifo_and_only_front_is_resolved() {
        let mut gate = Gate::default();
        gate.admit(Mode::NonBrave, waiting(10, 1, Effect::Modify));
        gate.admit(Mode::NonBrave, waiting(11, 2, Effect::Execute));
        assert_eq!(gate.position(), Some((1, 2)));
        assert_eq!(gate.resolve_front().unwrap().id, 10);
        assert_eq!(gate.position(), Some((1, 1)));
        assert_eq!(gate.resolve_front().unwrap().id, 11);
        assert!(gate.resolve_front().is_none());
    }

    #[test]
    fn relaxing_mode_releases_eligible_requests_in_fifo_order() {
        let mut gate = Gate::default();
        gate.admit(Mode::NonBrave, waiting(1, 1, Effect::Modify));
        gate.admit(Mode::NonBrave, waiting(2, 1, Effect::Execute));
        gate.admit(Mode::NonBrave, waiting(3, 2, Effect::Modify));
        assert_eq!(
            gate.redecide(Mode::Auto),
            [
                Decided {
                    id: 1,
                    verdict: Verdict::Allow
                },
                Decided {
                    id: 3,
                    verdict: Verdict::Allow
                }
            ]
        );
        assert_eq!(gate.front().unwrap().id, 2);
        assert_eq!(
            gate.redecide(Mode::Brave),
            [Decided {
                id: 2,
                verdict: Verdict::Allow
            }]
        );
        assert!(gate.is_empty());
    }

    #[test]
    fn removing_connection_or_id_and_draining_preserve_queue_order() {
        let mut gate = Gate::default();
        gate.admit(Mode::NonBrave, waiting(1, 1, Effect::Modify));
        gate.admit(Mode::NonBrave, waiting(2, 2, Effect::Modify));
        gate.admit(Mode::NonBrave, waiting(3, 1, Effect::Execute));
        assert_eq!(
            gate.remove_conn(1).iter().map(|w| w.id).collect::<Vec<_>>(),
            [1, 3]
        );
        assert_eq!(gate.front().unwrap().id, 2);
        gate.admit(Mode::NonBrave, waiting(4, 3, Effect::Modify));
        assert_eq!(gate.remove(4).unwrap().id, 4);
        assert_eq!(gate.drain().iter().map(|w| w.id).collect::<Vec<_>>(), [2]);
        assert!(gate.is_empty());
    }
}
