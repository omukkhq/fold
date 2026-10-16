/// One checklist item as the container rule sees it. A linked item counts by its child's state, not by its tick: recompute ticks it in the same pass, and Done is final, so the two never disagree once the pass is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemFact {
    Step { ticked: bool },
    Linked { child_done: bool },
}

/// The stored facts that decide whether a task completes by its children.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContainerFacts {
    /// Every item of every checklist of the task.
    pub items: Vec<ItemFact>,
    /// A task workspace of the task exists in any state. Work done in the task itself must reach main through its own proposal, so such a task never completes by the rule.
    pub ever_had_workspace: bool,
    /// A completion record exists.
    pub completed: bool,
    pub archived: bool,
    /// The task is a human task, which only a person marks Done, so it never completes by the rule.
    pub human_task: bool,
}

/// True when the task has at least one linked item, every linked child is Done, every plain step is ticked, it never had a workspace, it is no human task, and it is neither completed nor archived. A parent that keeps an unticked plain step for its own work therefore completes through its own proposal instead.
pub fn completes_by_children(facts: &ContainerFacts) -> bool {
    if facts.completed || facts.archived || facts.ever_had_workspace || facts.human_task {
        return false;
    }
    let mut linked = false;
    for item in &facts.items {
        match *item {
            ItemFact::Step { ticked: false } | ItemFact::Linked { child_done: false } => {
                return false;
            }
            ItemFact::Linked { child_done: true } => linked = true,
            ItemFact::Step { ticked: true } => {}
        }
    }
    linked
}
