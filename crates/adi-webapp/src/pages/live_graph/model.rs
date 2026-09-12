//! What the live graph is a graph *of*: the nodes, the edges between them, and where each one sits.
//!
//! Built from the two listings the panel already carries — `/api/agents` (what is defined) and
//! `/api/agents/runs/all` (what was done) — so the page costs no new endpoint, the same way the
//! analytics page is built. The join is over agent *names* and deliberately outer on both sides: an
//! agent that has never run is still an agent, and a conversation whose agent has since been
//! deleted still happened.
//!
//! **What the store cannot tell us, and what that means for the picture.** A run records which
//! *agent* asked for it (`launched_by: agent:<name>`), never which of that agent's conversations
//! did. So the chain is drawn through the agent — a chat, the agent that ran it, the chats that
//! agent started — and an edge from an agent to a chat means "some conversation of this agent
//! asked for that one", which is the most the record supports.
//!
//! Everything here is pure: listings and options in, a laid-out [`Graph`] out. The page re-runs it
//! when the data or the toggles change and draws whatever comes back — panning and zooming never
//! touch it.

use std::collections::HashMap;

use adi_webapp_api::types::{AgentDto, AgentRunInfo, AgentRuns, AllAgentRuns, LAUNCHED_BY_HUMAN};

/// The rest of `adi_agents::launcher`'s vocabulary. Only the word for a person is on the wire
/// already ([`LAUNCHED_BY_HUMAN`]); the panel cannot link that crate, so the other two are mirrored
/// here. An empty `launched_by` is deliberately none of them — see [`Origin::Unknown`].
const AUTOMATION: &str = "automation";
const AGENT_PREFIX: &str = "agent:";

/// How many of an agent's conversations the graph draws, newest first.
///
/// The listing carries up to fifty per agent and a busy machine has eighty agents: every one of
/// them on the canvas is four thousand boxes, which is not a picture of anything. Six is enough to
/// show that an agent is busy and which way its work flows; the count of what was cut is on the
/// page, because a graph that quietly shows a tenth of the machine is worse than a smaller one.
pub(crate) const CHATS_PER_AGENT: usize = 6;

/// Node geometry, in world units. One height for every node: a row of boxes that disagree about
/// their height reads as a chart of something, and nothing here is a quantity.
pub(crate) const NODE_H: f64 = 34.0;
pub(crate) const AGENT_W: f64 = 190.0;
pub(crate) const CHAT_W: f64 = 250.0;
pub(crate) const TOOL_W: f64 = 160.0;
pub(crate) const ORIGIN_W: f64 = 150.0;

/// Distance between the centres of two columns, and of two rows. The column pitch leaves 70 units
/// between the widest node and the next column — room for an edge to curve through rather than
/// past.
pub(crate) const COL_PITCH: f64 = 320.0;
pub(crate) const ROW_PITCH: f64 = 48.0;

/// How many relaxation passes the layering makes before it stops. Agents can start each other's
/// conversations, so the graph is not always acyclic and "until nothing changes" is not always a
/// thing that happens.
const MAX_PASSES: usize = 64;

/// What a node stands for. The kind decides its shape, its width and its tone — nothing else about
/// a node says which of these it is, because a word on every box would be the same word on most of
/// them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Origin,
    Agent,
    Chat,
    Tool,
}

impl Kind {
    pub(crate) const fn width(self) -> f64 {
        match self {
            Kind::Origin => ORIGIN_W,
            Kind::Agent => AGENT_W,
            Kind::Chat => CHAT_W,
            Kind::Tool => TOOL_W,
        }
    }
}

/// Where work came from when it came from outside the graph.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Origin {
    /// `launched_by: human` — somebody asked for it.
    Human,
    /// `launched_by: automation` — a trigger, a schedule, a script.
    Automation,
    /// `launched_by: ""` — nobody wrote it down. Its own root rather than folded into the person's:
    /// on this machine most history predates the record, and attributing it to somebody would put a
    /// year of agent-spawned work under their name.
    Unknown,
}

impl Origin {
    const fn id(self) -> &'static str {
        match self {
            Origin::Human => "origin:human",
            Origin::Automation => "origin:automation",
            Origin::Unknown => "origin:unknown",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Origin::Human => "You",
            Origin::Automation => "Automation",
            Origin::Unknown => "Unrecorded",
        }
    }

    const fn meta(self) -> &'static str {
        match self {
            Origin::Human => "Conversations a person asked for",
            Origin::Automation => "Conversations a trigger or a script started",
            Origin::Unknown => "Conversations opened before the store recorded who asked",
        }
    }
}

/// The one signal a node carries beside its name: a 6px dot, in the colour of what it is saying.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mark {
    None,
    /// Running right now.
    Running,
    /// Stopped with a question nobody has answered.
    Waiting,
    /// Ended badly.
    Failed,
}

/// What clicking a node does. A node with nothing to open is not a dead link: it simply does not
/// take the pointer (see [`crate::pages::live_graph`]'s hit test).
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum Action {
    None,
    /// Open this agent's editor.
    Agent(String),
    /// Open this conversation on the Agents page.
    Chat {
        agent: String,
        run_id: String,
        interactive: bool,
    },
}

/// One box on the canvas, already placed. `x`/`y` are its centre, in world units.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Node {
    pub(crate) id: String,
    /// What is drawn in the box, before it is cut to fit.
    pub(crate) label: String,
    /// What the head says while the pointer is on it — the whole of what a hover has to tell.
    pub(crate) meta: String,
    pub(crate) kind: Kind,
    pub(crate) mark: Mark,
    pub(crate) action: Action,
    pub(crate) layer: usize,
    pub(crate) x: f64,
    pub(crate) y: f64,
}

impl Node {
    pub(crate) const fn w(&self) -> f64 {
        self.kind.width()
    }

    /// Whether a world point is inside this box.
    pub(crate) fn hit(&self, x: f64, y: f64) -> bool {
        (x - self.x).abs() <= self.w() / 2.0 && (y - self.y).abs() <= NODE_H / 2.0
    }
}

/// A directed edge, by node index: `from` asked for, ran, or may run `to`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Edge {
    pub(crate) from: usize,
    pub(crate) to: usize,
}

/// What to draw. Every one of these is a toggle on the page, and each only ever *adds* to the
/// picture — nothing here hides something another option would have shown.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Options {
    /// Draw conversations, not only the agents they belong to. Off, the chain collapses onto the
    /// agents: an edge then means "this agent started work that one did".
    pub(crate) chats: bool,
    /// Draw the tools each agent may run, in a column of their own at the end.
    pub(crate) tools: bool,
    /// Keep agents that have never run. They have nothing to connect to, so they stand in a column
    /// of their own — which is the answer to "what is defined here that nothing uses".
    pub(crate) idle: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            chats: true,
            tools: false,
            idle: false,
        }
    }
}

/// The laid-out graph, plus what had to be left out to lay it out.
#[derive(Clone, Default, PartialEq, Debug)]
pub(crate) struct Graph {
    pub(crate) nodes: Vec<Node>,
    pub(crate) edges: Vec<Edge>,
    /// Conversations drawn, and conversations there are. Equal when nothing was cut.
    pub(crate) shown_chats: usize,
    pub(crate) total_chats: usize,
    pub(crate) agents: usize,
    /// The bounding box of every node, centre to centre plus half a box: what **Fit** fits.
    pub(crate) extent: (f64, f64, f64, f64),
}

impl Graph {
    pub(crate) fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The node under a world point, topmost last — the reverse of drawing order, so a box drawn
    /// over another is the one that answers for the pixel.
    pub(crate) fn at(&self, x: f64, y: f64) -> Option<usize> {
        self.nodes.iter().rposition(|n| n.hit(x, y))
    }

    /// The line under the title: how much of the machine this picture is.
    pub(crate) fn summary(&self) -> String {
        let agents = plural(self.agents, "agent", "agents");
        let chats = plural(self.total_chats, "conversation", "conversations");
        match (self.total_chats, self.shown_chats) {
            (0, _) => agents,
            // Chats turned off. "0 of 5" would be arithmetic where the answer is a sentence: the
            // conversations are what the links between the agents were derived from.
            (_, 0) => format!("{agents} · {chats}, none drawn"),
            (total, shown) if shown < total => {
                format!("{agents} · {shown} of {total} conversations")
            }
            _ => format!("{agents} · {chats}"),
        }
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Build and lay out the graph.
pub(crate) fn build(agents: &[AgentDto], chats: &AllAgentRuns, o: Options) -> Graph {
    let mut b = Builder::default();

    // Every agent that has run something, and — unless they are asked for — nothing else. An agent
    // with no runs has no edge to anything, so a machine with eighty definitions and six busy ones
    // would otherwise open as a column of boxes with the graph hiding behind it.
    let ran: HashMap<&str, &AgentRuns> = chats
        .agents
        .iter()
        .map(|a| (a.name.as_str(), a))
        .filter(|(_, a)| !a.runs.is_empty())
        .collect();

    // Defined agents first, in the order the listing sorted them (by name), then any agent the
    // history names that no longer exists — the outer half of the join.
    let mut named: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
    named.extend(
        chats
            .agents
            .iter()
            .map(|a| a.name.as_str())
            .filter(|n| !agents.iter().any(|a| a.name == *n)),
    );

    let defined: HashMap<&str, &AgentDto> = agents.iter().map(|a| (a.name.as_str(), a)).collect();

    for name in named {
        let runs = ran.get(name);
        if runs.is_none() && !o.idle {
            continue;
        }
        let count = runs.map_or(0, |r| r.runs.len());
        let meta = match (defined.get(name), count) {
            (Some(a), 0) => format!("Agent · never run · {}", scope_of(a)),
            (Some(a), n) => format!("Agent · {} · {}", plural(n, "chat", "chats"), scope_of(a)),
            (None, n) => format!("Agent · {} · no longer defined", plural(n, "chat", "chats")),
        };
        b.node(Node {
            id: agent_id(name),
            label: name.to_string(),
            meta,
            kind: Kind::Agent,
            mark: if defined.get(name).is_some_and(|a| a.running) {
                Mark::Running
            } else {
                Mark::None
            },
            action: Action::Agent(name.to_string()),
            layer: 0,
            x: 0.0,
            y: 0.0,
        });
    }

    for entry in &chats.agents {
        b.graph.total_chats += entry.runs.len();
        let mut runs: Vec<&AgentRunInfo> = entry.runs.iter().collect();
        // Newest first, by the moment the conversation last *said* something — the same order the
        // sessions rail is in, so the six drawn here are the six anybody would name.
        runs.sort_by_key(|r| std::cmp::Reverse(r.last_activity.max(r.started_at)));
        let cut = if o.chats { CHATS_PER_AGENT } else { 0 };

        for (i, run) in runs.iter().enumerate() {
            let agent = b.agent_of(&entry.name);
            let from = b.launcher(&run.launched_by);
            if i < cut {
                let chat = b.node(Node {
                    id: format!("chat:{}/{}", entry.name, run.run_id),
                    label: title_of(run),
                    meta: chat_meta(&entry.name, run),
                    kind: Kind::Chat,
                    mark: mark_of(run),
                    action: Action::Chat {
                        agent: entry.name.clone(),
                        run_id: run.run_id.clone(),
                        interactive: entry.interactive,
                    },
                    layer: 0,
                    x: 0.0,
                    y: 0.0,
                });
                b.graph.shown_chats += 1;
                b.edge(from, chat);
                b.edge(chat, agent);
            } else if !o.chats {
                // Collapsed: the conversation itself is not drawn, so its two edges become the one
                // thing it says — that whoever started it caused work in this agent.
                b.edge(from, agent);
            }
        }
    }

    if o.tools {
        for agent in agents {
            let Some(from) = b.find(&agent_id(&agent.name)) else {
                continue;
            };
            for tool in &agent.bin_tools {
                let to = b.node(Node {
                    id: format!("tool:{tool}"),
                    label: tool.clone(),
                    meta: format!("Tool · on the PATH of the agents it is joined to · {tool}"),
                    kind: Kind::Tool,
                    mark: Mark::None,
                    action: Action::None,
                    layer: 0,
                    x: 0.0,
                    y: 0.0,
                });
                b.edge(from, to);
            }
        }
    }

    let mut g = b.graph;
    g.agents = g.nodes.iter().filter(|n| n.kind == Kind::Agent).count();
    place(&mut g, o.tools);
    g
}

/// One agent's scope, for its hover line: the project it is filed under, or that it is global.
fn scope_of(a: &AgentDto) -> String {
    a.project
        .as_deref()
        .map_or_else(|| "global".to_string(), |p| format!("project {p}"))
}

/// What a conversation is called: the name somebody gave it, else the message it was opened with,
/// cut to something that fits in a box.
fn title_of(r: &AgentRunInfo) -> String {
    let raw = r
        .title
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or(&r.message)
        .trim();
    if raw.is_empty() {
        return "Untitled".to_string();
    }
    // One line: a task pasted into the composer arrives here with its newlines, and a box is one
    // line tall.
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The hover line for a conversation: whose it is, who asked for it, and where it got to.
fn chat_meta(agent: &str, r: &AgentRunInfo) -> String {
    let who = match r.launched_by.as_str() {
        LAUNCHED_BY_HUMAN => "started by you".to_string(),
        AUTOMATION => "started by automation".to_string(),
        "" => "nobody recorded who started it".to_string(),
        other => match other.strip_prefix(AGENT_PREFIX) {
            Some(name) => format!("started by {name}"),
            None => format!("started by {other}"),
        },
    };
    let state = match mark_of(r) {
        Mark::Running => "running",
        Mark::Waiting => "waiting on an answer",
        Mark::Failed => "ended in an error",
        Mark::None => "finished",
    };
    format!("Chat of {agent} · {who} · {state}")
}

/// The dot a conversation wears. Running first: a conversation that is going is the thing somebody
/// scanning the canvas is looking for, whatever it has been through to get here.
fn mark_of(r: &AgentRunInfo) -> Mark {
    if r.running {
        Mark::Running
    } else if r.pending_question.is_some() {
        Mark::Waiting
    } else if r.outcome.as_ref().is_some_and(|o| o.is_error) {
        Mark::Failed
    } else {
        Mark::None
    }
}

fn agent_id(name: &str) -> String {
    format!("agent:{name}")
}

/// Nodes and edges under construction, with the index that keeps a node from being made twice.
#[derive(Default)]
struct Builder {
    graph: Graph,
    by_id: HashMap<String, usize>,
}

impl Builder {
    /// Add a node, or find the one already standing under that id.
    fn node(&mut self, node: Node) -> usize {
        if let Some(&i) = self.by_id.get(&node.id) {
            return i;
        }
        let i = self.graph.nodes.len();
        self.by_id.insert(node.id.clone(), i);
        self.graph.nodes.push(node);
        i
    }

    fn find(&self, id: &str) -> Option<usize> {
        self.by_id.get(id).copied()
    }

    /// The node for an agent the history names — created on demand, because a conversation can
    /// belong to an agent that has since been deleted, and it still happened.
    fn agent_of(&mut self, name: &str) -> usize {
        let id = agent_id(name);
        if let Some(i) = self.find(&id) {
            return i;
        }
        self.node(Node {
            id,
            label: name.to_string(),
            meta: "Agent · no longer defined".to_string(),
            kind: Kind::Agent,
            mark: Mark::None,
            action: Action::Agent(name.to_string()),
            layer: 0,
            x: 0.0,
            y: 0.0,
        })
    }

    /// The node a `launched_by` points at: an origin, or the agent that asked.
    fn launcher(&mut self, launched_by: &str) -> usize {
        let origin = match launched_by {
            LAUNCHED_BY_HUMAN => Origin::Human,
            AUTOMATION => Origin::Automation,
            "" => Origin::Unknown,
            other => {
                return match other.strip_prefix(AGENT_PREFIX) {
                    // An agent that started work here is in the picture whether or not it has any
                    // conversation of its own left in the listing.
                    Some(name) => self.agent_of(name),
                    // A word from a launcher this client does not know. Saying so beats filing it
                    // under one of ours.
                    None => self.node(Node {
                        id: format!("origin:{other}"),
                        label: other.to_string(),
                        meta: format!("Started work here, and called itself {other}"),
                        kind: Kind::Origin,
                        mark: Mark::None,
                        action: Action::None,
                        layer: 0,
                        x: 0.0,
                        y: 0.0,
                    }),
                };
            }
        };
        self.node(Node {
            id: origin.id().to_string(),
            label: origin.label().to_string(),
            meta: origin.meta().to_string(),
            kind: Kind::Origin,
            mark: Mark::None,
            action: Action::None,
            layer: 0,
            x: 0.0,
            y: 0.0,
        })
    }

    /// Join two nodes. An agent that started its own work would otherwise draw a loop onto itself,
    /// and the same pair can arrive many times over — six conversations of one agent started by
    /// one other agent is one thing said six times.
    fn edge(&mut self, from: usize, to: usize) {
        if from != to
            && !self
                .graph
                .edges
                .iter()
                .any(|e| e.from == from && e.to == to)
        {
            self.graph.edges.push(Edge { from, to });
        }
    }
}

/// Put every node somewhere: a column per step away from where work came from, and a row within it.
///
/// The column is a breadth-first distance, not a longest path. An agent reached both by a person on
/// its first conversation and by a chain six deep sits one step after the person — which keeps the
/// picture as wide as the machine's deepest *new* path rather than as wide as its busiest agent's
/// history. An edge that then runs backwards is drawn as one.
// Row and column indices become coordinates. A graph with more nodes than an `f64` can count
// exactly would need a display the size of a country.
#[allow(clippy::cast_precision_loss)]
fn place(g: &mut Graph, tools_last: bool) {
    let n = g.nodes.len();
    if n == 0 {
        return;
    }

    // Roots: everything nothing points at. On a machine with a cycle of agents starting each
    // other's work there may be none at all, so the first node stands in as one.
    let mut incoming = vec![0usize; n];
    for e in &g.edges {
        incoming[e.to] += 1;
    }
    let mut layer = vec![usize::MAX; n];
    let mut queue: Vec<usize> = (0..n).filter(|&i| incoming[i] == 0).collect();
    if queue.is_empty() {
        queue.push(0);
    }
    for &i in &queue {
        layer[i] = 0;
    }
    let out: Vec<Vec<usize>> = {
        let mut o = vec![Vec::new(); n];
        for e in &g.edges {
            o[e.from].push(e.to);
        }
        o
    };
    let mut head = 0;
    let mut passes = 0;
    while head < queue.len() && passes < n * MAX_PASSES {
        let i = queue[head];
        head += 1;
        for &j in &out[i] {
            passes += 1;
            if layer[j] == usize::MAX {
                layer[j] = layer[i] + 1;
                queue.push(j);
            }
        }
    }
    // Anything still unreached is in a cycle with no way in. It is not nothing, so it starts its
    // own chain rather than being dropped.
    for l in &mut layer {
        if *l == usize::MAX {
            *l = 0;
        }
    }

    // Tools are the end of every road: one column past everything, so they read as what these
    // agents may run rather than as another step in the flow.
    if tools_last {
        let last = layer.iter().copied().max().unwrap_or(0) + 1;
        for (i, node) in g.nodes.iter().enumerate() {
            if node.kind == Kind::Tool {
                layer[i] = last;
            }
        }
    }
    for (i, node) in g.nodes.iter_mut().enumerate() {
        node.layer = layer[i];
    }

    // Order within a column by where the things pointing at it ended up, so an edge travels as
    // little vertical distance as it can. Columns are done left to right, which is the order that
    // makes "where its parents are" a question with an answer.
    let columns = layer.iter().copied().max().unwrap_or(0) + 1;
    let mut into: Vec<Vec<usize>> = vec![Vec::new(); n];
    for e in &g.edges {
        into[e.to].push(e.from);
    }
    let mut placed = vec![f64::NAN; n];
    for col in 0..columns {
        let mut rows: Vec<usize> = (0..n).filter(|&i| layer[i] == col).collect();
        rows.sort_by(|&a, &b| {
            let key = |i: usize| -> f64 {
                let ys: Vec<f64> = into[i]
                    .iter()
                    .map(|&p| placed[p])
                    .filter(|y| y.is_finite())
                    .collect();
                if ys.is_empty() {
                    f64::MAX
                } else {
                    ys.iter().sum::<f64>() / ys.len() as f64
                }
            };
            key(a)
                .partial_cmp(&key(b))
                .unwrap_or(std::cmp::Ordering::Equal)
                // A stable tie-break, so the same machine draws the same picture twice.
                .then_with(|| g.nodes[a].id.cmp(&g.nodes[b].id))
        });
        let count = rows.len();
        for (row, &i) in rows.iter().enumerate() {
            let y = (row as f64 - (count as f64 - 1.0) / 2.0) * ROW_PITCH;
            placed[i] = y;
            g.nodes[i].y = y;
            g.nodes[i].x = (col as f64 - (columns as f64 - 1.0) / 2.0) * COL_PITCH;
        }
    }

    g.extent = g.nodes.iter().fold(
        (f64::MAX, f64::MAX, f64::MIN, f64::MIN),
        |(x0, y0, x1, y1), n| {
            (
                x0.min(n.x - n.w() / 2.0),
                y0.min(n.y - NODE_H / 2.0),
                x1.max(n.x + n.w() / 2.0),
                y1.max(n.y + NODE_H / 2.0),
            )
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use adi_webapp_api::types::{AgentRunOutcome, AgentsState};

    /// An agent definition with only the fields this module reads set.
    fn agent(name: &str) -> AgentDto {
        serde_json::from_value(serde_json::json!({
            "name": name,
            "backend": "claude",
            "executor": "process",
            "created_at": 0,
            "updated_at": 0,
        }))
        .expect("an AgentDto with its required fields")
    }

    /// A run, named by its id and by who asked for it.
    fn run(id: &str, launched_by: &str) -> AgentRunInfo {
        AgentRunInfo {
            run_id: id.to_string(),
            started_at: 1,
            last_activity: 1,
            message: format!("do {id}"),
            title: None,
            running: false,
            hidden: false,
            starred: false,
            launched_by: launched_by.to_string(),
            overrides: String::new(),
            pending_question: None,
            outcome: None,
            awaits: Vec::new(),
        }
    }

    fn runs(name: &str, rs: Vec<AgentRunInfo>) -> AgentRuns {
        serde_json::from_value(serde_json::json!({ "name": name, "runs": rs }))
            .expect("an AgentRuns from its name and runs")
    }

    /// The machine this module exists to draw: a person opens a chat with `one`, `one` starts a
    /// chat of `two`, and `two` starts a chat of `three`.
    fn chain() -> (Vec<AgentDto>, AllAgentRuns) {
        let agents = vec![agent("one"), agent("two"), agent("three")];
        let all = AllAgentRuns {
            total: 3,
            agents: vec![
                runs("one", vec![run("r1", LAUNCHED_BY_HUMAN)]),
                runs("two", vec![run("r2", "agent:one")]),
                runs("three", vec![run("r3", "agent:two")]),
            ],
        };
        (agents, all)
    }

    fn node<'a>(g: &'a Graph, id: &str) -> &'a Node {
        g.nodes
            .iter()
            .find(|n| n.id == id)
            .unwrap_or_else(|| panic!("no node {id} in {:?}", g.nodes.iter().map(|n| &n.id)))
    }

    fn joined(g: &Graph, from: &str, to: &str) -> bool {
        let idx = |id: &str| g.nodes.iter().position(|n| n.id == id);
        match (idx(from), idx(to)) {
            (Some(a), Some(b)) => g.edges.iter().any(|e| e.from == a && e.to == b),
            _ => false,
        }
    }

    /// The chain reads left to right, one step per link: you, the chat you opened, the agent that
    /// ran it, the chat that agent started, and so on. This is the whole claim the page makes.
    #[test]
    fn the_spawn_chain_is_one_column_per_step() {
        let (agents, all) = chain();
        let g = build(&agents, &all, Options::default());
        for (id, layer) in [
            ("origin:human", 0),
            ("chat:one/r1", 1),
            ("agent:one", 2),
            ("chat:two/r2", 3),
            ("agent:two", 4),
            ("chat:three/r3", 5),
            ("agent:three", 6),
        ] {
            assert_eq!(node(&g, id).layer, layer, "{id} is in the wrong column");
        }
        assert!(joined(&g, "origin:human", "chat:one/r1"));
        assert!(joined(&g, "chat:one/r1", "agent:one"));
        assert!(joined(&g, "agent:one", "chat:two/r2"));
    }

    /// Every column is centred on the origin, so an untouched view opens on the middle of the
    /// graph rather than on its top-left corner.
    #[test]
    fn the_columns_are_centred_on_the_world_origin() {
        let (agents, all) = chain();
        let g = build(&agents, &all, Options::default());
        let xs: Vec<f64> = g.nodes.iter().map(|n| n.x).collect();
        let mid = (xs.iter().copied().fold(f64::MIN, f64::max)
            + xs.iter().copied().fold(f64::MAX, f64::min))
            / 2.0;
        assert!(mid.abs() < 1e-9, "the columns are centred on {mid}");
    }

    /// With conversations off, the picture is the agents and what they set off in each other — the
    /// chat's two edges become the one thing it said.
    #[test]
    fn chats_off_collapses_the_chain_onto_the_agents() {
        let (agents, all) = chain();
        let g = build(
            &agents,
            &all,
            Options {
                chats: false,
                ..Options::default()
            },
        );
        assert!(
            g.nodes.iter().all(|n| n.kind != Kind::Chat),
            "a conversation was drawn with chats off"
        );
        assert!(joined(&g, "origin:human", "agent:one"));
        assert!(joined(&g, "agent:one", "agent:two"));
        assert!(joined(&g, "agent:two", "agent:three"));
        assert_eq!(g.shown_chats, 0);
        assert_eq!(g.total_chats, 3, "the count is of what exists, not of what is drawn");
        assert_eq!(g.summary(), "3 agents · 3 conversations, none drawn");
    }

    /// An agent nothing has ever run is left out unless it is asked for: eighty of them would
    /// stand in front of the graph rather than in it.
    #[test]
    fn an_agent_that_never_ran_is_drawn_only_when_asked_for() {
        let (mut agents, all) = chain();
        agents.push(agent("idle"));
        let hidden = build(&agents, &all, Options::default());
        assert!(hidden.nodes.iter().all(|n| n.id != "agent:idle"));
        let shown = build(
            &agents,
            &all,
            Options {
                idle: true,
                ..Options::default()
            },
        );
        assert_eq!(node(&shown, "agent:idle").meta, "Agent · never run · global");
    }

    /// A conversation whose agent has since been deleted still happened, and the agent that
    /// started it is still in the picture even with no conversation of its own left.
    #[test]
    fn the_join_is_outer_on_both_sides() {
        let all = AllAgentRuns {
            total: 1,
            agents: vec![runs("gone", vec![run("r1", "agent:also-gone")])],
        };
        let g = build(&[], &all, Options::default());
        assert_eq!(node(&g, "agent:gone").meta, "Agent · 1 chat · no longer defined");
        assert!(joined(&g, "agent:also-gone", "chat:gone/r1"));
    }

    /// Only the newest few of an agent's conversations are drawn, and the page is told what it is
    /// not showing rather than left to imply it is showing everything.
    #[test]
    fn the_newest_conversations_are_drawn_and_the_rest_are_counted() {
        let mut rs: Vec<AgentRunInfo> = (0..CHATS_PER_AGENT + 4)
            .map(|i| {
                let mut r = run(&format!("r{i}"), LAUNCHED_BY_HUMAN);
                r.started_at = i as u64;
                r.last_activity = i as u64;
                r
            })
            .collect();
        rs.reverse();
        let all = AllAgentRuns {
            total: rs.len(),
            agents: vec![runs("one", rs)],
        };
        let g = build(&[agent("one")], &all, Options::default());
        assert_eq!(g.shown_chats, CHATS_PER_AGENT);
        assert_eq!(g.total_chats, CHATS_PER_AGENT + 4);
        assert!(
            g.nodes.iter().any(|n| n.id == "chat:one/r9"),
            "the newest conversation is not drawn"
        );
        assert!(
            g.nodes.iter().all(|n| n.id != "chat:one/r0"),
            "the oldest conversation was drawn over a newer one"
        );
        assert_eq!(g.summary(), "1 agent · 6 of 10 conversations");
    }

    /// The three words a run can carry for who asked, and the absence that is none of them.
    #[test]
    fn every_launcher_lands_on_its_own_root() {
        let all = AllAgentRuns {
            total: 3,
            agents: vec![runs(
                "one",
                vec![
                    run("r1", LAUNCHED_BY_HUMAN),
                    run("r2", AUTOMATION),
                    run("r3", ""),
                ],
            )],
        };
        let g = build(&[agent("one")], &all, Options::default());
        assert!(joined(&g, "origin:human", "chat:one/r1"));
        assert!(joined(&g, "origin:automation", "chat:one/r2"));
        assert!(joined(&g, "origin:unknown", "chat:one/r3"));
        assert_eq!(node(&g, "origin:unknown").label, "Unrecorded");
    }

    /// Two agents that start each other's work is a cycle, and a layering that waits for one to
    /// settle would not return. Every node still gets a column.
    #[test]
    fn a_cycle_between_two_agents_still_lays_out() {
        let all = AllAgentRuns {
            total: 2,
            agents: vec![
                runs("one", vec![run("r1", "agent:two")]),
                runs("two", vec![run("r2", "agent:one")]),
            ],
        };
        let g = build(&[agent("one"), agent("two")], &all, Options::default());
        assert!(!g.is_empty());
        assert!(g.nodes.iter().all(|n| n.x.is_finite() && n.y.is_finite()));
    }

    /// Tools stand past the end of the chain, not inside it.
    #[test]
    fn tools_are_the_last_column() {
        let (mut agents, all) = chain();
        agents[0].bin_tools = vec!["sys-db".to_string()];
        let g = build(
            &agents,
            &all,
            Options {
                tools: true,
                ..Options::default()
            },
        );
        let tool = node(&g, "tool:sys-db");
        let deepest = g
            .nodes
            .iter()
            .filter(|n| n.kind != Kind::Tool)
            .map(|n| n.layer)
            .max()
            .expect("a node that is not a tool");
        assert!(tool.layer > deepest, "a tool shares a column with the flow");
        assert!(joined(&g, "agent:one", "tool:sys-db"));
    }

    /// What a conversation wears, in the order that matters: a run that is going says so, whatever
    /// it has been through to get here.
    #[test]
    fn a_running_chat_is_marked_running_whatever_else_is_true_of_it() {
        let mut r = run("r1", LAUNCHED_BY_HUMAN);
        r.running = true;
        r.outcome = Some(AgentRunOutcome {
            is_error: true,
            ..AgentRunOutcome::default()
        });
        assert_eq!(mark_of(&r), Mark::Running);
        r.running = false;
        assert_eq!(mark_of(&r), Mark::Failed);
    }

    /// A hit is inside the box a node was drawn as, not near it.
    #[test]
    fn a_node_is_hit_inside_its_own_box() {
        let (agents, all) = chain();
        let g = build(&agents, &all, Options::default());
        let n = node(&g, "agent:one").clone();
        assert!(n.hit(n.x, n.y));
        assert!(n.hit(n.x + n.w() / 2.0 - 1.0, n.y + NODE_H / 2.0 - 1.0));
        assert!(!n.hit(n.x + n.w() / 2.0 + 2.0, n.y));
        assert!(!n.hit(n.x, n.y + NODE_H));
    }

    /// The empty machine: no agents, no history, nothing drawn — and nothing that panics on the
    /// way to saying so.
    #[test]
    fn nothing_at_all_is_an_empty_graph() {
        let g = build(
            &[],
            &AllAgentRuns {
                agents: Vec::new(),
                total: 0,
            },
            Options::default(),
        );
        assert!(g.is_empty());
        assert_eq!(g.summary(), "0 agents");
        assert_eq!(g.at(0.0, 0.0), None);
    }

    /// The shape of a real machine, not of a fixture: 80 agents, 50 conversations each, most of
    /// them started by one busy agent. Measured off this operator's own store on 2026-09-12, and
    /// here because every number that matters — how many boxes, how wide the picture, how long the
    /// layout takes — is decided by this case and by no smaller one.
    #[test]
    fn a_machine_with_eighty_agents_and_a_thousand_conversations_stays_a_picture() {
        let agents: Vec<AgentDto> = (0..80).map(|i| agent(&format!("a{i}"))).collect();
        let all = AllAgentRuns {
            total: 80 * 50,
            agents: (0..80)
                .map(|i| {
                    let rs: Vec<AgentRunInfo> = (0..50)
                        .map(|j| {
                            let mut r = run(
                                &format!("r{i}-{j}"),
                                // As the real store reads: most history unattributed, a busy agent
                                // behind a fifth of it, a person behind a few.
                                match j % 5 {
                                    0 => "agent:a0",
                                    1 => LAUNCHED_BY_HUMAN,
                                    _ => "",
                                },
                            );
                            r.started_at = j as u64;
                            r.last_activity = j as u64;
                            r
                        })
                        .collect();
                    runs(&format!("a{i}"), rs)
                })
                .collect(),
        };
        let g = build(&agents, &all, Options::default());
        assert_eq!(g.total_chats, 4000);
        assert_eq!(g.shown_chats, 80 * CHATS_PER_AGENT);
        // Boxes, not a wall of them: the cap is what keeps this under a thousand.
        assert!(g.nodes.len() < 1000, "{} nodes", g.nodes.len());
        // And it is a graph rather than a heap — every box has a finite place in it.
        assert!(g.nodes.iter().all(|n| n.x.is_finite() && n.y.is_finite()));
        assert_eq!(g.summary(), "80 agents · 480 of 4000 conversations");
    }

    /// `AgentsState` is what the page actually holds, so the shape this module is given is the
    /// shape that arrives — not a vector somebody built by hand for a test.
    #[test]
    fn it_reads_the_listing_the_page_holds() {
        let state: AgentsState = serde_json::from_value(serde_json::json!({
            "agents": [{
                "name": "one", "backend": "claude", "executor": "process",
                "created_at": 0, "updated_at": 0,
            }],
            "form": { "backends": [], "fields": [], "presets": [] },
        }))
        .expect("an AgentsState");
        let (_, all) = chain();
        let g = build(&state.agents, &all, Options::default());
        assert!(g.nodes.iter().any(|n| n.id == "agent:one"));
    }
}
