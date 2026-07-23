ADI for Windows
===============

The ADI platform (DNS + control panel + services), cross-compiled for Windows x64.

There is no separate native window on Windows: the macOS app is a thin Swift wrapper around
the same binaries, and its control panel is a web UI. On Windows you use that web control
panel directly in your browser — the launcher opens it for you.


What's in this folder
---------------------

  adi-mono.exe   The CLI and the brain — every command (`adi-mono up`, `enable`, `status`, ...).
  adi-dns.exe    The .test / .adi split-DNS resolver.
  adi-hive.exe   The front-door reverse proxy that serves *.adi hosts.
  adi-app.exe    The web control panel (served on 127.0.0.1:<port>).

  Start ADI.cmd          Start all services and open the control panel in your browser.
  Stop ADI.cmd           Stop all services.
  Add ADI to PATH.cmd    Add this folder to your user PATH (no admin) — recommended.
  adi.cmd                Run the CLI as `adi ...` instead of `adi-mono ...`.


Quick start
-----------

1. Unzip this folder somewhere permanent (e.g. C:\Program Files\ADI or your user folder).
2. Double-click **Start ADI.cmd**.
   - It runs `adi-mono up`, which registers each service as a per-user Scheduled Task
     (they auto-start at logon and restart on failure — the Windows analog of the macOS
     LaunchAgents), then opens the control panel.
3. That's it. The control panel is a normal web page at http://127.0.0.1:<port>/ .


How supervision works on Windows
--------------------------------

Instead of macOS launchd, each service is a **Task Scheduler** task named `family.adi.app.*`,
created with `schtasks`. They run as *you* (no admin), start at logon, and restart on failure.
Manage them from a terminal:

  adi-mono up           Start everything (idempotent; safe to re-run).
  adi-mono status       Show each service: enabled / running / detail.
  adi-mono disable      Stop and unregister everything.

or with the Task Scheduler UI (look under Task Scheduler Library for `family.adi.app.*`).


The .adi domain (optional, needs one admin prompt)
--------------------------------------------------

To reach the control panel at a friendly name like http://app.adi/ , install the DNS route:

  adi-mono dns install-route

On Windows this adds a **DNS Client NRPT rule** pointing the whole `.adi` namespace at the
local resolver — that one step raises a single UAC prompt. The resolver then binds 127.0.0.1:53
(NRPT can only redirect a namespace, not a port), and the front door serves *.adi on
127.0.0.53:80. Remove it with:

  adi-mono dns remove-route

You do **not** need this to use ADI — http://127.0.0.1:<port>/ always works. `Start ADI.cmd`
reads that direct URL from `adi-mono status --json` and opens it.


Requirements & notes
--------------------

* Windows 10/11 x64.
* PowerShell (built in) is used for the launcher and for the NRPT route step.
* Some features that shell out to Unix tools on macOS (project hooks and dashboard service
  runners that execute `sh` scripts, `lsof`/`docker` port helpers) are not yet adapted to
  `cmd`/PowerShell on Windows. The core platform — services, DNS, secrets, agents (the
  headless `process`/`harness` backends), the control panel — runs natively. The interactive
  `tmux:*` agent backend is macOS/Linux-only; use the `process:*` or `harness:*` backends.
* Run **Add ADI to PATH.cmd** once so `adi-mono` is callable by name — the `harness:adi`
  agent backend re-invokes it that way.
