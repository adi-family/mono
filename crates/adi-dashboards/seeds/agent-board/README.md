# Agent board

Talk to this machine's agents: pick one, pick a conversation or start a new one, read it as it
streams, answer it.

It is also the ADI home screen's chat. `frontend/widget.html` is the widget the new UI frames
(`adi-widget-size: half` — the right half of the home screen), and it mounts the same chat
module the board's own page does, `frontend/modules/chat.ts`.

The board keeps no state. Every agent, run and transcript belongs to the control panel, and
`backend/routes/*` relay to its agent API (`backend/lib/panel.ts`) — server-side, because the
panel answers `/api` only to its own origin or to a caller with none.

| route | panel endpoint |
| --- | --- |
| `GET /api/agents` | `GET /api/agents` |
| `POST /api/runs {name}` | `POST /api/agents/runs` |
| `POST /api/start {name, message}` | `POST /api/agents/run` |
| `POST /api/peek {name, run_id}` | `POST /api/agents/run/peek` |
| `POST /api/reply {name, run_id, message}` | `POST /api/agents/run/reply` |
| `POST /api/stop {name, run_id}` | `POST /api/agents/run/stop` |

Built on one machine first; meant to ship seeded with ADI as the default home-screen chat. Its
authored files are kept in the repo at `crates/adi-dashboards/seeds/agent-board/` — the panel
generates the rest (`frontend/index.*`, `backend/index.ts`, `.adi/hive.yaml`) when it creates or
migrates a dashboard. Nothing copies the seed onto a machine yet: an edit made to one copy has
to be carried to the other by hand until something does.
