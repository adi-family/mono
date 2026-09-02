ADI for Windows
===============

ADI is your machine's own control plane: a control panel in your browser, a local DNS resolver
that serves the `.adi` domain, and the services your projects declare. It runs entirely on this
computer.


Install
-------

Run **ADI-Setup-x64.exe** and click through it. That is the whole procedure.

It installs into your own user account (`%LOCALAPPDATA%\Programs\ADI`), so installing raises no
administrator prompt. One step during the install does ask for administrator, once: pointing the
`.adi` namespace at the local resolver, so `http://app.adi/` works. Say no to it and everything
still runs — the control panel opens on `http://127.0.0.1:<port>/` instead, and you can set the
domain up later from ADI's icon by the clock.

Afterwards there is one thing to open: **ADI**, in the Start menu and on the desktop.


Using it
--------

Opening ADI starts the platform and takes you to the control panel. It then stays in the
notification area, by the clock:

  * **click the icon** to open the control panel
  * **right-click** for: open the panel, start or stop ADI, set up the `.adi` domain, quit
  * **Quit** closes the tray icon only. The services keep running — they are scheduled tasks and
    start again at every logon. **Stop ADI** is what stops them.

In a terminal the whole platform is the `adi` command (`adi status`, `adi up`, `adi projects`,
...). The installer puts it on your PATH, so open a *new* terminal after installing. `adi-mono`
is the same program under its full name.


What is actually installed
--------------------------

    %LOCALAPPDATA%\Programs\ADI\
      bin\ADI.exe          the app you open: starts the stack, opens the panel, lives in the tray
      bin\adi-mono.exe     the CLI and the brain -- every command
      bin\adi-dns.exe      the .test / .adi split-DNS resolver
      bin\adi-hive.exe     the front-door reverse proxy that serves *.adi hosts
      bin\adi-app.exe      the web control panel
      README.txt, LICENSE.txt, VERSION, Uninstall ADI.exe

The platform is four separate programs because it is four separate services, each supervised on
its own — the same four that live inside `ADI.app` on macOS. They are in `bin\` because there is
never a reason to pick one: `ADI.exe` is the app, `adi` is the command.

Your data is somewhere else entirely: `%USERPROFILE%\.adi` — projects, secrets, the database,
every agent transcript. Uninstalling never touches it.


How supervision works
---------------------

Each service is a **Task Scheduler** task named `family.adi.app.*`, created with `schtasks`. They
run as you (no administrator), start at logon, and restart on failure — the Windows counterpart
of the macOS LaunchAgents. From a terminal:

    adi up           Start everything (idempotent; safe to re-run).
    adi status       Each service: enabled / running / detail.
    adi disable      Stop and unregister everything.

or from the Task Scheduler UI, under Task Scheduler Library, as `family.adi.app.*`.


The .adi domain
---------------

The installer offers this, and ADI's tray menu offers it again if you skipped it:

    adi dns install-route

On Windows this adds a **DNS Client NRPT rule** pointing the whole `.adi` namespace at the local
resolver — the one step that raises a UAC prompt. The resolver then binds `127.0.0.1:53` (NRPT
can redirect a namespace, not a port), and the front door serves `*.adi` on `127.0.0.53:80`.
Remove it with `adi dns remove-route`.

You never need it: `http://127.0.0.1:<port>/` always works, and ADI opens that URL when the
domain is not set up.


Updating
--------

ADI updates itself: the control panel checks the published manifest, and takes the release's
`ADI-windows-x64.zip` — the same files, in the same layout, dropped into `bin\`. You do not
re-run the installer to update.


Uninstalling
------------

**Settings → Apps → Installed apps → ADI → Uninstall**, or `Uninstall ADI.exe` in the install
folder. It stops the services, unregisters their scheduled tasks, gives back the `.adi` domain
(one administrator prompt), takes ADI off your PATH, and removes the folder.

It does not touch `%USERPROFILE%\.adi`. Reinstalling picks your store straight back up; delete
that folder by hand if you really want ADI gone.


Notes and limits
----------------

* Windows 10/11 x64. PowerShell (built in) is used for the PATH entry and the NRPT route step.
* The installer is not code-signed yet, so SmartScreen shows "Windows protected your PC" on
  first run — **More info → Run anyway**.
* Some features shell out to Unix tools and are not adapted to `cmd`/PowerShell yet: project
  hooks and dashboard service runners that execute `sh` scripts, and the `lsof`/`docker` port
  helpers. The core platform — services, DNS, secrets, agents (the headless `process`/`harness`
  backends), the control panel — runs natively. The interactive `tmux:*` agent backend is
  macOS/Linux-only; use `process:*` or `harness:*`.


The archive
-----------

Releases also carry **ADI-windows-x64.zip**: the same files without the installer, for an
unattended or portable install. Unpack it somewhere permanent and run `bin\ADI.exe`. Nothing
registers itself until you do — but you also get no Start-menu entry, no PATH, and no entry in
Installed apps. The installer is the supported route; this is the one the updater uses.
