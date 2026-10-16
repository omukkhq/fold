use fold_core::events::types;
use fold_core::messages::{
    MAX_CHOICE_DESCRIPTION_LEN, MAX_QUESTION_HEADER_LEN, chosen_answer, short_line,
};
use fold_core::roles::{PermissionBehavior, ask_answer_turn, permission_answer_turn};
use fold_core::{
    Actor, AgentName, Choice, Event, EventActor, FoldError, HumanName, IdKind, MessageBody,
    MessageChoices, MessageId, NewEvent, PermissionQuestionId, WorkerName, WorkerState, codes,
    policy,
};
use fold_store::agents::{self, NewQuestion, QuestionKind, QuestionRow};
use fold_store::{StoreError, counters, messages, projects};
use serde_json::{Value, json};

use super::super::output::ToolRequest;
use super::{Input, Process, Supervisor, lock};
use crate::services::agents::claude::{
    ASK_USER_QUESTION, AskUserQuestion, ClaudeInput, PermissionDecision,
};
use crate::services::agents::{AgentError, AgentService, main_actor};
use crate::services::events::EventBuffer;
use crate::services::messages::{MessageError, WorkerQuestion, ask_humans_in_tx};

/// A live `can_use_tool` request of one process that waits for a question's answer.
#[derive(Debug, Clone)]
pub(in crate::services::agents::workers) struct Waiter {
    agent: AgentName,
    generation: u64,
    request_id: String,
}

/// The tool error a worker gets when a human denied its call without a note.
const DENIED: &str = "the human denied this tool call";

/// The tool error of a call fold could not put to a human.
const NO_HUMAN: &str = "fold could not put this tool call to a human; continue without it";

impl From<MessageError> for AgentError {
    fn from(error: MessageError) -> Self {
        match error {
            MessageError::Fold(error) => Self::Fold(error),
            MessageError::Store(error) => Self::Store(error),
            MessageError::Event(error) => Self::Event(error),
        }
    }
}

fn same_call(question: &QuestionRow, request: &ToolRequest) -> bool {
    question.tool == request.call.tool_name && question.input == request.call.input
}

fn question_not_found(question: PermissionQuestionId) -> FoldError {
    FoldError::new(
        codes::PERMISSION_QUESTION_NOT_FOUND,
        format!("{question} does not exist"),
    )
    .with_details([("question", json!(question.to_string()))])
    .with_hint("list the open questions")
}

fn question_closed(question: PermissionQuestionId) -> FoldError {
    FoldError::new(
        codes::PERMISSION_QUESTION_CLOSED,
        format!("{question} is already answered or its worker stopped"),
    )
    .with_details([("question", json!(question.to_string()))])
    .with_hint("list the open questions")
}

impl Supervisor {
    /// Writes the answer to one live request of the process.
    fn reply(
        &self,
        agent: &AgentName,
        generation: u64,
        request_id: &str,
        decision: PermissionDecision,
    ) {
        let Ok(line) = ClaudeInput::permission(request_id, decision).to_line() else {
            return;
        };
        let input = lock(&self.inner.running)
            .get(agent)
            .filter(|run| run.generation == generation)
            .map(|run| run.input.clone());
        if let Some(input) = input {
            let _ = input.send(Input::Line(line));
        }
    }

    /// Drops the waiters of a process that has ended.
    pub(super) fn forget_waiters(&self, generation: u64) {
        let mut pending = lock(&self.inner.pending);
        for waiters in pending.values_mut() {
            waiters.retain(|waiter| waiter.generation != generation);
        }
        pending.retain(|_, waiters| !waiters.is_empty());
    }

    /// A `can_use_tool` request: the first repeat of a call whose request ended with its process takes the stored answer once; an open question for the same call takes it as one more waiter; otherwise it becomes a new question, so one answer never allows or denies a later call. A request fold cannot put to a human is denied, so the turn goes on.
    pub(super) async fn on_tool_request(&self, process: &Process, request: ToolRequest) {
        if let Err(error) = self.put_to_human(process, &request).await {
            tracing::warn!(agent = %process.agent, code = super::super::code_of(&error), "could not put a worker's tool call to a human");
            self.reply(
                &process.agent,
                process.generation,
                &request.request_id,
                PermissionDecision::Deny {
                    message: NO_HUMAN.to_owned(),
                },
            );
        }
    }

    async fn put_to_human(
        &self,
        process: &Process,
        request: &ToolRequest,
    ) -> Result<(), AgentError> {
        let questions = agents::questions_of_worker(self.inner.store.read(), process.id).await?;
        let replayed = {
            let mut replays = lock(&self.inner.replays);
            let found = replays.get_mut(&process.agent).and_then(|ids| {
                let at = ids.iter().position(|id| {
                    questions.iter().any(|question| {
                        question.id == *id
                            && question.answer.is_some()
                            && same_call(question, request)
                    })
                })?;
                Some(ids.remove(at))
            });
            replays.retain(|_, ids| !ids.is_empty());
            found
        };
        let answered = replayed.and_then(|id| questions.iter().find(|question| question.id == id));
        if let Some(question) = answered
            && let Some(decision) = self.decision(question, &request.call.input).await?
        {
            self.reply(
                &process.agent,
                process.generation,
                &request.request_id,
                decision,
            );
            return Ok(());
        }
        let waiter = Waiter {
            agent: process.agent.clone(),
            generation: process.generation,
            request_id: request.request_id.clone(),
        };
        if let Some(open) = questions
            .iter()
            .rev()
            .find(|question| same_call(question, request) && question.is_open())
        {
            #[cfg(feature = "failpoints")]
            fail::fail_point!("worker_question_before_waiter");
            return self.join(open.id, waiter, &request.call.input).await;
        }
        if request.call.tool_name == ASK_USER_QUESTION {
            self.ask_humans(process, request, waiter).await
        } else {
            self.ask_permission(process, request, waiter).await
        }
    }

    /// Adds `waiter` to an open question. An answer that came after the question was read has found no waiter here, so the question is read again: when it is answered and the waiter is still in place, the waiter takes it out and replies itself; a waiter that is gone was answered by [`Supervisor::deliver`].
    async fn join(&self, id: i64, waiter: Waiter, input: &Value) -> Result<(), AgentError> {
        lock(&self.inner.pending)
            .entry(id)
            .or_default()
            .push(waiter.clone());
        let Some(question) = agents::question_by_id(self.inner.store.read(), id).await? else {
            return Ok(());
        };
        if question.is_open() {
            return Ok(());
        }
        let taken = {
            let mut pending = lock(&self.inner.pending);
            let taken = pending.get_mut(&id).is_some_and(|waiters| {
                let before = waiters.len();
                waiters.retain(|other| {
                    other.generation != waiter.generation || other.request_id != waiter.request_id
                });
                waiters.len() < before
            });
            pending.retain(|_, waiters| !waiters.is_empty());
            taken
        };
        if taken && let Some(decision) = self.decision(&question, input).await? {
            self.reply(
                &waiter.agent,
                waiter.generation,
                &waiter.request_id,
                decision,
            );
        }
        Ok(())
    }

    /// A new `tool` question with `worker.permission_asked`. The waiter joins before the commit, so an answer that follows the commit at once finds it.
    async fn ask_permission(
        &self,
        process: &Process,
        request: &ToolRequest,
        waiter: Waiter,
    ) -> Result<(), AgentError> {
        let now = self.inner.clock.now();
        let mut buffer = EventBuffer::new();
        let mut joined = None;
        let written = self
            .inner
            .store
            .write(async |tx| {
                let number = counters::allocate(tx, IdKind::PermissionQuestion).await?;
                let number = PermissionQuestionId::from_number(number)
                    .ok_or(StoreError::Corrupt("counters.next"))?;
                let id = agents::insert_question(
                    tx.conn(),
                    &NewQuestion {
                        worker_process_id: process.id,
                        kind: QuestionKind::Tool,
                        number: Some(number),
                        session_id: &process.session_id,
                        request_id: &request.request_id,
                        tool_use_id: request.call.tool_use_id.as_deref(),
                        tool: &request.call.tool_name,
                        input: &request.call.input,
                        now,
                    },
                )
                .await?;
                let event =
                    NewEvent::new(types::WORKER_PERMISSION_ASKED, main_actor(&process.agent))?
                        .data(json!({
                            "question": number.to_string(),
                            "agent": process.agent.as_str(),
                            "tool": request.call.tool_name,
                        }))?;
                buffer.append(tx, &event, now).await?;
                lock(&self.inner.pending)
                    .entry(id)
                    .or_default()
                    .push(waiter.clone());
                joined = Some(id);
                Ok::<_, AgentError>(())
            })
            .await;
        if let Err(error) = written {
            if let Some(id) = joined {
                lock(&self.inner.pending).remove(&id);
            }
            return Err(error);
        }
        self.inner.bus.publish(buffer);
        Ok(())
    }

    /// A new `ask` question: each question of the `AskUserQuestion` call goes to every human as a question message from the worker, with the call's options as choices, linked to the worker's task when it has exactly one.
    async fn ask_humans(
        &self,
        process: &Process,
        request: &ToolRequest,
        waiter: Waiter,
    ) -> Result<(), AgentError> {
        let asked = request
            .call
            .questions()
            .filter(|asked| !asked.questions.is_empty());
        let registry = lock(&self.inner.registry).clone();
        let (Some(asked), Some(registry)) = (asked, registry) else {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                "fold cannot ask anyone this question",
            )
            .into());
        };
        let humans = projects::list_humans(registry.read()).await?;
        let mut bodies = Vec::with_capacity(asked.questions.len());
        for question in &asked.questions {
            // A description or header that fold would refuse is left out, so the question keeps its options.
            let options: Vec<Choice> = question
                .options
                .iter()
                .map(|option| Choice {
                    label: option.label.clone(),
                    description: option
                        .description
                        .as_deref()
                        .and_then(|text| short_line(text, MAX_CHOICE_DESCRIPTION_LEN)),
                })
                .collect();
            let header = question
                .header
                .as_deref()
                .and_then(|text| short_line(text, MAX_QUESTION_HEADER_LEN));
            bodies.push((
                MessageBody::parse(question.question.clone())?,
                MessageChoices::parse(&options, header.as_deref()).ok(),
            ));
        }
        let worker = WorkerName::main(&process.agent);
        let now = self.inner.clock.now();
        let mut buffer = EventBuffer::new();
        let mut joined = None;
        let written = self
            .inner
            .store
            .write(async |tx| {
                let row = agents::find_worker_process(tx.conn(), &process.agent)
                    .await?
                    .ok_or(StoreError::Corrupt("worker_processes"))?;
                let task = match row.tasks.as_slice() {
                    [task] => Some(*task),
                    _ => None,
                };
                let id = agents::insert_question(
                    tx.conn(),
                    &NewQuestion {
                        worker_process_id: process.id,
                        kind: QuestionKind::Ask,
                        number: None,
                        session_id: &process.session_id,
                        request_id: &request.request_id,
                        tool_use_id: request.call.tool_use_id.as_deref(),
                        tool: &request.call.tool_name,
                        input: &request.call.input,
                        now,
                    },
                )
                .await?;
                for (position, (body, choices)) in (1_i64..).zip(&bodies) {
                    let sent = ask_humans_in_tx(
                        tx,
                        &mut buffer,
                        WorkerQuestion {
                            humans: humans.clone(),
                            worker: &worker,
                            task,
                            body,
                            choices: choices.as_ref(),
                            now,
                        },
                    )
                    .await?;
                    agents::insert_question_message(tx.conn(), id, position, sent.id).await?;
                }
                lock(&self.inner.pending)
                    .entry(id)
                    .or_default()
                    .push(waiter.clone());
                joined = Some(id);
                Ok::<_, AgentError>(())
            })
            .await;
        if let Err(error) = written {
            if let Some(id) = joined {
                lock(&self.inner.pending).remove(&id);
            }
            return Err(error);
        }
        self.inner.bus.publish(buffer);
        Ok(())
    }

    /// The decision an answered question gives a request with `input`: the human's behavior for a `tool` question, the humans' replies for an `ask` question.
    async fn decision(
        &self,
        question: &QuestionRow,
        input: &Value,
    ) -> Result<Option<PermissionDecision>, AgentError> {
        let Some(answer) = &question.answer else {
            return Ok(None);
        };
        Ok(match (question.kind, answer.behavior) {
            (QuestionKind::Tool, PermissionBehavior::Allow) => Some(PermissionDecision::Allow {
                updated_input: input.clone(),
            }),
            (QuestionKind::Tool, PermissionBehavior::Deny) => Some(PermissionDecision::Deny {
                message: answer
                    .message
                    .clone()
                    .filter(|note| !note.trim().is_empty())
                    .unwrap_or_else(|| DENIED.to_owned()),
            }),
            (QuestionKind::Ask, _) => self.ask_answers(question).await?.map(|(answers, _)| {
                PermissionDecision::answer_questions(
                    input,
                    answers
                        .iter()
                        .map(|(question, answer)| (question.as_str(), answer.as_str())),
                )
            }),
        })
    }

    /// Each question of an `ask` question with the first human reply to its message, and the human who replied last; `None` while a message has no reply.
    async fn ask_answers(
        &self,
        question: &QuestionRow,
    ) -> Result<Option<(Vec<(String, String)>, HumanName)>, AgentError> {
        let Ok(asked) = serde_json::from_value::<AskUserQuestion>(question.input.clone()) else {
            return Ok(None);
        };
        let ids = agents::question_messages(self.inner.store.read(), question.id).await?;
        let mut answers = Vec::with_capacity(ids.len());
        let mut last = None;
        for (asked, message) in asked.questions.iter().zip(ids) {
            let Some((body, human)) =
                messages::first_human_reply(self.inner.store.read(), message).await?
            else {
                return Ok(None);
            };
            let labels: Vec<String> = asked
                .options
                .iter()
                .map(|option| option.label.clone())
                .collect();
            answers.push((asked.question.clone(), chosen_answer(&labels, &body)));
            last = Some(human);
        }
        Ok(last.map(|human| (answers, human)))
    }

    /// Sends an answered question's decision to every live request that waits for it. A worker whose process ended while it waited, at a daemon stop, resumes with the answer as its first turn, and its first repeat of the call takes the stored answer.
    pub(in crate::services::agents::workers) async fn deliver(
        &self,
        question: &QuestionRow,
    ) -> Result<(), AgentError> {
        let waiters = lock(&self.inner.pending)
            .remove(&question.id)
            .unwrap_or_default();
        let live: Vec<Waiter> = waiters
            .into_iter()
            .filter(|waiter| {
                lock(&self.inner.running)
                    .get(&waiter.agent)
                    .is_some_and(|run| run.generation == waiter.generation)
            })
            .collect();
        if !live.is_empty() {
            for waiter in live {
                if let Some(decision) = self.decision(question, &question.input).await? {
                    self.reply(
                        &waiter.agent,
                        waiter.generation,
                        &waiter.request_id,
                        decision,
                    );
                }
            }
            return Ok(());
        }
        let Some(turn) = self.answer_turn(question).await? else {
            return Ok(());
        };
        if let Some(run) = lock(&self.inner.running).get(&question.agent) {
            self.replay(question);
            let _ = run.input.send(Input::Turn(turn));
            return Ok(());
        }
        let mut conn = self
            .inner
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let Some(row) = agents::find_worker_process(&mut conn, &question.agent).await? else {
            return Ok(());
        };
        drop(conn);
        if !matches!(row.state, WorkerState::Working | WorkerState::Idle) {
            return Ok(());
        }
        let now = self.inner.clock.now();
        self.inner
            .store
            .write(async |tx| agents::restart_worker_process(tx.conn(), row.id, now).await)
            .await?;
        self.replay(question);
        if let Some(watch) = self.launch(&row, true, Some(turn)).await? {
            tokio::spawn(self.clone().restart_lost(row.agent.clone(), watch, None));
        }
        Ok(())
    }

    /// Keeps an answered question for the first repeat of its call by the resumed worker.
    fn replay(&self, question: &QuestionRow) {
        lock(&self.inner.replays)
            .entry(question.agent.clone())
            .or_default()
            .push(question.id);
    }

    /// The user turn that tells a resumed worker the answer to its question.
    async fn answer_turn(&self, question: &QuestionRow) -> Result<Option<String>, AgentError> {
        let Some(answer) = &question.answer else {
            return Ok(None);
        };
        Ok(match question.kind {
            QuestionKind::Tool => Some(permission_answer_turn(
                &question.tool,
                &question.input,
                answer.behavior == PermissionBehavior::Allow,
                answer.message.as_deref(),
            )),
            QuestionKind::Ask => self
                .ask_answers(question)
                .await?
                .map(|(answers, _)| ask_answer_turn(&answers)),
        })
    }

    /// Whether a worker that the daemon's restart would resume waits for an answer instead. An `ask` question whose messages all have their replies is answered here, which resumes the worker with the answers.
    pub(super) async fn waits_for_answer(
        &self,
        row: &fold_store::agents::WorkerProcessRow,
    ) -> Result<bool, AgentError> {
        let questions = agents::questions_of_worker(self.inner.store.read(), row.id).await?;
        let open: Vec<&QuestionRow> = questions
            .iter()
            .filter(|question| question.is_open())
            .collect();
        if open.is_empty() {
            return Ok(false);
        }
        self.inner
            .store
            .write(async |tx| agents::set_worker_pid(tx.conn(), row.id, None).await)
            .await?;
        for question in open {
            if question.kind == QuestionKind::Ask {
                self.complete_ask(question.id).await?;
            }
        }
        Ok(true)
    }

    /// Reads a stored event for the question flow: a human message in the thread of an open `ask` question may complete its answers.
    pub(super) async fn on_event(&self, event: &Event) {
        if event.event_type != types::MESSAGE_SENT || !event.actor.as_str().starts_with("human:") {
            return;
        }
        let Some(thread) = event
            .data
            .get("thread")
            .and_then(Value::as_str)
            .and_then(|thread| thread.parse::<MessageId>().ok())
        else {
            return;
        };
        let found = async {
            let mut conn = self
                .inner
                .store
                .read()
                .acquire()
                .await
                .map_err(StoreError::from)?;
            agents::open_ask_of_thread(&mut conn, thread).await
        }
        .await;
        let question = match found {
            Ok(Some(question)) => question,
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(
                    seq = event.seq,
                    code = error.code().unwrap_or(codes::INTERNAL_ERROR),
                    "could not read a worker's question"
                );
                return;
            }
        };
        if let Err(error) = self.complete_ask(question).await {
            tracing::warn!(
                seq = event.seq,
                code = super::super::code_of(&error),
                "could not answer a worker's question"
            );
        }
    }

    /// Answers every open `ask` question whose messages all have their replies, for a watcher that may have missed the replies' events.
    pub(super) async fn answer_open_asks(&self) {
        let open = match agents::open_questions(self.inner.store.read()).await {
            Ok(open) => open,
            Err(error) => {
                tracing::warn!(
                    code = error.code().unwrap_or(codes::INTERNAL_ERROR),
                    "could not read the open questions"
                );
                return;
            }
        };
        for question in open {
            if question.kind != QuestionKind::Ask {
                continue;
            }
            if let Err(error) = self.complete_ask(question.id).await {
                tracing::warn!(
                    agent = %question.agent,
                    code = super::super::code_of(&error),
                    "could not answer a worker's question"
                );
            }
        }
    }

    /// Answers an open `ask` question once each of its messages has a human reply, then delivers the answers.
    async fn complete_ask(&self, id: i64) -> Result<(), AgentError> {
        let Some(question) = agents::question_by_id(self.inner.store.read(), id).await? else {
            return Ok(());
        };
        if !question.is_open() {
            return Ok(());
        }
        let Some((_, human)) = self.ask_answers(&question).await? else {
            return Ok(());
        };
        let now = self.inner.clock.now();
        self.inner
            .store
            .write(async |tx| {
                agents::insert_answer(tx.conn(), id, PermissionBehavior::Allow, &human, None, now)
                    .await
            })
            .await?;
        let Some(answered) = agents::question_by_id(self.inner.store.read(), id).await? else {
            return Ok(());
        };
        self.deliver(&answered).await
    }
}

impl AgentService {
    /// Every item that waits for a human, oldest first.
    pub async fn needs_you(
        &self,
    ) -> Result<Vec<crate::services::agents::NeedsYouItem>, AgentError> {
        Ok(crate::services::agents::needs_you(&self.store).await?)
    }

    /// The open permission questions of every worker, oldest first. `AskUserQuestion` calls are not among them: they are question messages.
    pub async fn open_permission_questions(&self) -> Result<Vec<QuestionRow>, AgentError> {
        Ok(agents::open_questions(self.store.read())
            .await?
            .into_iter()
            .filter(|question| question.kind == QuestionKind::Tool)
            .collect())
    }

    /// A named human's answer to an open permission question, with `worker.permission_answered` in the same transaction. After the commit the answer goes to the worker: as the `control_response` to its waiting request, or, when its process ended at a daemon stop, as the first turn of its resumed session. A second answer, or an answer after fold stopped the worker, is `permission_question_closed`.
    pub async fn answer_worker_permission(
        &self,
        actor: &Actor,
        question: PermissionQuestionId,
        behavior: PermissionBehavior,
        message: Option<String>,
    ) -> Result<QuestionRow, AgentError> {
        policy::can_answer_worker_permission(actor).map_err(FoldError::from)?;
        let Actor::Human { name, .. } = actor else {
            return Err(FoldError::new(codes::HUMAN_REQUIRED, "only a human answers").into());
        };
        let supervisor = self.supervisor()?.clone();
        let message = message.filter(|note| !note.trim().is_empty());
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let answered = self
            .store
            .write(async |tx| {
                let row = agents::find_question(tx.conn(), question)
                    .await?
                    .filter(|row| row.kind == QuestionKind::Tool)
                    .ok_or_else(|| question_not_found(question))?;
                if !row.is_open() {
                    return Err(question_closed(question).into());
                }
                agents::insert_answer(tx.conn(), row.id, behavior, name, message.as_deref(), now)
                    .await?;
                let event =
                    NewEvent::new(types::WORKER_PERMISSION_ANSWERED, EventActor::human(name))?
                        .data(json!({
                            "question": question.to_string(),
                            "agent": row.agent.as_str(),
                            "tool": row.tool,
                            "behavior": behavior.as_str(),
                        }))?;
                buffer.append(tx, &event, now).await?;
                agents::find_question(tx.conn(), question)
                    .await?
                    .ok_or(StoreError::Corrupt("worker_permission_questions"))
                    .map_err(AgentError::from)
            })
            .await?;
        self.bus.publish(buffer);
        supervisor.deliver(&answered).await?;
        Ok(answered)
    }
}
