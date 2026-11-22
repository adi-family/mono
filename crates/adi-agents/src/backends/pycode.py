"""The Python a conversation keeps between `PyCode` calls.

Two ways in, one namespace:

  pycode.py kernel --state F --shell-cwd F --shell-env F --shared PREFIX --home DIR --idle SECONDS --protocol N
      The conversation's interpreter. Listens on a Unix socket, runs one cell per connection in
      a namespace that outlives the call, and exits when it has been idle too long or when its
      state file is gone (the session was deleted) or names another kernel (it was replaced).

  pycode.py run SCRIPT [--shared PREFIX] [--absent NAMES]
      One cell in a fresh interpreter, for a background job: same helpers, same display of
      what it evaluates, nothing kept afterwards.

Written against Python 3.8 so the interpreter a machine happens to ship is enough. Standard
library only: this file is embedded in the adi binary and dropped beside a session, so there is
nowhere to install a dependency from.

The Rust half is `interpreter.rs`, shared with `tscode.ts`; the wire is one JSON object per line, one request and
one reply per connection.

For a code-only agent a request also says where the cell's *tools* are answered, and the
platform's other tools become functions for as long as the cell runs — see "the platform's
tools, as functions" below.
"""

import ast
import io
import json
import linecache
import os
import pprint
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import types

HERE = os.path.abspath(__file__)

# Never carried between calls: the shell maintains these itself, and a copy frozen from an earlier
# call would only ever be wrong. The same four `shell.rs` filters out of what Bash carries.
UNCARRIED = ("PWD", "OLDPWD", "SHLVL", "_")

# How much of one stream `sh()` keeps. Output is data here — it gets parsed, not just read — so
# the cap is far above what a call may print; it exists to keep a runaway command from taking the
# interpreter's memory with it.
MAX_CAPTURE = 64 << 20

# How long `sh()` gives its readers once the command is gone. A pipe stays open while any writer
# holds it, and `(sleep 60 &)` exits at once having handed its stdout to an orphan — waiting for
# end-of-file there is waiting for the orphan.
DRAIN_GRACE = 0.2


# ───────────────────────────── helpers the model calls ─────────────────────────────


class Sh(str):
    """What a command printed, as a string, with how it ended beside it.

    The string is stdout with trailing newlines dropped — what `$(...)` would have given — so
    `head = sh("git rev-parse HEAD")` is usable as written. `.stdout` and `.stderr` are the
    streams untouched, `.code` the exit status.
    """

    def __new__(cls, stdout, stderr, code, cmd, truncated=False, echoed=False):
        self = super().__new__(cls, stdout.rstrip("\n"))
        self.echoed = echoed
        self.stdout = stdout
        self.stderr = stderr
        self.code = code
        self.cmd = cmd
        self.truncated = truncated
        return self

    @property
    def ok(self):
        return self.code == 0

    @property
    def lines(self):
        return self.splitlines()

    def json(self):
        return json.loads(self.stdout)

    def __reduce__(self):
        # A `str` subclass with a constructor of its own does not pickle by default, and a result
        # handed to a worker process or cached to disk should.
        return (Sh, (self.stdout, self.stderr, self.code, self.cmd, self.truncated, self.echoed))

    def __repr__(self):
        # Shown when a command is a top-level expression, and read the way Bash's result is:
        # both streams, the status only when it is not zero, and a word when there was nothing.
        if self.echoed:
            # Already copied through as it ran; saying it again would only double it.
            return "(exit %s)" % self.code if self.code != 0 else ""
        text = self.stdout
        if self.stderr.strip():
            if text and not text.endswith("\n"):
                text += "\n"
            text += self.stderr
        text = text.rstrip("\n")
        if self.truncated:
            text += "\n[output was cut at %d bytes per stream]" % MAX_CAPTURE
        if self.code != 0:
            return "(exit %s)\n%s" % (self.code, text) if text else "(exit %s)" % self.code
        return text if text.strip() else "(no output)"


class ShError(Exception):
    """A command run with `sh()` exited non-zero, or outlived its timeout."""

    def __init__(self, message, result):
        super().__init__(message)
        self.result = result
        self.code = result.code
        self.stdout = result.stdout
        self.stderr = result.stderr


class _Pump(threading.Thread):
    """Empty one of a command's pipes as it fills, optionally copying it through as it goes."""

    def __init__(self, stream, tee):
        super().__init__(daemon=True)
        self.stream = stream
        self.tee = tee
        self.chunks = []
        self.size = 0
        self.truncated = False
        self.start()

    def run(self):
        fd = self.stream.fileno()
        try:
            while True:
                chunk = os.read(fd, 65536)
                if not chunk:
                    break
                if self.tee is not None:
                    try:
                        os.write(self.tee, chunk)
                    except OSError:
                        pass
                if self.size < MAX_CAPTURE:
                    self.chunks.append(chunk[: MAX_CAPTURE - self.size])
                    self.truncated = self.truncated or len(chunk) > MAX_CAPTURE - self.size
                else:
                    self.truncated = True
                self.size += len(chunk)
        except (OSError, ValueError):
            pass

    def text(self):
        self.join(DRAIN_GRACE)
        return b"".join(list(self.chunks)).decode("utf-8", "replace")


def _stop(proc):
    """End a command and everything it started. It leads a session of its own, so the group is
    the whole tree — killing only the `sh` would leave the build it launched running."""
    for sig, grace in ((signal.SIGTERM, 1.0), (signal.SIGKILL, 1.0)):
        try:
            os.killpg(proc.pid, sig)
        except (ProcessLookupError, PermissionError):
            pass
        try:
            proc.wait(grace)
            return
        except subprocess.TimeoutExpired:
            continue


def sh(cmd, check=True, timeout=None, input=None, cwd=None, env=None, echo=False):
    """Run a command and return what it printed (see `Sh`).

    A string runs through `/bin/sh`; a list or tuple runs as written, with no shell and so no
    quoting to get wrong. A non-zero exit raises `ShError` carrying the output — pass
    `check=False` where failing is an answer rather than a fault (`grep`, `diff`, `test`).
    `echo=True` also copies the output through as it is produced, which is how to watch a long
    command instead of reading it afterwards.
    """
    if isinstance(cmd, (str, bytes)):
        argv = ["/bin/sh", "-c", cmd]
        shown = cmd if isinstance(cmd, str) else cmd.decode("utf-8", "replace")
    else:
        argv = [os.fspath(part) for part in cmd]
        shown = " ".join(str(part) for part in argv)
    if isinstance(input, str):
        input = input.encode()

    where = os.path.abspath(cwd) if cwd is not None else os.getcwd()
    child_env = dict(os.environ if env is None else env)
    # A program that reads $PWD rather than asking the kernel would otherwise be told where the
    # run began, not where this command starts.
    child_env["PWD"] = where

    sys.stdout.flush()
    sys.stderr.flush()
    try:
        proc = subprocess.Popen(
            argv,
            stdin=subprocess.PIPE if input is not None else subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            cwd=where,
            env=child_env,
            start_new_session=True,
        )
    except OSError as error:
        # A program that is not there. Through the shell that is status 127 and a line on
        # stderr; a list is given the same ending rather than a traceback through `subprocess`,
        # so `check=False` means the same thing whichever way the command was written.
        missing = isinstance(error, FileNotFoundError) and not os.path.isdir(where)
        what = where if missing else argv[0]
        got = Sh("", "%s: %s\n" % (what, error.strerror or error), 127, shown)
        if check:
            raise ShError("couldn't run %s%s" % (shown, _tail(got)), got) from None
        return got
    # `Popen.wait` answers a first interrupt by waiting a quarter of a second for the child to
    # die of the same Ctrl-C. This child leads its own session and was sent nothing, so that wait
    # is only a delay before `_stop` does the job.
    proc._sigint_wait_secs = 0
    out = _Pump(proc.stdout, 1 if echo else None)
    err = _Pump(proc.stderr, 2 if echo else None)

    def result(code):
        return Sh(out.text(), err.text(), code, shown, out.truncated or err.truncated, echo)

    try:
        if input is not None:
            try:
                proc.stdin.write(input)
                proc.stdin.close()
            except (BrokenPipeError, OSError):
                pass
        code = proc.wait(timeout)
    except subprocess.TimeoutExpired:
        _stop(proc)
        got = result(proc.returncode if proc.returncode is not None else -9)
        raise ShError(
            "command still running after %ss and was killed: %s%s" % (timeout, shown, _tail(got)), got
        ) from None
    except BaseException:
        # The call's own deadline, or anything else that unwinds through here: the command must
        # not outlive the cell that started it, and what it had printed is worth showing — it is
        # usually the only clue to why it was slow.
        _stop(proc)
        if not echo:
            sys.stdout.write(out.text())
            sys.stderr.write(err.text())
        raise

    got = result(code)
    if check and code != 0:
        raise ShError("command exited %s: %s%s" % (code, shown, _tail(got)), got)
    return got


# Where shared values are kept: a path prefix, to which a name and `.json` are added. Set by
# whoever starts this file — it is the session's own namespace, so the values are swept with the
# conversation, and the TypeScript interpreter is handed the same prefix.
_SHARED = None


def _shared_file(name):
    if not isinstance(name, str) or not name or not name.isascii() or not all(c.isalnum() or c in "_-" for c in name):
        raise ValueError("a shared name is letters, digits, _ and -, not %r" % (name,))
    if _SHARED is None:
        raise RuntimeError("there is nowhere to share a value from here")
    return "%s%s.json" % (_SHARED, name)


def share(name, value):
    """Keep `value` under `name`, outside this interpreter.

    What is kept is JSON, so it is found again — with `shared(name)` — wherever JSON can go and
    a variable cannot: by a background cell, by this conversation after its interpreter is
    restarted or lost, and by its TypeScript interpreter. For a result, a list of names, a
    configuration; something large belongs in a file of its own.
    """
    path = _shared_file(name)
    try:
        text = json.dumps(value, allow_nan=False)
    except (TypeError, ValueError) as error:
        raise TypeError(
            "share(%r): only what JSON can hold can be shared (%s). Convert it first — a set to a "
            "list, an object to a dict — or write it to a file." % (name, error)
        ) from None
    _replace(path, text)


def shared(name=None):
    """The value kept under `name` by `share` — or, with no name, the names there are."""
    if name is None:
        import glob

        if _SHARED is None:
            return []
        return sorted(found[len(_SHARED):-len(".json")] for found in glob.glob(glob.escape(_SHARED) + "*.json"))
    path = _shared_file(name)
    try:
        with open(path) as f:
            return json.load(f)
    except FileNotFoundError:
        raise KeyError("nothing is shared as %r. What is: %s" % (name, ", ".join(shared()) or "nothing yet")) from None


def _tail(result, limit=4000):
    """The end of what a failed command printed, for the exception that reports it."""
    text = (result.stderr if result.stderr.strip() else result.stdout).rstrip("\n")
    if not text.strip():
        return ""
    if len(text) > limit:
        text = "[…]" + text[-limit:]
    return "\n" + text


# ───────────────────────────── the platform's tools, as functions ─────────────────────────────

# A code-only agent has one tool — this interpreter — and reaches every other from inside a cell:
# `Read(path=...)`, `mcp.n8n.call("list", {})`. The tools themselves are not here. They are code
# in whoever is serving the turn (`code_only.rs`), which listens on a socket for as long as a cell
# runs and says where in the cell's request. A call is one connection: a line of JSON out, a line
# of JSON back.


class ToolError(Exception):
    """A tool called from code failed, or could not be reached. The message is what the tool
    said."""

    def __init__(self, message, tool=None):
        super().__init__(message)
        self.tool = tool


# What the cell now running was told about its tools — where they are answered, what they are
# called — or `None` between cells and for an agent that is not code-only.
_DOOR = None

# Around a tool that moves the conversation's shell: two functions, called before and after.
# `Bash` starts where the shell was last recorded and records where it ends, so the cell's own
# position is written down before the call and read back after it — a `cd` in either is then a
# `cd` in both, mid-cell as between cells.
_SHELL_SYNC = None


class ToolText(str):
    """What a tool answered with. A string in every way; a class of its own so that a call
    standing on a line of its own shows what the tool said — as a bare `sh(...)` shows what a
    command printed — rather than the quoted and escaped `repr` of it."""

    __slots__ = ()


def _ask(tool, message):
    """Send one call through the door and return its answer."""
    door = _DOOR
    if door is None:
        raise ToolError(
            "%s can only be called while a cell of yours is running: not from a background "
            "script, and not from a thread that outlived its cell" % tool,
            tool,
        )
    try:
        payload = json.dumps(message).encode("utf-8") + b"\n"
    except (TypeError, ValueError) as error:
        raise ToolError("%s: its arguments are not something JSON can hold (%s)" % (tool, error), tool) from None
    conn = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    chunks = []
    try:
        # Blocking, and deliberately: the call's deadline arrives as an interrupt, which ends a
        # wait here exactly as it ends one on a command.
        conn.connect(door["socket"])
        conn.sendall(payload)
        while True:
            chunk = conn.recv(1 << 16)
            if not chunk:
                break
            chunks.append(chunk)
    except OSError as error:
        raise ToolError("couldn't reach the platform for %s: %s" % (tool, error), tool) from None
    finally:
        conn.close()
    try:
        reply = json.loads(b"".join(chunks).decode("utf-8"))
    except ValueError:
        raise ToolError("%s got no answer" % tool, tool) from None
    if not reply.get("ok"):
        raise ToolError(str(reply.get("text") or "%s failed" % tool), tool)
    return reply


def _arguments(what, args, kwargs):
    """A call's arguments: keywords, or one dict of them, or both."""
    if len(args) > 1 or (args and not isinstance(args[0], dict)):
        raise TypeError("%s takes keyword arguments, or one dict of them" % what)
    given = dict(args[0]) if args else {}
    given.update(kwargs)
    return given


def _tool(name):
    """The platform tool called `name`, as a function of keyword arguments."""

    def call(*args, **kwargs):
        given = _arguments(name, args, kwargs)
        sync = _SHELL_SYNC if name == "Bash" else None
        if sync:
            sync[0]()
        try:
            return ToolText(_ask(name, {"tool": name, "input": given, "cwd": os.getcwd()}).get("text") or "")
        finally:
            if sync:
                sync[1]()

    call.__name__ = call.__qualname__ = name
    call.__doc__ = "The platform's %s tool: returns the text it answers with, raises ToolError when it fails." % name
    return call


class _McpServer:
    """One MCP server attached to the agent: `tools()` lists what it offers, `call(name, {...})`
    calls one, and any other attribute is that call — `mcp.n8n.list_workflows(active=True)`."""

    def __init__(self, id):
        self._id = id

    def tools(self):
        return _ask("mcp.%s.tools" % self._id, {"mcp": self._id, "list": True}).get("value") or []

    def call(self, tool, *args, **kwargs):
        if not isinstance(tool, str) or not tool:
            raise TypeError("mcp.%s.call takes the tool's name first: call('name', {...})" % self._id)
        what = "mcp.%s.%s" % (self._id, tool)
        reply = _ask(what, {"mcp": self._id, "tool": tool, "input": _arguments(what, args, kwargs)})
        value = reply.get("value")
        return value if value is not None else ToolText(reply.get("text") or "")

    def __getattr__(self, tool):
        if tool.startswith("_"):
            raise AttributeError(tool)
        return lambda *args, **kwargs: self.call(tool, *args, **kwargs)

    def __repr__(self):
        return "<MCP server %s: .tools() lists what it offers, .call(name, {...}) calls one>" % self._id


class _Mcp:
    """The MCP servers attached to the agent, by id: `mcp.n8n`, `mcp["my-server"]`."""

    def __init__(self, ids):
        self._servers = {id: _McpServer(id) for id in ids}

    def _missing(self, id):
        return "no MCP server named %s is attached to you — %s" % (
            id,
            ("the ones that are: " + ", ".join(self._servers)) if self._servers else "none is",
        )

    def __getattr__(self, id):
        if id.startswith("_"):
            raise AttributeError(id)
        try:
            return self._servers[id]
        except KeyError:
            raise AttributeError(self._missing(id)) from None

    def __getitem__(self, id):
        try:
            return self._servers[id]
        except KeyError:
            raise KeyError(self._missing(id)) from None

    def __iter__(self):
        return iter(self._servers)

    def __len__(self):
        return len(self._servers)

    def __dir__(self):
        return list(self._servers)

    def __repr__(self):
        return "<MCP servers: %s>" % (", ".join(self._servers) or "none")


def _offer(ns, tools, sync=None):
    """Put a cell's tools in reach, or take them away again. The functions are names in the
    cell's namespace, set afresh for every cell that is offered them, so a cell that happened to
    define a `Read` of its own does not cost the next one the tool."""
    global _DOOR, _SHELL_SYNC
    _DOOR = tools or None
    _SHELL_SYNC = sync if tools else None
    if not tools:
        return
    for name in tools.get("names") or []:
        ns[name] = _tool(name)
    ns["ToolError"] = ToolError
    ns["mcp"] = _Mcp(tools.get("servers") or [])


# ───────────────────────────── running one cell ─────────────────────────────


class _Unparsed(Exception):
    """Internal: the cell is not valid Python, so none of it ran."""

    def __init__(self, error):
        super().__init__(str(error))
        self.error = error


class _Stderr(io.TextIOWrapper):
    """Unbuffered, and it empties stdout first — so a traceback lands after the prints that led
    to it even though stdout is buffered."""

    def write(self, text):
        try:
            sys.stdout.flush()
        except (OSError, ValueError):
            pass
        return super().write(text)


def _buffer_output():
    """Give the interpreter a buffered stdout over the same descriptor.

    Output is captured into a file and read once the cell ends, so nothing is gained by writing
    each `print` through as it happens — and a great deal is lost: measured at half a
    millisecond a line, which turns a loop that prints a hundred thousand rows into a minute.
    What buffering would cost is order, and two things keep that: stderr flushes stdout before
    it writes, and so does anything about to start a process that will write for itself.
    """
    sys.stdout = io.TextIOWrapper(
        io.BufferedWriter(io.FileIO(1, "w", closefd=False), 1 << 16),
        encoding="utf-8", errors="backslashreplace",
    )
    sys.stderr = _Stderr(
        io.FileIO(2, "w", closefd=False),
        encoding="utf-8", errors="backslashreplace", write_through=True,
    )

    spawning = ("subprocess.Popen", "os.system", "os.posix_spawn", "os.fork", "os.forkpty", "os.exec")

    def flush_before_a_child_writes(event, _args):
        if event in spawning:
            try:
                sys.stdout.flush()
            except (OSError, ValueError):
                pass

    sys.addaudithook(flush_before_a_child_writes)


class Cells:
    """A namespace and the machinery that runs source in it, one cell at a time."""

    def __init__(self):
        import pathlib
        import re
        import shutil

        # A real `__main__` module rather than a bare dict: pickling, dataclasses and
        # multiprocessing all look a class up through `sys.modules[cls.__module__]`. The module
        # this file was run as is kept referenced, so replacing it cannot tear its globals down.
        self._host = sys.modules.get("__main__")
        main = types.ModuleType("__main__")
        sys.modules["__main__"] = main
        self.ns = main.__dict__
        self.ns.update(
            sh=sh,
            Sh=Sh,
            ShError=ShError,
            share=share,
            shared=shared,
            os=os,
            sys=sys,
            re=re,
            json=json,
            subprocess=subprocess,
            shutil=shutil,
            time=time,
            pathlib=pathlib,
            Path=pathlib.Path,
        )
        self.count = 0
        self.loop = None
        self.in_cell = False
        self.ended = threading.Event()
        self.stopped_by = None

    def on_sigint(self, _signum, _frame):
        # Only a running cell is interruptible. A signal that lands a moment late — after the
        # cell ended, while its reply is being written — must not take the reply with it.
        if self.in_cell:
            raise KeyboardInterrupt

    def interrupt(self, why):
        """Stop the running cell, from another thread. `why` is `"timeout"` or `"abandoned"`.

        A real signal rather than `_thread.interrupt_main()`: only a signal interrupts a blocking
        call (a sleep, a wait on a child, a socket read). Sent again while the cell is still
        going, because a cell is free to catch the first one — but slowly, so that `sh()` has
        time to finish killing the command it was waiting on before the next arrives.
        """
        if not self.in_cell:
            return
        self.stopped_by = self.stopped_by or why
        main_thread = threading.main_thread().ident
        for _ in range(4):
            if not self.in_cell:
                return
            try:
                signal.pthread_kill(main_thread, signal.SIGINT)
            except (OSError, ValueError):
                return
            if self.ended.wait(3.0):
                return

    def run(self, code, timeout=None):
        """Run `code`. Returns `"ok"`, `"error"` or `"interrupted"`; what it printed, and any
        traceback, have gone to stdout/stderr like everything else the cell wrote."""
        self.count += 1
        name = "<cell %d>" % self.count
        # Registered so a traceback can quote the line — including, later, a line of a function
        # an earlier cell defined.
        linecache.cache[name] = (len(code), None, code.splitlines(True), name)

        self.stopped_by = None
        self.ended.clear()
        timer = None
        if timeout:
            timer = threading.Timer(timeout, self.interrupt, ("timeout",))
            timer.daemon = True
            timer.start()

        status = "ok"
        try:
            self.in_cell = True
            try:
                self._execute(code, name)
            finally:
                self.in_cell = False
                self.ended.set()
                if timer is not None:
                    timer.cancel()
        except KeyboardInterrupt:
            # The traceback is kept, cut down to the cell's own frames: it names the line the
            # cell was stuck on, and the library frames beneath it are only where the wait was.
            status = "interrupted"
            self._report(*sys.exc_info(), own_only=True)
        except _Unparsed as unparsed:
            import traceback

            status = "error"
            sys.stderr.write("".join(traceback.format_exception_only(SyntaxError, unparsed.error)))
        except SystemExit as exit_:
            # `sys.exit()` in a cell ends the cell, not the conversation's interpreter.
            if exit_.code not in (None, 0):
                status = "error"
                print("SystemExit: %s" % (exit_.code,), file=sys.stderr)
        except BaseException:
            status = "error"
            self._report(*sys.exc_info())
        try:
            sys.stdout.flush()
            sys.stderr.flush()
        except (OSError, ValueError):
            pass
        return status

    def _execute(self, code, name):
        flags = getattr(ast, "PyCF_ALLOW_TOP_LEVEL_AWAIT", 0)
        try:
            tree = ast.parse(code, name, "exec")
        except SyntaxError as error:
            # Nothing ran. Shown as the error alone: the frames above it are the parser's.
            raise _Unparsed(error) from None
        for node in tree.body:
            # Every top-level expression is evaluated for display; `_show` decides which are
            # worth it. A statement is never shown, and never was.
            if isinstance(node, ast.Expr):
                compiled = compile(ast.Expression(node.value), name, "eval", flags=flags)
                value = self._await(eval(compiled, self.ns))
                self._show(value, node is tree.body[-1])
            else:
                module = ast.Module(body=[node], type_ignores=[])
                self._await(eval(compile(module, name, "exec", flags=flags), self.ns))

    def _await(self, value):
        """Top-level `await` works: a cell compiled with it comes back as a coroutine to drive."""
        if not isinstance(value, types.CoroutineType):
            return value
        # Imported here: it is a tenth of the interpreter's start-up, and most cells never await.
        import asyncio

        if self.loop is None or self.loop.is_closed():
            self.loop = asyncio.new_event_loop()
            asyncio.set_event_loop(self.loop)
        return self.loop.run_until_complete(value)

    def _show(self, value, last):
        """Print what a top-level expression came to: a command's output wherever it stands, so
        three bare `sh(...)` lines read like the three commands they are, and any other value
        only when it is the cell's last line, as a notebook would.

        It used to be every expression, as the interactive interpreter does. The first agent to
        use the tool for real showed what that costs: half of what a cell calls is called for
        its effect and returns something anyway — `write_text` its byte count, `pop` its item —
        and each of those landed in the output as a number nobody asked for.
        """
        if isinstance(value, Sh):
            text = repr(value)
            if text:
                print(text)
            return
        if isinstance(value, ToolText):
            # What a tool answered, standing on a line of its own: shown as the tool said it.
            if value.strip():
                print(value.rstrip("\n"))
            return
        if not last or value is None:
            return
        self.ns["_"] = value
        if isinstance(value, (dict, list, tuple, set, frozenset)) and not hasattr(value, "_fields"):
            try:
                # In the order it is in, not sorted: a dict's order is part of what it is, and a
                # display that rearranges it answers "what order are these in?" wrongly.
                text = pprint.pformat(value, width=120, compact=True, sort_dicts=False)
            except Exception:
                text = repr(value)
        else:
            text = repr(value)
        if text:
            print(text)

    @staticmethod
    def _report(kind, error, tb, own_only=False):
        """The traceback as the model should read it: its own cells and the libraries it called,
        without the frames of this file that happen to sit around them."""
        # Imported when first needed: on a recent Python it is the slowest import this file
        # makes, and a conversation whose cells all succeed never pays for it.
        import traceback

        report = traceback.TracebackException(kind, error, tb)
        seen = set()
        todo = [report]
        while todo:
            item = todo.pop()
            if item is None or id(item) in seen:
                continue
            seen.add(id(item))
            kept = [
                frame
                for frame in item.stack
                if (frame.filename.startswith("<cell ") if own_only else os.path.abspath(frame.filename) != HERE)
            ]
            item.stack = traceback.StackSummary.from_list(kept)
            todo.extend((item.__cause__, item.__context__))
        sys.stdout.flush()
        sys.stderr.write("".join(report.format()))
        sys.stderr.flush()


# ───────────────────────────── the conversation's state ─────────────────────────────


def _quote(value):
    return "'" + value.replace("'", "'\\''") + "'"


def _is_name(key):
    return bool(key) and not key[0].isdigit() and all(c.isalnum() or c == "_" for c in key) and key.isascii()


class Shared:
    """The directory and the exported variables a conversation carries — the same two files the
    `Bash` tool reads and writes (see `shell.rs`), so the conversation has one location and one
    environment whichever tool moved it."""

    def __init__(self, cwd_file, env_file, home):
        self.cwd_file = cwd_file
        self.env_file = env_file
        self.home = home
        # The file's text as last read or written, and what it said — see `_carried_by`.
        self._text = None
        self._carried = {}

    def _carried_by(self, base):
        """What the conversation has exported.

        The file is `sh`'s own `export -p` output whenever Bash wrote it last, and the only
        reader that is always right about that format is `sh`. So the *names* are taken from the
        file's text — the one part of it simple enough to read here — and `sh` is asked to source
        it and print what each of them ended up holding. One short process, and only when the
        text differs from what was read or written last: Bash rewrites the file on every command,
        usually with exactly what was already there.
        """
        try:
            with open(self.env_file, "rb") as f:
                text = f.read()
        except OSError:
            return {}
        if text == self._text:
            return dict(self._carried)
        names = []
        for line in text.decode("utf-8", "replace").splitlines():
            for lead in ("export ", "declare -x "):
                if line.startswith(lead):
                    name = line[len(lead):].split("=", 1)[0].strip()
                    if _is_name(name) and name not in UNCARRIED and name not in names:
                        names.append(name)
        carried = {}
        if names:
            # Each name is checked to be a plain identifier above, which is what makes it safe to
            # splice into the `eval`. A name the file mentions but did not set is skipped.
            script = (
                'if sh -n "$1" 2>/dev/null; then . "$1"; fi; shift; '
                "for n do eval 'v=${'\"$n\"'-}; s=${'\"$n\"'+x}'; "
                "[ -n \"$s\" ] && printf '%s=%s\\0' \"$n\" \"$v\"; done; true"
            )
            try:
                out = subprocess.run(
                    ["/bin/sh", "-c", script, "sh", self.env_file] + names,
                    env=base, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                    stderr=subprocess.DEVNULL, timeout=30,
                ).stdout
            except (OSError, subprocess.SubprocessError):
                return {}
            for item in out.split(b"\0"):
                key, sep, value = item.partition(b"=")
                if sep:
                    carried[os.fsdecode(key)] = os.fsdecode(value)
        self._text, self._carried = text, carried
        return dict(carried)

    def load(self, base):
        """Stand where the conversation stands: the launch environment as the caller has it now
        (so a secret rotated between turns arrives fresh), with what the conversation exported
        on top, in the directory its last command ended in."""
        carried = self._carried_by(base)
        os.environ.clear()
        os.environ.update(base)
        os.environ.update(carried)
        target = self.home
        try:
            with open(self.cwd_file) as f:
                recorded = f.read().strip()
            if recorded and os.path.isdir(recorded):
                target = recorded
        except OSError:
            pass
        try:
            os.chdir(target)
        except OSError:
            os.chdir(self.home)
        os.environ["PWD"] = os.getcwd()

    def save(self, base):
        """Record where the cell left things, for the next call of either tool."""
        try:
            where = os.getcwd()
            _replace(self.cwd_file, where + "\n")
        except OSError:
            pass
        lines = []
        carried = {}
        for key in sorted(os.environ):
            value = os.environ[key]
            if key in UNCARRIED or not _is_name(key) or base.get(key) == value:
                continue
            carried[key] = value
            lines.append("export %s=%s\n" % (key, _quote(value)))
        if carried == self._carried and self._text is not None:
            # Nothing was exported, changed or removed, so the file already says this — possibly
            # in Bash's own spelling. Rewriting it in ours would only make the next read, by
            # either tool, a parse of a file that says the same thing.
            return
        text = "".join(lines)
        try:
            _replace(self.env_file, text)
            self._text, self._carried = text.encode("utf-8"), carried
        except OSError:
            pass


def _replace(path, text):
    scratch = "%s.%d.next" % (path, os.getpid())
    with open(scratch, "w") as f:
        f.write(text)
    os.replace(scratch, path)


# ───────────────────────────── the kernel ─────────────────────────────


OMITTED = (
    "\n\n[… %d bytes omitted here. Print less of it — what you computed is still in your "
    "variables, so slice or summarize it rather than running this again.]\n\n"
)


def _printed(fd, limit):
    """What a call returns of what it printed: all of it when it fits, otherwise the start and
    the end with the middle named as missing. The end is kept because that is where a traceback
    is. Only the two ends are ever read — a cell is free to print gigabytes."""
    size = os.fstat(fd).st_size
    if size <= limit:
        return os.pread(fd, size, 0).decode("utf-8", "replace")
    head, tail = (limit * 3) // 5, (limit * 2) // 5
    return "%s%s%s" % (
        os.pread(fd, head, 0).decode("utf-8", "replace"),
        OMITTED % (size - head - tail),
        os.pread(fd, tail, size - tail).decode("utf-8", "replace"),
    )


class Kernel:
    def __init__(self, args):
        global _SHARED
        _SHARED = args.get("shared")
        self.state = args["state"]
        self.idle = float(args["idle"])
        # Echoed back in the state file, so a newer binary can tell an interpreter it started
        # from one an older build left running.
        self.protocol = int(args.get("protocol", 0))
        self.shared = Shared(args["shell-cwd"], args["shell-env"], args["home"])
        self.cells = Cells()
        self.dir = self._private_dir()
        self.socket = os.path.join(self.dir, "k.sock")

    @staticmethod
    def _private_dir():
        # A socket path has ~100 bytes to live in, and a session directory is often longer than
        # that on its own — so the socket lives in a private temp directory and the state file
        # says where. mkdtemp makes it 0700, which is the whole of the access control.
        made = tempfile.mkdtemp(prefix="adi-py-")
        if len(os.fsencode(made)) > 80:
            os.rmdir(made)
            made = tempfile.mkdtemp(prefix="adi-py-", dir="/tmp")
        return made

    def serve(self):
        _buffer_output()
        signal.signal(signal.SIGINT, self.cells.on_sigint)
        signal.signal(signal.SIGTERM, lambda *_: self.leave("terminated"))
        server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        server.bind(self.socket)
        os.chmod(self.socket, 0o600)
        server.listen(8)
        server.settimeout(2.0)
        self._announce({
            "pid": os.getpid(),
            "socket": self.socket,
            "protocol": self.protocol,
            "python": sys.version.split()[0],
        })

        last = time.monotonic()
        while True:
            try:
                conn, _ = server.accept()
            except socket.timeout:
                if not self._still_mine():
                    self.leave(None)
                if time.monotonic() - last > self.idle:
                    self.leave("idle")
                continue
            except InterruptedError:
                continue
            try:
                self.answer(conn)
            except (OSError, ValueError):
                pass
            finally:
                try:
                    conn.close()
                except OSError:
                    pass
            last = time.monotonic()

    def _announce(self, record):
        _replace(self.state, json.dumps(record) + "\n")

    def _still_mine(self):
        """Whether the state file still names this process. Gone means the session was deleted;
        another pid means a newer kernel took over. Either way this one is nobody's."""
        try:
            with open(self.state) as f:
                return json.load(f).get("pid") == os.getpid()
        except (OSError, ValueError):
            return False

    def leave(self, why):
        if why is not None and self._still_mine():
            # A tombstone, so the next call can say the interpreter it finds is a new one.
            try:
                self._announce({"ended": why})
            except OSError:
                pass
        try:
            os.unlink(self.socket)
        except OSError:
            pass
        try:
            os.rmdir(self.dir)
        except OSError:
            pass
        os._exit(0)

    def answer(self, conn):
        conn.settimeout(30)
        buf = b""
        while not buf.endswith(b"\n"):
            chunk = conn.recv(1 << 16)
            if not chunk:
                return
            buf += chunk
        request = json.loads(buf.decode("utf-8"))
        conn.settimeout(None)
        if request.get("op") == "ping":
            reply = {"pid": os.getpid()}
        else:
            reply = self.run(request, conn)
        conn.sendall(json.dumps(reply).encode("utf-8") + b"\n")

    def _watch(self, conn, wake):
        """Stop the cell if whoever asked for it hangs up. A cancelled turn takes its tool
        server with it, and a cell nobody will read the result of should not go on holding the
        interpreter — the next call would only queue behind it."""
        import select

        while True:
            try:
                ready, _, _ = select.select([conn, wake], [], [])
            except (OSError, ValueError):
                return
            if wake in ready:
                return
            try:
                gone = conn.recv(1, socket.MSG_PEEK) == b""
            except OSError:
                gone = True
            if gone:
                self.cells.interrupt("abandoned")
                return
            time.sleep(0.05)

    def run(self, request, conn):
        base = {str(k): str(v) for k, v in (request.get("env") or {}).items()}
        timeout = request.get("timeout_ms")
        limit = int(request.get("max_output") or 32000)
        began = time.monotonic()
        self.shared.load(base)
        # Put back in case a cell replaced it: the deadline is delivered through this handler.
        signal.signal(signal.SIGINT, self.cells.on_sigint)

        # Captured at the descriptor rather than by swapping `sys.stdout`, so what a child
        # process or a C library writes is caught along with every `print`. A file rather than a
        # pipe: nothing has to read it while the cell runs, and a process the cell leaves behind
        # holding it cannot keep this call open. Unlinked at once and made anew per cell, so such
        # a process goes on writing into a file nobody reads instead of into a later call.
        sys.stdout.flush()
        sys.stderr.flush()
        path = os.path.join(self.dir, "out")
        out = os.open(path, os.O_RDWR | os.O_CREAT | os.O_TRUNC | os.O_APPEND, 0o600)
        os.unlink(path)
        saved = (os.dup(1), os.dup(2))
        wake_r, wake_w = os.pipe()
        watcher = threading.Thread(target=self._watch, args=(conn, wake_r), daemon=True)
        os.dup2(out, 1)
        os.dup2(out, 2)
        try:
            watcher.start()
            # A code-only agent's cell is told where its tools are answered. For as long as it runs.
            _offer(
                self.cells.ns,
                request.get("tools"),
                (lambda: self.shared.save(base), lambda: self.shared.load(base)),
            )
            try:
                status = self.cells.run(request.get("code") or "", timeout / 1000.0 if timeout else None)
            finally:
                _offer(self.cells.ns, None)
        finally:
            os.dup2(saved[0], 1)
            os.dup2(saved[1], 2)
            os.close(saved[0])
            os.close(saved[1])
            os.write(wake_w, b"x")
            watcher.join(2.0)
            os.close(wake_r)
            os.close(wake_w)
        try:
            text = _printed(out, limit)
        finally:
            os.close(out)

        self.shared.save(base)
        return {
            "status": status,
            "stopped_by": self.cells.stopped_by,
            "output": text,
            "elapsed_ms": int((time.monotonic() - began) * 1000),
            "cell": self.cells.count,
        }


def _options(argv):
    found = {}
    key = None
    for word in argv:
        if key is None and word.startswith("--"):
            key = word[2:]
        elif key is not None:
            found[key] = word
            key = None
    return found


def main(argv):
    if len(argv) >= 2 and argv[0] == "run":
        global _SHARED
        given = _options(argv[2:])
        _SHARED = given.get("shared")
        with open(argv[1]) as f:
            code = f.read()
        cells = Cells()
        # A code-only agent's tools are functions only while a cell runs. Here they are names
        # that say so when called, which is more use than a `NameError`.
        if given.get("absent"):
            for name in given["absent"].split(","):
                cells.ns[name] = _tool(name)
            cells.ns["ToolError"] = ToolError
        signal.signal(signal.SIGINT, cells.on_sigint)
        return 0 if cells.run(code) == "ok" else 1
    if argv and argv[0] == "kernel":
        Kernel(_options(argv[1:])).serve()
        return 0
    sys.stderr.write("usage: pycode.py kernel --state F --shell-cwd F --shell-env F --shared PREFIX --home DIR --idle S\n"
                     "       pycode.py run SCRIPT [--shared PREFIX]\n")
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
