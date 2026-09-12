//! What the live graph is a graph *of*: the conversations on this machine, what set each one off,
//! and where each card sits.
//!
//! Built from the two listings the panel already carries — `/api/agents` (what is defined) and
//! `/api/agents/runs/all` (what was done) — so the page costs no new endpoint, the same way the
//! analytics page is built. An agent is not a step of its own: it is written on the cards of its
//! conversations, which is where somebody reading one looks for whose it is. The definitions are
//! still read, for what a card says about itself — the project it is filed under, or that the agent
//! it belongs to is gone.
//!
//! **What the store cannot tell us, and what that means for the picture.** A run records which
//! *agent* asked for it (`launched_by: agent:<name>`), never which of that agent's conversations
//! did. Every edge is therefore an agent-level fact drawn between two conversations: it leaves the
//! conversation of that agent nearest in time to the one it points at (see [`started_by`]), and
//! what it asserts is that *that agent* set this off — nothing about which of its chats did. The
//! hover line says it in those words, "started by adi-agent", and never names a chat.
//!
//! **Every card has at most one parent**, which is what makes the picture a tree rather than a
//! web — and a tree is laid out as one: a subtree owns a band of rows that nothing else may stand
//! in, so what a conversation started is read by looking down from it rather than by following a
//! line into a column shared with fifty strangers. See [`place`].
//!
//! Everything here is pure: the listings in, a laid-out [`Graph`] out. The page re-runs it when the
//! data or the focus changes and draws whatever comes back — panning and zooming never touch it.

use std::collections::HashMap;

use adi_webapp_api::types::{AgentDto, AgentRunInfo, AgentRuns, AllAgentRuns, LAUNCHED_BY_HUMAN};

/// The rest of `adi_agents::launcher`'s vocabulary. Only the word for a person is on the wire
/// already ([`LAUNCHED_BY_HUMAN`]); the panel cannot link that crate, so the other two are mirrored
/// here. An empty `launched_by` is deliberately none of them — see [`Origin::Unknown`].
const AUTOMATION: &str = "automation";
const AGENT_PREFIX: &str = "agent:";

/// How many of an agent's conversations the graph draws, newest first, and how many it draws in
/// all.
///
/// Both caps are load-bearing, and the second one was learnt the hard way. The listing carries up
/// to fifty conversations per agent, and this operator's machine has eighty agents and 1212 of
/// them: six each is still 328 cards, and 328 cards is a column fifteen thousand units tall that
/// fits on a screen at 6% — a grey smear, not a graph. Eighty in all is a picture: what has
/// happened lately, spread across the machine rather than taken from whichever agent talks most.
pub(crate) const CHATS_PER_AGENT: usize = 6;
pub(crate) const CHATS_DRAWN: usize = 80;

/// The tallest a block of one conversation's childless children may get before it wraps into a
/// block of columns.
///
/// On a real machine one conversation — or one origin — can be the start of three hundred things.
/// Stacked, that is a line; wrapped, it is a block whose shape a screen can hold.
///
/// Twelve is measured, not chosen: a block is as tall as this and as wide as it has to be, so this
/// number alone decides the shape of the whole picture, and the fitted scale is what it costs. On
/// the machine the fixture at the bottom of this file models, 1160×690 of stage fits the graph at
/// 25% with a block of 16, **39% with 12**, 38% with 10 and 26% with 6 — either side of it the
/// picture is a smear on one axis. Labels go out below 34% (`paint::LABEL_SCALE`), so the window
/// is narrow and this sits in the middle of it.
const MAX_ROWS: usize = 12;

/// The air between one subtree and the next, in rows. Half a row, which is enough for two bands to
/// read as two without paying a whole card's height at every branch.
const SIBLING_GAP: f64 = 0.5;

/// Node geometry, in world units. A conversation is a card of two lines — what it is called, and
/// whose it is — so it is taller than the pill that stands for where work came from; those are the
/// only two shapes on the canvas.
pub(crate) const CHAT_H: f64 = 46.0;
pub(crate) const ORIGIN_H: f64 = 34.0;
pub(crate) const CHAT_W: f64 = 250.0;
pub(crate) const ORIGIN_W: f64 = 150.0;

/// Room between one column and the next, and the distance between two rows. A column is as wide as
/// the widest kind of node standing in it plus this gap — enough for an edge to curve through
/// rather than past, and no wider, because every unit of width is a unit the whole picture has to
/// be shrunk by to fit on a screen.
pub(crate) const COL_GAP: f64 = 70.0;
pub(crate) const ROW_PITCH: f64 = 60.0;

/// What a node stands for. The kind decides its shape, its size and its tone — nothing else about a
/// node says which of these it is, because a word on every card would be the same word on nearly
/// all of them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Origin,
    Chat,
}

impl Kind {
    pub(crate) const fn width(self) -> f64 {
        match self {
            Kind::Origin => ORIGIN_W,
            Kind::Chat => CHAT_W,
        }
    }

    pub(crate) const fn height(self) -> f64 {
        match self {
            Kind::Origin => ORIGIN_H,
            Kind::Chat => CHAT_H,
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

/// The conversation a card stands for — what **Open chat** opens while the picture is rooted at it.
/// An origin is not a conversation and carries none.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Chat {
    pub(crate) agent: String,
    pub(crate) run_id: String,
    pub(crate) interactive: bool,
}

/// How a node stands in the picture as it is drawn *now* — a fact about where the focus is, not
/// about the machine. Everything is [`Role::Plain`] until somebody clicks a card.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(crate) enum Role {
    #[default]
    Plain,
    /// What the picture is rooted at: the card that was clicked, and the only one drawn with
    /// everything it started and nothing it did not.
    Focus,
    /// The one step above the focus, drawn quietly and for one reason — so there is something to
    /// click to go back up.
    Ancestor,
}

/// One card on the canvas, already placed. `x`/`y` are its centre, in world units.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Node {
    pub(crate) id: String,
    /// The first line: what the conversation is called, or what the origin is.
    pub(crate) label: String,
    /// The second line: whose conversation this is. Empty on an origin, which is nobody's.
    pub(crate) agent: String,
    /// What the head says while the pointer is on it — the whole of what a hover has to tell.
    pub(crate) meta: String,
    pub(crate) kind: Kind,
    pub(crate) mark: Mark,
    pub(crate) chat: Option<Chat>,
    pub(crate) role: Role,
    pub(crate) layer: usize,
    pub(crate) x: f64,
    pub(crate) y: f64,
}

impl Node {
    pub(crate) const fn w(&self) -> f64 {
        self.kind.width()
    }

    pub(crate) const fn h(&self) -> f64 {
        self.kind.height()
    }

    /// Whether a world point is inside this card.
    pub(crate) fn hit(&self, x: f64, y: f64) -> bool {
        (x - self.x).abs() <= self.w() / 2.0 && (y - self.y).abs() <= self.h() / 2.0
    }
}

/// A directed edge, by node index: `from` asked for, or may have asked for, `to`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Edge {
    pub(crate) from: usize,
    pub(crate) to: usize,
}

/// The laid-out graph, plus what had to be left out to lay it out.
#[derive(Clone, Default, PartialEq, Debug)]
pub(crate) struct Graph {
    pub(crate) nodes: Vec<Node>,
    pub(crate) edges: Vec<Edge>,
    /// Conversations drawn, and conversations there are. Equal when nothing was cut.
    pub(crate) shown_chats: usize,
    pub(crate) total_chats: usize,
    /// Whether the machine's own picture is a sample — whether [`pick`] left anything out before
    /// a focus narrowed it further. What is under one conversation can only be counted out of
    /// what was drawn, and the line has to say so when that is less than everything.
    pub(crate) sampled: bool,
    /// How many agents the drawn cards are between them — the spread of the picture, which a count
    /// of conversations alone does not say.
    pub(crate) agents: usize,
    /// The node the picture is rooted at, when somebody has clicked one. `None` is the whole
    /// machine.
    pub(crate) focus: Option<usize>,
    /// …and how many conversations hang off it, however deep. Only meaningful with a focus.
    pub(crate) under: usize,
    /// The bounding box of every node, centre to centre plus half a card: what **Fit** fits.
    pub(crate) extent: (f64, f64, f64, f64),
}

impl Graph {
    pub(crate) fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The node under a world point, topmost last — the reverse of drawing order, so a card drawn
    /// over another is the one that answers for the pixel.
    pub(crate) fn at(&self, x: f64, y: f64) -> Option<usize> {
        self.nodes.iter().rposition(|n| n.hit(x, y))
    }

    /// The line under the title: how much of the machine this picture is — or, once it is rooted
    /// at one conversation, what that one set off.
    pub(crate) fn summary(&self) -> String {
        if let Some(root) = self.focus.and_then(|i| self.nodes.get(i)) {
            if self.under == 0 {
                return format!("{} \u{b7} started nothing", root.label);
            }
            let under = plural(self.under, "conversation", "conversations");
            // "drawn under it" rather than "under it" whenever the machine's own picture is a
            // sample: this is a count of cards on the canvas, and saying it flat would claim the
            // page had looked at conversations it never drew.
            let how = if self.sampled { "drawn under it" } else { "under it" };
            return format!("{} \u{b7} {under} {how}", root.label);
        }
        let agents = plural(self.agents, "agent", "agents");
        let chats = plural(self.total_chats, "conversation", "conversations");
        if self.total_chats == 0 {
            return chats;
        }
        if self.shown_chats < self.total_chats {
            return format!("{agents} · {} of {chats}", self.shown_chats);
        }
        format!("{agents} · {chats}")
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Build and lay out the graph, rooted at `focus` — the id of a node — or at the whole machine.
///
/// A focus that names nothing on the canvas (the conversation aged out of the newest few, the
/// listing has not arrived yet) draws the whole machine rather than an empty stage.
pub(crate) fn build(agents: &[AgentDto], chats: &AllAgentRuns, focus: Option<&str>) -> Graph {
    let mut b = Builder::default();
    let defined: HashMap<&str, &AgentDto> = agents.iter().map(|a| (a.name.as_str(), a)).collect();

    for entry in &chats.agents {
        b.graph.total_chats += entry.runs.len();
    }

    // Every card first, so that joining them up afterwards can pick from all of an agent's — the
    // conversation an edge leaves depends on when the one it points at started.
    let drawn = pick(chats);
    let mut cards: Vec<(usize, &AgentRunInfo)> = Vec::with_capacity(drawn.len());
    let mut by_agent: HashMap<&str, Vec<usize>> = HashMap::new();
    for (entry, run) in &drawn {
        let i = b.node(Node {
            id: format!("chat:{}/{}", entry.name, run.run_id),
            label: title_of(run),
            agent: entry.name.clone(),
            meta: chat_meta(&entry.name, defined.get(entry.name.as_str()).copied(), run),
            kind: Kind::Chat,
            mark: mark_of(run),
            chat: Some(Chat {
                agent: entry.name.clone(),
                run_id: run.run_id.clone(),
                interactive: entry.interactive,
            }),
            role: Role::Plain,
            layer: 0,
            x: 0.0,
            y: 0.0,
        });
        cards.push((i, run));
        by_agent.entry(entry.name.as_str()).or_default().push(i);
    }
    b.graph.shown_chats = cards.len();
    b.graph.agents = by_agent.len();

    let starts: HashMap<usize, u64> = cards.iter().map(|&(i, r)| (i, r.started_at)).collect();
    for &(to, run) in &cards {
        let from = match run.launched_by.strip_prefix(AGENT_PREFIX) {
            // An agent asked for this one. Which of its conversations is not in the record, so the
            // edge leaves whichever of them this one started nearest to.
            Some(name) => started_by(
                by_agent.get(name).map_or(&[][..], Vec::as_slice),
                &starts,
                to,
            ),
            None => Some(b.origin(&run.launched_by)),
        };
        if let Some(from) = from {
            b.edge(from, to);
        }
    }

    let mut g = b.graph;
    g.sampled = g.shown_chats < g.total_chats;
    if let Some(id) = focus {
        focus_on(&mut g, id);
    }
    place(&mut g);
    g
}

/// Cut the graph down to what hangs off the node called `id`, plus the one node above it.
///
/// The one above is the whole of how you get back out: it is drawn quietly, it takes a click like
/// any other card, and clicking it roots the picture there. Its *other* children are not drawn —
/// this is a picture of one conversation's consequences, and the siblings of the thing you asked
/// about are not among them.
fn focus_on(g: &mut Graph, id: &str) {
    let Some(root) = g.nodes.iter().position(|n| n.id == id) else {
        return;
    };
    let n = g.nodes.len();
    let mut kids: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut parent: Vec<Option<usize>> = vec![None; n];
    for e in &g.edges {
        kids[e.from].push(e.to);
        if parent[e.to].is_none() {
            parent[e.to] = Some(e.from);
        }
    }

    // Everything below the root, breadth first. `keep` doubles as the visited set, which is what
    // stops two agents that start each other's work from being walked round for ever.
    let mut keep = vec![false; n];
    keep[root] = true;
    let mut queue = vec![root];
    let mut at = 0;
    while at < queue.len() {
        let i = queue[at];
        at += 1;
        for &k in &kids[i] {
            if !std::mem::replace(&mut keep[k], true) {
                queue.push(k);
            }
        }
    }
    // …and the step above, unless the root is in a cycle and its parent is already below it.
    let up = parent[root].filter(|&p| !keep[p]);
    if let Some(p) = up {
        keep[p] = true;
    }

    let mut index = vec![usize::MAX; n];
    let kept = std::mem::take(&mut g.nodes);
    for (i, node) in kept.into_iter().enumerate() {
        if keep[i] {
            index[i] = g.nodes.len();
            g.nodes.push(node);
        }
    }
    // An edge survives only with both ends: the ones leaving the step above into its other
    // children fall away here, along with whatever used to point at it.
    let edges = std::mem::take(&mut g.edges);
    g.edges = edges
        .into_iter()
        .filter(|e| keep[e.from] && keep[e.to])
        .map(|e| Edge {
            from: index[e.from],
            to: index[e.to],
        })
        .collect();

    // Both of these carry what a click does, because on this canvas that is the one thing a card
    // cannot say for itself: the root's click is the gesture's own undo, and the step above is
    // only there to be clicked.
    let at = &mut g.nodes[index[root]];
    at.meta = format!("{} \u{b7} click again for the whole machine", at.meta);
    at.role = Role::Focus;
    if let Some(p) = up {
        let step = &mut g.nodes[index[p]];
        step.meta = format!("Go up \u{b7} {}", step.meta);
        step.role = Role::Ancestor;
    }
    g.focus = Some(index[root]);
    // What the head counts: the conversations this one set off, at any depth. Neither the root
    // itself nor the step above it is under anything.
    g.under = queue.len() - 1;
    g.shown_chats = g
        .nodes
        .iter()
        .filter(|node| node.kind == Kind::Chat && node.role != Role::Ancestor)
        .count();
}

/// Which conversations get a card: the newest few of each agent's, the newest of those across the
/// whole machine — and then whoever started one of those, however old their own conversation is.
///
/// The first two cuts matter because without the first, one agent's history fills the canvas, and
/// without the second, eighty agents' sixes do. The third is what keeps the picture joined up: a
/// card naming an agent with nothing of its own on the canvas would hang from nothing, and the one
/// thing this page is about is what set what off.
fn pick(chats: &AllAgentRuns) -> Vec<(&AgentRuns, &AgentRunInfo)> {
    let mut picked: Vec<(&AgentRuns, &AgentRunInfo)> = chats
        .agents
        .iter()
        .flat_map(|entry| {
            let mut runs: Vec<&AgentRunInfo> = entry.runs.iter().collect();
            runs.sort_by_key(|r| std::cmp::Reverse(recency(r)));
            runs.truncate(CHATS_PER_AGENT);
            runs.into_iter().map(move |r| (entry, r))
        })
        .collect();
    picked.sort_by_key(|(_, r)| std::cmp::Reverse(recency(r)));
    picked.truncate(CHATS_DRAWN);

    // Walks what it is adding to, so an agent pulled in for having started something is itself
    // asked who started it. It terminates because every pass adds an agent that had no card, and
    // there are finitely many agents.
    let mut drawn: Vec<&str> = picked.iter().map(|(e, _)| e.name.as_str()).collect();
    let mut at = 0;
    while at < picked.len() {
        let (_, run) = picked[at];
        at += 1;
        let Some(name) = run.launched_by.strip_prefix(AGENT_PREFIX) else {
            continue;
        };
        if drawn.contains(&name) {
            continue;
        }
        let Some(entry) = chats.agents.iter().find(|a| a.name == name) else {
            continue;
        };
        let Some(newest) = entry.runs.iter().max_by_key(|r| recency(r)) else {
            continue;
        };
        drawn.push(entry.name.as_str());
        picked.push((entry, newest));
    }
    picked
}

/// Which of an agent's cards an edge into `to` leaves.
///
/// The store names the agent that asked and not the conversation, so this is a choice the picture
/// has to make and the record cannot: the conversation of that agent that had already begun and
/// began *last* — the one most likely to have been the one talking — and, where every one of them
/// began after this, the one that began soonest. Nearest in time, with what was already open
/// preferred. What the edge means is unchanged by which end it is drawn from: that agent set this
/// off.
fn started_by(candidates: &[usize], starts: &HashMap<usize, u64>, to: usize) -> Option<usize> {
    let child = starts.get(&to).copied().unwrap_or_default();
    candidates
        .iter()
        .copied()
        .filter(|&i| i != to)
        .min_by_key(|i| {
            let s = starts.get(i).copied().unwrap_or_default();
            if s <= child { (0, child - s) } else { (1, s - child) }
        })
}

/// One agent's scope, for the hover line of its conversations: the project it is filed under, or
/// that it is global.
fn scope_of(a: &AgentDto) -> String {
    a.project
        .as_deref()
        .map_or_else(|| "global".to_string(), |p| format!("project {p}"))
}

/// What a conversation is called: the name somebody gave it, else the message it was opened with,
/// cut to something that fits in a card.
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
    // One line: a task pasted into the composer arrives here with its newlines, and a card's title
    // is one line tall.
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The hover line for a conversation: whose it is, who asked for it, and where it got to. The agent
/// is on the card already, so this is what the card has no room for.
fn chat_meta(agent: &str, defined: Option<&AgentDto>, r: &AgentRunInfo) -> String {
    let whose = match defined {
        Some(a) => format!("Chat of {agent} · {}", scope_of(a)),
        None => format!("Chat of {agent} · no longer defined"),
    };
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
    format!("{whose} · {who} · {state}")
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

/// When a conversation last said something — what "newest" means here, and the same order the
/// sessions rail is in. `started_at` stands in for a conversation that has said nothing yet.
fn recency(r: &AgentRunInfo) -> u64 {
    r.last_activity.max(r.started_at)
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

    /// The root a `launched_by` that is not an agent points at.
    fn origin(&mut self, launched_by: &str) -> usize {
        let origin = match launched_by {
            LAUNCHED_BY_HUMAN => Origin::Human,
            AUTOMATION => Origin::Automation,
            "" => Origin::Unknown,
            // A word from a launcher this client does not know. Saying so beats filing it under
            // one of ours.
            other => {
                return self.node(Node {
                    id: format!("origin:{other}"),
                    label: other.to_string(),
                    agent: String::new(),
                    meta: format!("Started work here, and called itself {other}"),
                    kind: Kind::Origin,
                    mark: Mark::None,
                    chat: None,
                    role: Role::Plain,
                    layer: 0,
                    x: 0.0,
                    y: 0.0,
                });
            }
        };
        self.node(Node {
            id: origin.id().to_string(),
            label: origin.label().to_string(),
            agent: String::new(),
            meta: origin.meta().to_string(),
            kind: Kind::Origin,
            mark: Mark::None,
            chat: None,
            role: Role::Plain,
            layer: 0,
            x: 0.0,
            y: 0.0,
        })
    }

    /// Join two nodes. A conversation that started another of its own agent's would otherwise draw
    /// a loop onto itself, and the same pair can arrive many times over.
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

/// The tree the picture is drawn as: who hangs from each node, and which nodes hang from nothing.
///
/// Every card is given exactly one edge in (see [`build`]), so this is already a forest — except
/// that two agents can start each other's work, which makes a ring with no way into it. A ring is
/// broken at its lowest-numbered member, which becomes a root of its own; the edge is still drawn,
/// and [`paint`](super::paint) draws one that runs backwards as a curve that goes back.
fn tree(g: &Graph) -> (Vec<Vec<usize>>, Vec<usize>) {
    let n = g.nodes.len();
    let mut parent: Vec<Option<usize>> = vec![None; n];
    for e in &g.edges {
        if e.from != e.to && parent[e.to].is_none() {
            parent[e.to] = Some(e.from);
        }
    }

    // Walk every chain of parents once, marking what has been settled. A node found still on the
    // current chain closes a ring, and the ring is cut there. Each node is visited once, so this
    // is linear however tangled the machine is.
    let (unseen, walking, settled) = (0_u8, 1_u8, 2_u8);
    let mut state = vec![unseen; n];
    for i in 0..n {
        if state[i] != unseen {
            continue;
        }
        let mut chain: Vec<usize> = Vec::new();
        let mut step = Some(i);
        while let Some(c) = step {
            if state[c] == walking {
                let from = chain.iter().position(|&p| p == c).unwrap_or(0);
                let lowest = chain[from..].iter().copied().min().unwrap_or(c);
                parent[lowest] = None;
                break;
            }
            if state[c] == settled {
                break;
            }
            state[c] = walking;
            chain.push(c);
            step = parent[c];
        }
        for &p in &chain {
            state[p] = settled;
        }
    }

    // Children in the order their cards were made, which is the order [`pick`] chose them in:
    // newest first, the same order the sessions rail is in. Deterministic, so a machine that has
    // not changed draws the same picture twice.
    let mut kids: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, p) in parent.iter().enumerate() {
        if let Some(p) = *p {
            kids[p].push(i);
        }
    }
    let roots = (0..n).filter(|&i| parent[i].is_none()).collect();
    (kids, roots)
}

/// Place `i`'s subtree with its top row at `top`, and answer how many rows it took.
///
/// **The band is the whole point of this page.** A subtree owns a contiguous run of rows and
/// nothing from anywhere else may stand in it, so a conversation's children are the cards level
/// with it and immediately under it — never a card in the same column that belongs to somebody
/// four branches away.
///
/// Children come in two sorts, and they are treated differently on purpose. One that started
/// nothing is a leaf, and leaves are stacked into one block, wrapped into more than one column
/// past [`MAX_ROWS`] of them — on this operator's machine one origin is where fifty conversations
/// came from, and fifty stacked is a line no screen holds. One that started something gets a band
/// of its own under that block, as tall as everything below it. The parent comes to rest level
/// with the middle of its own band.
// Row indices become coordinates. A graph with more rows than an `f64` counts exactly would need a
// display the size of a country.
#[allow(clippy::cast_precision_loss)]
fn arrange(i: usize, top: f64, kids: &[Vec<usize>], row: &mut [f64], col: &mut [usize]) -> f64 {
    if kids[i].is_empty() {
        row[i] = top;
        return 1.0;
    }
    let (leaves, branches): (Vec<usize>, Vec<usize>) =
        kids[i].iter().copied().partition(|&k| kids[k].is_empty());

    let mut used = 0.0;
    if !leaves.is_empty() {
        let columns = leaves.len().div_ceil(MAX_ROWS).max(1);
        // Split evenly rather than filling the first column to the brim: two columns of nine read
        // as a block, one of eighteen and one of one reads as a mistake.
        let per = leaves.len().div_ceil(columns).max(1);
        for (at, &k) in leaves.iter().enumerate() {
            col[k] = at / per;
            row[k] = top + (at % per) as f64;
        }
        used = per as f64;
    }
    for &k in &branches {
        if used > 0.0 {
            used += SIBLING_GAP;
        }
        col[k] = 0;
        used += arrange(k, top + used, kids, row, col);
    }
    row[i] = top + (used - 1.0) / 2.0;
    used
}

/// Put every node somewhere: a column per step away from where the work came from, and a band of
/// rows per subtree (see [`arrange`]).
// As above: row and column indices become coordinates.
#[allow(clippy::cast_precision_loss)]
fn place(g: &mut Graph) {
    let n = g.nodes.len();
    if n == 0 {
        return;
    }
    let (kids, roots) = tree(g);

    let mut row = vec![0.0; n];
    let mut col = vec![0usize; n];
    let mut top = 0.0;
    for &r in &roots {
        col[r] = 0;
        top += arrange(r, top, &kids, &mut row, &mut col) + SIBLING_GAP;
    }

    // How far from the root each node is. Every node is reachable now that the rings are cut, so
    // this needs nothing of the recursion above — only the order.
    let mut depth = vec![0usize; n];
    let mut queue = roots;
    let mut at = 0;
    while at < queue.len() {
        let i = queue[at];
        at += 1;
        for &k in &kids[i] {
            depth[k] = depth[i] + 1;
            queue.push(k);
        }
    }

    // Where each band starts, and how wide its columns are: as wide as the widest kind of node
    // standing in it, and as many columns across as the widest block of leaves in it needs. A band
    // of origins is narrower than a band of conversations, and charging the whole picture for the
    // widest card anywhere in it would be paid for in the fit.
    let bands = depth.iter().copied().max().unwrap_or(0) + 1;
    let mut band_left = vec![0.0; bands];
    let mut band_pitch = vec![0.0; bands];
    let mut spent = 0.0;
    for band in 0..bands {
        let members: Vec<usize> = (0..n).filter(|&i| depth[i] == band).collect();
        let pitch = members
            .iter()
            .map(|&i| g.nodes[i].w())
            .fold(0.0_f64, f64::max)
            + COL_GAP;
        let columns = members.iter().map(|&i| col[i] + 1).max().unwrap_or(1);
        band_left[band] = spent;
        band_pitch[band] = pitch;
        spent += columns as f64 * pitch;
    }

    for (i, node) in g.nodes.iter_mut().enumerate() {
        node.layer = depth[i];
        // The centre of its column, which is where a card of any width is hung from.
        node.x = band_pitch[depth[i]].mul_add(col[i] as f64 + 0.5, band_left[depth[i]]);
        node.y = row[i] * ROW_PITCH;
    }

    let raw = extent_of(g);
    // Bring the whole picture onto the world origin, which is where an untouched view is pointed.
    // The *extent* is what is centred, not the node centres: columns are not all the same width,
    // and it is the edges of the outermost cards that decide whether the picture looks centred.
    let (dx, dy) = (-raw.0.midpoint(raw.2), -raw.1.midpoint(raw.3));
    for node in &mut g.nodes {
        node.x += dx;
        node.y += dy;
    }
    g.extent = extent_of(g);
}

/// The box every node fits inside, `(x0, y0, x1, y1)`.
fn extent_of(g: &Graph) -> (f64, f64, f64, f64) {
    g.nodes.iter().fold(
        (f64::MAX, f64::MAX, f64::MIN, f64::MIN),
        |(x0, y0, x1, y1), n| {
            (
                x0.min(n.x - n.w() / 2.0),
                y0.min(n.y - n.h() / 2.0),
                x1.max(n.x + n.w() / 2.0),
                y1.max(n.y + n.h() / 2.0),
            )
        },
    )
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

    /// Whoever is drawn in `id`'s column, top to bottom.
    fn column_of<'a>(g: &'a Graph, id: &str) -> Vec<&'a Node> {
        let at = node(g, id);
        let mut down: Vec<&Node> = g
            .nodes
            .iter()
            .filter(|n| (n.x - at.x).abs() < 1.0)
            .collect();
        down.sort_by(|a, b| a.y.total_cmp(&b.y));
        down
    }

    /// The property this page turns on, checked over a whole graph: down any one column, a
    /// parent's children are an unbroken run. A parent that shows up twice in that order has had
    /// somebody else's children drawn through the middle of its own, which is exactly the picture
    /// this layout exists to stop.
    fn children_stay_together(g: &Graph) -> String {
        let parent = |i: usize| g.edges.iter().find(|e| e.to == i).map(|e| e.from);
        // Keyed on the bits of the x, not on a rounded one: every card in a column is given the
        // same x by the same arithmetic, so they are equal to the last bit.
        let mut columns: HashMap<u64, Vec<usize>> = HashMap::new();
        for (i, n) in g.nodes.iter().enumerate() {
            columns.entry(n.x.to_bits()).or_default().push(i);
        }
        let mut broken: Vec<String> = Vec::new();
        for down in columns.values_mut() {
            down.sort_by(|&a, &b| g.nodes[a].y.total_cmp(&g.nodes[b].y));
            let mut runs: Vec<Option<usize>> = Vec::new();
            for &i in down.iter() {
                if runs.last() != Some(&parent(i)) {
                    runs.push(parent(i));
                }
            }
            for (at, p) in runs.iter().enumerate() {
                if runs[..at].contains(p) {
                    let whose = p.map_or("(nothing)", |i| g.nodes[i].id.as_str());
                    broken.push(format!("the children of {whose} are drawn in two runs"));
                }
            }
        }
        broken.join("; ")
    }

    /// The chain reads left to right, one card per step: you, the conversation you opened, the one
    /// it started, and on. The agent is not a step of its own — it is written on the card.
    #[test]
    fn the_spawn_chain_is_one_card_per_conversation() {
        let (agents, all) = chain();
        let g = build(&agents, &all, None);
        for (id, layer) in [
            ("origin:human", 0),
            ("chat:one/r1", 1),
            ("chat:two/r2", 2),
            ("chat:three/r3", 3),
        ] {
            assert_eq!(node(&g, id).layer, layer, "{id} is in the wrong column");
        }
        assert!(g.nodes.iter().all(|n| n.kind != Kind::Origin || n.agent.is_empty()));
        assert_eq!(node(&g, "chat:two/r2").agent, "two", "the card does not name its agent");
        assert!(joined(&g, "origin:human", "chat:one/r1"));
        assert!(joined(&g, "chat:one/r1", "chat:two/r2"));
        assert!(joined(&g, "chat:two/r2", "chat:three/r3"));
    }

    /// The hover line carries what the card has no room for: whose conversation it is, where that
    /// agent is filed, who asked, and where it got to.
    #[test]
    fn the_hover_line_says_whose_chat_it_is_and_who_asked() {
        let (agents, all) = chain();
        let g = build(&agents, &all, None);
        assert_eq!(
            node(&g, "chat:two/r2").meta,
            "Chat of two · global · started by one · finished"
        );
    }

    /// Two branches, which is the shape a chain cannot show: a person opens two conversations, and
    /// each of those starts two more. Everything in the second row belongs to `hub` or to `mid`,
    /// and the whole of this page is being able to tell which.
    fn family() -> (Vec<AgentDto>, AllAgentRuns) {
        let at = |id: &str, launched_by: &str, t: u64| {
            let mut r = run(id, launched_by);
            (r.started_at, r.last_activity) = (t, t);
            r
        };
        let agents = vec![agent("hub"), agent("mid"), agent("kid")];
        let all = AllAgentRuns {
            total: 6,
            agents: vec![
                runs("hub", vec![at("h", LAUNCHED_BY_HUMAN, 10)]),
                runs("mid", vec![at("m", LAUNCHED_BY_HUMAN, 15)]),
                runs(
                    "kid",
                    vec![
                        at("k1", "agent:hub", 20),
                        at("j1", "agent:mid", 25),
                        at("k2", "agent:hub", 30),
                        at("j2", "agent:mid", 35),
                    ],
                ),
            ],
        };
        (agents, all)
    }

    /// What the user asked this page for. A conversation's children are stacked against it, one
    /// unbroken run down the column, and the two that `mid` started never come between the two
    /// that `hub` did. The parent comes to rest level with the middle of its own.
    #[test]
    fn a_conversations_children_are_stacked_against_it_and_nobody_elses_come_between() {
        let (agents, all) = family();
        let g = build(&agents, &all, None);
        assert_eq!(children_stay_together(&g), "");
        let down: Vec<&str> = column_of(&g, "chat:kid/k1")
            .iter()
            .map(|n| n.id.as_str())
            .collect();
        assert_eq!(
            down,
            ["chat:kid/j2", "chat:kid/j1", "chat:kid/k2", "chat:kid/k1"],
            "the second row is interleaved"
        );
        let middle = |a: &str, b: &str| node(&g, a).y.midpoint(node(&g, b).y);
        for (parent, kids) in [
            ("chat:mid/m", ("chat:kid/j2", "chat:kid/j1")),
            ("chat:hub/h", ("chat:kid/k2", "chat:kid/k1")),
        ] {
            let (y, want) = (node(&g, parent).y, middle(kids.0, kids.1));
            assert!((y - want).abs() < 1e-9, "{parent} sits at {y}, not {want}");
        }
    }

    /// Clicking a card draws the graph from it: that conversation, everything it set off however
    /// deep, and the one step above it to click on the way back — and nothing else on the machine.
    #[test]
    fn a_click_draws_the_graph_from_the_card_that_was_clicked() {
        let (agents, all) = chain();
        let g = build(&agents, &all, Some("chat:two/r2"));
        let mut ids: Vec<&str> = g.nodes.iter().map(|n| n.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, ["chat:one/r1", "chat:three/r3", "chat:two/r2"]);
        assert_eq!(node(&g, "chat:two/r2").role, Role::Focus);
        assert_eq!(node(&g, "chat:one/r1").role, Role::Ancestor);
        assert_eq!(node(&g, "chat:three/r3").role, Role::Plain);
        assert!(joined(&g, "chat:one/r1", "chat:two/r2"), "no way back up");
        assert!(joined(&g, "chat:two/r2", "chat:three/r3"));
        assert_eq!(g.summary(), "do r2 · 1 conversation under it");
        assert!(!g.sampled, "nothing was left out of this machine's picture");
    }

    /// The step above is drawn for one reason — to be clicked — so what *else* it started is not
    /// drawn. This is a picture of one conversation's consequences, and its siblings are not
    /// among them.
    #[test]
    fn the_step_above_the_focus_brings_none_of_its_other_children() {
        let (agents, all) = family();
        let g = build(&agents, &all, Some("chat:kid/k2"));
        let mut ids: Vec<&str> = g.nodes.iter().map(|n| n.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, ["chat:hub/h", "chat:kid/k2"]);
        assert!(
            node(&g, "chat:hub/h").meta.starts_with("Go up \u{b7} "),
            "the step above does not say it is the way back"
        );
        assert!(
            node(&g, "chat:kid/k2")
                .meta
                .ends_with("click again for the whole machine"),
            "the root does not say what a second click does"
        );
        assert_eq!(g.summary(), "do k2 · started nothing");
    }

    /// A focus can name a conversation that is no longer drawn — it aged out of the newest few
    /// while the page was open, or the listing has not arrived yet. The whole machine is a better
    /// answer than an empty stage.
    #[test]
    fn a_focus_on_something_that_is_not_there_draws_the_whole_machine() {
        let (agents, all) = chain();
        let g = build(&agents, &all, Some("chat:nobody/gone"));
        assert_eq!(g.nodes.len(), 4);
        assert_eq!(g.focus, None);
        assert_eq!(g.summary(), "3 agents · 3 conversations");
    }

    /// An origin takes a click like any other card: "everything a person started" is a question
    /// worth asking on a machine where most of the history is automation's.
    #[test]
    fn an_origin_can_be_what_the_picture_is_rooted_at() {
        let (agents, all) = family();
        let g = build(&agents, &all, Some("origin:human"));
        assert_eq!(node(&g, "origin:human").role, Role::Focus);
        assert_eq!(g.under, 6, "everything below it, however deep");
        assert!(
            g.nodes.iter().all(|n| n.role != Role::Ancestor),
            "nothing is above where the work came from"
        );
    }

    /// A count of what is under one conversation is a count of cards, and the page draws only the
    /// newest few. Where anything was left out, the line says "drawn under it" — it must not claim
    /// to have looked at conversations it never put on the canvas.
    #[test]
    fn what_is_under_one_conversation_is_counted_as_what_was_drawn() {
        let rs: Vec<AgentRunInfo> = (0..CHATS_PER_AGENT + 3)
            .map(|i| {
                let mut r = run(&format!("r{i}"), LAUNCHED_BY_HUMAN);
                (r.started_at, r.last_activity) = (i as u64, i as u64);
                r
            })
            .collect();
        let all = AllAgentRuns {
            total: rs.len(),
            agents: vec![runs("one", rs)],
        };
        let g = build(&[agent("one")], &all, Some("origin:human"));
        assert!(g.sampled);
        assert_eq!(g.summary(), "You · 6 conversations drawn under it");
    }

    /// The picture is centred on the origin, so an untouched view opens on the middle of the graph
    /// rather than on its top-left corner. Its *extent*, not its node centres: the columns are not
    /// all the same width, and it is the outermost edges that decide what looks centred.
    #[test]
    fn the_graph_is_centred_on_the_world_origin() {
        let (agents, all) = chain();
        let g = build(&agents, &all, None);
        let (x0, y0, x1, y1) = g.extent;
        assert!(x0.midpoint(x1).abs() < 1e-9, "x is centred on {}", x0.midpoint(x1));
        assert!(y0.midpoint(y1).abs() < 1e-9, "y is centred on {}", y0.midpoint(y1));
    }

    /// An agent that has never run has no conversation, and a conversation is the only thing this
    /// page draws — so a machine's idle definitions never stand in front of its history.
    #[test]
    fn an_agent_that_never_ran_is_not_on_the_canvas() {
        let (mut agents, all) = chain();
        agents.push(agent("idle"));
        let g = build(&agents, &all, None);
        assert!(g.nodes.iter().all(|n| n.agent != "idle"));
        assert_eq!(g.agents, 3, "the count is of the agents that are drawn");
    }

    /// A conversation whose agent has since been deleted still happened, and its card says so
    /// rather than dropping it.
    #[test]
    fn a_chat_of_a_deleted_agent_is_still_drawn() {
        let all = AllAgentRuns {
            total: 1,
            agents: vec![runs("gone", vec![run("r1", LAUNCHED_BY_HUMAN)])],
        };
        let g = build(&[], &all, None);
        let card = node(&g, "chat:gone/r1");
        assert_eq!(card.agent, "gone");
        assert_eq!(card.meta, "Chat of gone · no longer defined · started by you · finished");
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
        let g = build(&[agent("one")], &all, None);
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

    /// Whoever started a drawn conversation is drawn too, however long ago their own conversation
    /// was: without it the card would name an agent with nothing on the canvas to hang from.
    #[test]
    fn the_conversation_that_started_one_is_pulled_in_however_old_it_is() {
        let mut starter = run("old", LAUNCHED_BY_HUMAN);
        starter.started_at = 1;
        starter.last_activity = 1;
        let mut child = run("new", "agent:starter");
        child.started_at = 9_000;
        child.last_activity = 9_000;
        let all = AllAgentRuns {
            total: 2,
            agents: vec![
                runs("starter", vec![starter]),
                runs("child-agent", vec![child]),
            ],
        };
        let g = build(&[agent("starter"), agent("child-agent")], &all, None);
        assert!(joined(&g, "chat:starter/old", "chat:child-agent/new"));
        assert!(joined(&g, "origin:human", "chat:starter/old"));
    }

    /// The store names the agent that asked and not the conversation, so the edge leaves the one of
    /// that agent's that had already begun and began last — the one most likely to have been
    /// talking — and only that one. Six conversations of one agent would otherwise draw six edges
    /// into everything it ever started.
    #[test]
    fn one_edge_leaves_the_conversation_nearest_in_time() {
        let mut early = run("early", LAUNCHED_BY_HUMAN);
        (early.started_at, early.last_activity) = (10, 10);
        let mut mid = run("mid", LAUNCHED_BY_HUMAN);
        (mid.started_at, mid.last_activity) = (50, 50);
        let mut late = run("late", LAUNCHED_BY_HUMAN);
        (late.started_at, late.last_activity) = (900, 900);
        let mut child = run("child", "agent:one");
        (child.started_at, child.last_activity) = (60, 60);
        let all = AllAgentRuns {
            total: 4,
            agents: vec![
                runs("one", vec![early, mid, late]),
                runs("two", vec![child]),
            ],
        };
        let g = build(&[agent("one"), agent("two")], &all, None);
        assert!(joined(&g, "chat:one/mid", "chat:two/child"));
        assert!(!joined(&g, "chat:one/early", "chat:two/child"));
        assert!(!joined(&g, "chat:one/late", "chat:two/child"));
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
        let g = build(&[agent("one")], &all, None);
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
        let g = build(&[agent("one"), agent("two")], &all, None);
        assert!(!g.is_empty());
        assert!(g.nodes.iter().all(|n| n.x.is_finite() && n.y.is_finite()));
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

    /// A hit is inside the card a node was drawn as, not near it — and a conversation's card is
    /// taller than an origin's pill, so the two cannot share one height.
    #[test]
    fn a_node_is_hit_inside_its_own_card() {
        let (agents, all) = chain();
        let g = build(&agents, &all, None);
        let n = node(&g, "chat:one/r1").clone();
        assert!(n.hit(n.x, n.y));
        assert!(n.hit(n.x + n.w() / 2.0 - 1.0, n.y + CHAT_H / 2.0 - 1.0));
        assert!(!n.hit(n.x + n.w() / 2.0 + 2.0, n.y));
        assert!(!n.hit(n.x, n.y + CHAT_H));
        assert!(!node(&g, "origin:human").hit(0.0, ORIGIN_H));
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
            None,
        );
        assert!(g.is_empty());
        assert_eq!(g.summary(), "0 conversations");
        assert_eq!(g.at(0.0, 0.0), None);
    }

    /// The shape of a real machine, not of a fixture: 80 agents, 50 conversations each, most of
    /// them unattributed and a fifth started by one busy agent. Measured off this operator's own
    /// store on 2026-09-12 — 80 agents, 1212 runs, `launched_by` 888 empty / 226 `agent:adi-agent`
    /// / 89 human — and here because every number that matters is decided by this case and by no
    /// smaller one. The first version of this page was only ever seen against a store with one
    /// agent and five chats, and it shipped as a graph that fitted on screen at 6%.
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
                                match j % 5 {
                                    0 => "agent:a0",
                                    1 => LAUNCHED_BY_HUMAN,
                                    _ => "",
                                },
                            );
                            r.started_at = (i * 50 + j) as u64;
                            r.last_activity = r.started_at;
                            r
                        })
                        .collect();
                    runs(&format!("a{i}"), rs)
                })
                .collect(),
        };
        let g = build(&agents, &all, None);
        assert_eq!(g.total_chats, 4000);
        // The global cap, and above it only the conversations pulled in for having started one —
        // at most one per agent on the canvas, and on this fixture exactly one.
        assert!(
            (CHATS_DRAWN..=CHATS_DRAWN + g.agents).contains(&g.shown_chats),
            "{} cards drawn",
            g.shown_chats
        );
        assert!(g.nodes.len() < 200, "{} nodes", g.nodes.len());
        assert!(g.nodes.iter().all(|n| n.x.is_finite() && n.y.is_finite()));

        // One edge into a conversation, never one per conversation of the agent that asked: this is
        // the difference between a picture and a hairball. Measured against the real store, the
        // other rule drew 369 edges between the same 80 cards.
        assert!(
            g.edges.len() <= g.nodes.len(),
            "{} edges between {} cards",
            g.edges.len(),
            g.nodes.len()
        );

        // The shape of it, which is the whole point of the wrap: a picture a screen can hold, not
        // a line. Anything past about 6:1 either way is a smear on one axis at every zoom.
        let (x0, y0, x1, y1) = g.extent;
        let aspect = (x1 - x0) / (y1 - y0);
        assert!((0.2..6.0).contains(&aspect), "the graph is {aspect:.1}:1");
        // …and small enough that fitting it leaves the labels legible. On the 1160×690 stage this
        // operator's display gives the page: 6% before the caps, 23% with a tidy tree wrapped at
        // 20 rows, 39% at the 12 [`MAX_ROWS`] settled on.
        assert!(
            super::super::view::Viewport::fit(g.extent, 1160.0, 690.0).scale
                > super::super::paint::LABEL_SCALE,
            "fitting it would put the labels out"
        );

        // …and it is still a tree at that size: nobody's children are drawn through the middle of
        // anybody else's, which is the one thing a hundred cards could quietly take away.
        assert_eq!(children_stay_together(&g), "");

        // And a card left unconnected would be a box saying nothing: every conversation was started
        // by somebody, and whoever it was is on the canvas.
        let joined: std::collections::HashSet<usize> =
            g.edges.iter().flat_map(|e| [e.from, e.to]).collect();
        let loose = (0..g.nodes.len()).filter(|i| !joined.contains(i)).count();
        assert_eq!(loose, 0, "{loose} cards float unattached");
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
        let g = build(&state.agents, &all, None);
        assert!(g.nodes.iter().any(|n| n.id == "chat:one/r1"));
    }
}
