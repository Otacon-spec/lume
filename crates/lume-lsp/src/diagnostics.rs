use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
};

use lsp_types::{
    DiagnosticRelatedInformation, DiagnosticSeverity, NumberOrString, PublishDiagnosticsParams,
    Range, Uri,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ServerId(String);

impl ServerId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProblemId(pub u64);

#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    pub id: ProblemId,
    pub server_id: ServerId,
    pub uri: Uri,
    pub range: Range,
    pub severity: Option<DiagnosticSeverity>,
    pub code: Option<NumberOrString>,
    pub source: Option<String>,
    pub message: String,
    pub related: Vec<DiagnosticRelatedInformation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishResult {
    Applied,
    IgnoredStale,
}

#[derive(Debug, Clone)]
struct Generation {
    version: Option<i32>,
    problems: Vec<Problem>,
}

#[derive(Debug, Default)]
pub struct DiagnosticStore {
    generations: HashMap<(ServerId, String), Generation>,
}

impl DiagnosticStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn publish(
        &mut self,
        server_id: ServerId,
        params: PublishDiagnosticsParams,
    ) -> PublishResult {
        let key = (server_id.clone(), params.uri.as_str().to_owned());
        if self
            .generations
            .get(&key)
            .is_some_and(|current| is_stale(current.version, params.version))
        {
            return PublishResult::IgnoredStale;
        }
        let uri = params.uri;
        let problems = params
            .diagnostics
            .into_iter()
            .map(|diagnostic| {
                let related = diagnostic.related_information.unwrap_or_default();
                let id = problem_id(
                    &server_id,
                    &uri,
                    diagnostic.range,
                    diagnostic.severity,
                    diagnostic.code.as_ref(),
                    diagnostic.source.as_deref(),
                    &diagnostic.message,
                );
                Problem {
                    id,
                    server_id: server_id.clone(),
                    uri: uri.clone(),
                    range: diagnostic.range,
                    severity: diagnostic.severity,
                    code: diagnostic.code,
                    source: diagnostic.source,
                    message: diagnostic.message,
                    related,
                }
            })
            .collect();
        self.generations.insert(
            key,
            Generation {
                version: params.version,
                problems,
            },
        );
        PublishResult::Applied
    }

    pub fn snapshot(&self) -> Vec<Problem> {
        let mut problems = self
            .generations
            .values()
            .flat_map(|generation| generation.problems.iter().cloned())
            .collect::<Vec<_>>();
        sort_problems(&mut problems);
        problems
    }

    /// I problemi di un solo file, per chi disegna il documento aperto.
    pub fn for_uri(&self, uri: &Uri) -> Vec<Problem> {
        self.generations
            .iter()
            .filter(|((_, key), _)| key == uri.as_str())
            .flat_map(|(_, generation)| generation.problems.iter().cloned())
            .collect()
    }

    /// Unisce più negozi e applica lo stesso ordinamento dello snapshot singolo.
    pub fn merged_snapshot<'a>(stores: impl IntoIterator<Item = &'a Self>) -> Vec<Problem> {
        let mut problems = stores
            .into_iter()
            .flat_map(|store| store.generations.values())
            .flat_map(|generation| generation.problems.iter().cloned())
            .collect::<Vec<_>>();
        sort_problems(&mut problems);
        problems
    }

    /// Rilascia una generazione solo quando non contiene più problemi.
    pub fn release(&mut self, server_id: &ServerId, uri: &Uri) -> bool {
        let key = (server_id.clone(), uri.as_str().to_owned());
        if self
            .generations
            .get(&key)
            .is_some_and(|generation| generation.problems.is_empty())
        {
            self.generations.remove(&key);
            return true;
        }
        false
    }

    pub fn generation_count(&self) -> usize {
        self.generations.len()
    }

    pub fn clear_server(&mut self, server_id: &ServerId) {
        self.generations
            .retain(|(publisher, _), _| publisher != server_id);
    }
}

fn is_stale(current: Option<i32>, incoming: Option<i32>) -> bool {
    match (current, incoming) {
        (Some(_), None) => true,
        (Some(current), Some(incoming)) => incoming < current,
        _ => false,
    }
}

fn sort_problems(problems: &mut [Problem]) {
    problems.sort_by(|left, right| {
        severity_rank(left.severity)
            .cmp(&severity_rank(right.severity))
            .then_with(|| left.uri.as_str().cmp(right.uri.as_str()))
            .then_with(|| left.range.start.line.cmp(&right.range.start.line))
            .then_with(|| left.range.start.character.cmp(&right.range.start.character))
            .then_with(|| left.server_id.cmp(&right.server_id))
            .then_with(|| left.message.cmp(&right.message))
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn severity_rank(severity: Option<DiagnosticSeverity>) -> u8 {
    match severity {
        Some(DiagnosticSeverity::ERROR) => 0,
        Some(DiagnosticSeverity::WARNING) => 1,
        Some(DiagnosticSeverity::INFORMATION) => 2,
        Some(DiagnosticSeverity::HINT) => 3,
        _ => 4,
    }
}

#[allow(clippy::too_many_arguments)]
fn problem_id(
    server_id: &ServerId,
    uri: &Uri,
    range: Range,
    severity: Option<DiagnosticSeverity>,
    code: Option<&NumberOrString>,
    source: Option<&str>,
    message: &str,
) -> ProblemId {
    let mut hasher = DefaultHasher::new();
    server_id.hash(&mut hasher);
    uri.as_str().hash(&mut hasher);
    range.start.line.hash(&mut hasher);
    range.start.character.hash(&mut hasher);
    range.end.line.hash(&mut hasher);
    range.end.character.hash(&mut hasher);
    severity_rank(severity).hash(&mut hasher);
    format_args!("{code:?}").to_string().hash(&mut hasher);
    source.hash(&mut hasher);
    message.hash(&mut hasher);
    ProblemId(hasher.finish())
}
