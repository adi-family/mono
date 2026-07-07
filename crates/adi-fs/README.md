# adi-fs

An **isolated, base-directory-jailed filesystem** — a small, dependency-free primitive.

A [`Jail`] is rooted at one base directory. Every operation takes a *relative* path and stays
confined to that base. The relative path is normalized and any component that would climb out
is refused:

- `..` parent components,
- absolute paths (`/etc/passwd`),
- Windows drive/UNC prefixes,
- and — as defense in depth — a symlink whose target resolves outside the base
  (the resolved path is canonicalized and checked against the canonicalized base).

**There is no going backward past the base.**

## API

```rust
use adi_fs::Jail;

let jail = Jail::new("/Users/me/.adi/mono/projects/demo");

jail.list("")?;                          // browse a directory (dirs first, then by name)
jail.read_to_string(".adi/hive.yaml")?;  // read a text file
jail.write(".adi/hive.yaml", bytes)?;    // atomic write (creates parents inside the jail)
jail.metadata("config.toml")?;           // stat
jail.exists(".adi/hive.yaml");           // membership test
```

Errors are a single `Error` enum: `Escape` (the security boundary), `NotFound`, `NotAFile`,
`NotText` (non-UTF-8), and `Io`.

## Where it's used

The adi control panel (`app.adi`) exposes each project's own directory through a `Jail` so the
project can browse and edit the files under it — its `.adi/hive.yaml` and anything beside it —
without ever reaching the rest of the disk. The wire DTOs that surface a jail over HTTP live in
`adi-webapp-api`; this crate stays a reusable primitive with no dependencies.
