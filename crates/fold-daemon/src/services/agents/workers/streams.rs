use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use fold_core::stall::{CallKind, OpenCall};
use fold_core::{AgentName, Timestamp};

use crate::services::agents::claude::{ClaudeMessage, ContentBlock, UserContent};
use crate::support::lock;

/// What the supervisor reads of each running worker process's output, by the service clock, for the stall rule: the time of its newest line and the calls of its own turn that have no result yet. A subagent's lines count as output, but only the worker's own calls open and end. It lives in memory only, so a daemon that starts again counts from its start.
#[derive(Debug, Default)]
pub struct WorkerStreams {
    agents: Mutex<HashMap<AgentName, Stream>>,
}

#[derive(Debug)]
struct Stream {
    /// The process the entry belongs to, so the end of an older process never removes a newer one's.
    generation: u64,
    last_output_at: Timestamp,
    /// By Claude Code's id of the call.
    open: BTreeMap<String, OpenCall>,
}

/// A running process's stream as the stall rule reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamFacts {
    pub last_output_at: Timestamp,
    /// The oldest open call.
    pub open_call: Option<OpenCall>,
}

impl WorkerStreams {
    /// A process of `agent` started at `at`; it replaces whatever an older process left.
    pub(crate) fn started(&self, agent: &AgentName, generation: u64, at: Timestamp) {
        lock(&self.agents).insert(
            agent.clone(),
            Stream {
                generation,
                last_output_at: at,
                open: BTreeMap::new(),
            },
        );
    }

    /// The process `generation` of `agent` wrote `message` at `at`.
    pub(crate) fn line(
        &self,
        agent: &AgentName,
        generation: u64,
        at: Timestamp,
        message: &ClaudeMessage,
    ) {
        let mut agents = lock(&self.agents);
        let Some(stream) = agents
            .get_mut(agent)
            .filter(|stream| stream.generation == generation)
        else {
            return;
        };
        stream.last_output_at = stream.last_output_at.max(at);
        match message {
            ClaudeMessage::Assistant(assistant) if assistant.parent_tool_use_id.is_none() => {
                for block in &assistant.message.content {
                    if let ContentBlock::ToolUse { id, name, .. } = block {
                        stream.open.entry(id.clone()).or_insert(OpenCall {
                            kind: CallKind::of(name),
                            since: at,
                        });
                    }
                }
            }
            ClaudeMessage::User(user) if user.parent_tool_use_id.is_none() => {
                if let UserContent::Blocks(blocks) = &user.message.content {
                    for block in blocks {
                        if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                            stream.open.remove(tool_use_id);
                        }
                    }
                }
            }
            // A turn's result ends every call of the turn, also one whose result line never came.
            ClaudeMessage::Result(_) => stream.open.clear(),
            _ => {}
        }
    }

    /// The process `generation` of `agent` ended.
    pub(crate) fn ended(&self, agent: &AgentName, generation: u64) {
        let mut agents = lock(&self.agents);
        if agents
            .get(agent)
            .is_some_and(|stream| stream.generation == generation)
        {
            agents.remove(agent);
        }
    }

    /// The stream of the running process of `agent`; `None` when no process of it started since the daemon did.
    pub fn facts(&self, agent: &AgentName) -> Option<StreamFacts> {
        let agents = lock(&self.agents);
        let stream = agents.get(agent)?;
        Some(StreamFacts {
            last_output_at: stream.last_output_at,
            open_call: stream.open.values().min_by_key(|call| call.since).copied(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millis(millis).unwrap()
    }

    fn parse(line: &str) -> ClaudeMessage {
        ClaudeMessage::parse(line).unwrap()
    }

    fn tool_use(id: &str, name: &str, parent: Option<&str>) -> ClaudeMessage {
        let parent = parent.map_or("null".to_owned(), |parent| format!("\"{parent}\""));
        parse(&format!(
            r#"{{"type":"assistant","parent_tool_use_id":{parent},"message":{{"content":[{{"type":"tool_use","id":"{id}","name":"{name}","input":{{}}}}]}}}}"#
        ))
    }

    fn tool_result(id: &str) -> ClaudeMessage {
        parse(&format!(
            r#"{{"type":"user","parent_tool_use_id":null,"message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"{id}","content":"ok"}}]}}}}"#
        ))
    }

    #[test]
    fn a_call_is_open_from_its_use_to_its_result_and_every_line_is_output() {
        let streams = WorkerStreams::default();
        let agent = AgentName::parse("web-worker").unwrap();
        assert_eq!(streams.facts(&agent), None);
        streams.started(&agent, 1, at(0));
        assert_eq!(
            streams.facts(&agent),
            Some(StreamFacts {
                last_output_at: at(0),
                open_call: None
            })
        );
        streams.line(&agent, 1, at(10), &tool_use("a", "Bash", None));
        streams.line(&agent, 1, at(20), &tool_use("b", "Task", None));
        streams.line(&agent, 1, at(30), &tool_use("c", "Read", Some("b")));
        let facts = streams.facts(&agent).unwrap();
        assert_eq!(facts.last_output_at, at(30));
        assert_eq!(
            facts.open_call,
            Some(OpenCall {
                kind: CallKind::Command,
                since: at(10)
            }),
            "the oldest call; a subagent's own call opens nothing"
        );
        streams.line(&agent, 1, at(40), &tool_result("a"));
        assert_eq!(
            streams.facts(&agent).unwrap().open_call,
            Some(OpenCall {
                kind: CallKind::Subagent,
                since: at(20)
            })
        );
        streams.line(
            &agent,
            1,
            at(50),
            &parse(r#"{"type":"result","subtype":"success","is_error":false,"num_turns":1,"session_id":"s","total_cost_usd":0}"#),
        );
        assert_eq!(
            streams.facts(&agent),
            Some(StreamFacts {
                last_output_at: at(50),
                open_call: None
            }),
            "the turn's result ends its calls"
        );
    }

    #[test]
    fn only_the_current_process_changes_the_stream() {
        let streams = WorkerStreams::default();
        let agent = AgentName::parse("web-worker").unwrap();
        streams.started(&agent, 1, at(0));
        streams.started(&agent, 2, at(5));
        streams.line(&agent, 1, at(10), &tool_use("a", "Bash", None));
        assert_eq!(streams.facts(&agent).unwrap().open_call, None);
        streams.ended(&agent, 1);
        assert!(streams.facts(&agent).is_some(), "an older end keeps it");
        streams.ended(&agent, 2);
        assert_eq!(streams.facts(&agent), None);
        streams.line(&agent, 2, at(20), &tool_result("a"));
        assert_eq!(streams.facts(&agent), None);
    }
}
