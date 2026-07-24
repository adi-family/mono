# Tasks

A simple task tree stored at `~/.adi/mono/tasks/tasks.json`. Use it to track the work you and
the user agree on, so progress survives across runs.

## Do it
- List: `adi tasks list` or `GET /api/tasks`. Panel: `/tasks`.
- Create: `POST /api/tasks/create` (`{ "title", "project", "parent", "tag", "details" }`).
  `project` and `parent` are optional ids — a task can belong to a project and nest under
  another task.
- Close / reopen / archive / delete: `POST /api/tasks/archive` · `/reopen` · `/delete`.

## Notes
- The explorer's per-project badge counts open tasks — set `project` so work shows up where it
  belongs.
- Prefer small, verifiable tasks; close each as you confirm it rather than batching.
