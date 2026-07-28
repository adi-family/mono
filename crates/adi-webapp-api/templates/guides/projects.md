# Projects

A project is a unit of work registered under `~/.adi/mono/projects/<id>/` with a `config.toml`
manifest and an optional `.adi/hive.yaml` (see `services.md`).

## Where it lives
- `~/.adi/mono/projects/<id>/config.toml` — name, description, parent.
- `~/.adi/mono/projects/<id>/.adi/hive.yaml` — the services the project supervises.

## Do it
- List: `adi projects list` or `GET /api/projects`. Panel: `/projects`.
- Create: `POST /api/projects/create` (`{ "name", "description", "parent" }`). `parent` is an
  optional project id, so a project can nest under another.
- Archive / restore / remove: `POST /api/projects/archive` · `/unarchive` · `/remove`.

## Notes
- Sub-projects nest via `parent`; the explorer shows the hierarchy and badges each project
  with its open-task count.
- Keep a project's files under its own directory — don't scatter its state elsewhere in the
  store.
- A capability that only serves this project belongs in a tool filed under it —
  `adi tools add <name> --project <id>`; it runs in the project's directory, against the
  project's database. See `tools.md`.
