/*
 * The TypeScript a conversation keeps between `TsCode` calls.
 *
 * Two ways in, one namespace:
 *
 *   bun tscode.ts kernel --state F --capture F --shell-cwd F --shell-env F --shared PREFIX --home DIR --idle SECONDS --protocol N
 *       The conversation's interpreter. Listens on a Unix socket, runs one cell per connection in
 *       the process's own global scope, and exits when it has been idle too long or when its
 *       state file is gone (the session was deleted) or names another kernel (it was replaced).
 *
 *   bun tscode.ts run SCRIPT [--shared PREFIX] [--absent NAMES]
 *       One cell in a fresh process, for a background job: same helpers, same display of what
 *       it evaluates, nothing kept afterwards.
 *
 * Runs on Bun, which is the one JavaScript runtime the platform puts on every machine, and uses
 * nothing but what Bun ships: this file is embedded in the adi binary and dropped beside a
 * session, so there is nowhere to install a dependency from.
 *
 * The Rust half is `interpreter.rs`, shared with `pycode.py`; the wire is the same one — a JSON
 * object per line, one request and one reply per connection.
 *
 * For a code-only agent a request also says where the cell's *tools* are answered, and the
 * platform's other tools become functions for as long as the cell runs — see "the platform's
 * tools, as functions" below.
 *
 * # Why a cell is rewritten before it runs
 *
 * JavaScript has no scope that both outlives a piece of code and can be declared into twice. A
 * `const` at the top of a module dies with the module; one at the top of a script cannot be
 * declared again, so running the same cell twice would be a syntax error. So a cell's top-level
 * declarations are turned into plain assignments, which in non-strict code create properties of
 * the global object — the one scope every later cell can see and overwrite. `import` statements
 * become awaited dynamic imports for the same reason, and each top-level expression is handed to
 * a function that prints it.
 *
 * The rewriting is done on what Bun's own transpiler prints, never on what the model wrote. Its
 * printer puts every top-level statement at the start of a line and ends it with a semicolon, so
 * finding them needs a tokenizer rather than a parser. And it is checked: the rewritten cell is
 * compiled before it is run, and one that does not compile falls back to a plainer rewriting
 * instead of failing code that was valid.
 */

import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import fs from "node:fs";
import { createRequire } from "node:module";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";

const HERE = import.meta.path;

/** Never carried between calls: the shell maintains these itself. The same four `shell.rs` filters. */
const UNCARRIED = new Set(["PWD", "OLDPWD", "SHLVL", "_"]);

/** How much of one stream `sh()` keeps — far above what a call may print; output here is data. */
const MAX_CAPTURE = 64 << 20;

/**
 * How long `sh()` gives a command's pipes once the command itself is gone. A pipe stays open
 * while any writer holds it, and `(sleep 60 &)` exits at once having handed its stdout on.
 */
const DRAIN_GRACE_MS = 200;

/**
 * How long a cell is given to end once it has been told to. Nothing can interrupt running
 * JavaScript from inside its own process: what can be done is to kill the commands it is waiting
 * on, which ends most cells that are slow. One that is still going after this is not going to stop.
 */
const STOP_GRACE_MS = 3000;

// ───────────────────────────── helpers the model calls ─────────────────────────────

/** What a command printed, and how it ended. */
class Sh {
  constructor(
    readonly stdout: string,
    readonly stderr: string,
    readonly code: number,
    readonly cmd: string,
    readonly truncated = false,
    readonly echoed = false,
  ) {}
  get ok(): boolean {
    return this.code === 0;
  }
  /** The same number under the name Bun's and Node's results give it. */
  get exitCode(): number {
    return this.code;
  }
  /** This result, as one that shows nothing when it stands on a line of its own. */
  quieted(): Sh {
    return new Sh(this.stdout, this.stderr, this.code, this.cmd, this.truncated, true);
  }
  /** stdout with trailing newlines dropped — what `$(...)` would have given. */
  get text(): string {
    return this.stdout.replace(/\n+$/, "");
  }
  get lines(): string[] {
    return this.text === "" ? [] : this.text.split("\n");
  }
  json(): unknown {
    return JSON.parse(this.stdout);
  }
  toString(): string {
    return this.text;
  }
  toJSON(): string {
    return this.text;
  }
  [Symbol.toPrimitive](): string {
    return this.text;
  }
  /** Shown when a command is a top-level expression, and read the way Bash's result is. */
  shown(): string {
    if (this.echoed) return this.code !== 0 ? `(exit ${this.code})` : "";
    let text = this.stdout;
    if (this.stderr.trim()) {
      if (text && !text.endsWith("\n")) text += "\n";
      text += this.stderr;
    }
    text = text.replace(/\n+$/, "");
    if (this.truncated) text += `\n[output was cut at ${MAX_CAPTURE} bytes per stream]`;
    if (this.code !== 0) return text ? `(exit ${this.code})\n${text}` : `(exit ${this.code})`;
    // Nothing, for a command that said nothing: in a cell of several, a word for each silent one
    // is a line of noise apiece. A cell that printed nothing at all is said to have by whoever
    // reports it.
    return text.trim() ? text : "";
  }
}

/** A command run with `sh()` exited non-zero, outlived its timeout, or could not be started. */
class ShError extends Error {
  readonly code: number;
  readonly stdout: string;
  readonly stderr: string;
  /** `killed`: the command did not end by itself — it was stopped at a deadline. */
  constructor(
    message: string,
    readonly result: Sh,
    readonly killed = false,
  ) {
    super(message);
    this.name = "ShError";
    this.code = result.code;
    this.stdout = result.stdout;
    this.stderr = result.stderr;
  }
}

type ShOptions = {
  /** Raise on a non-zero exit. On by default. */
  check?: boolean;
  /** Kill the command after this many milliseconds. */
  timeout?: number;
  input?: string | Uint8Array;
  cwd?: string;
  env?: Record<string, string | undefined>;
  /** Also copy the output through as it is produced. */
  echo?: boolean;
};

/** The end of what a failed command printed, for the error that reports it. */
function tail(result: Sh, limit = 4000): string {
  let text = (result.stderr.trim() ? result.stderr : result.stdout).replace(/\n+$/, "");
  if (!text.trim()) return "";
  if (text.length > limit) text = "[…]" + text.slice(-limit);
  return "\n" + text;
}

/** Commands a cell is waiting on right now — what a deadline can actually stop. */
const running = new Set<{ stop: (why: string) => void }>();

/**
 * Why the cell now running is being stopped, once it is. Stopping the commands it is waiting on
 * is not enough by itself: a cell told to stop between two commands would simply start the
 * second. So while this is set no command starts at all, and a cell that is a list of commands
 * unwinds at the next one whenever the word arrives.
 */
let stopping: string | undefined;

/** End a command and everything it started. It leads a session of its own, so the group is the tree. */
function killTree(child: ChildProcess) {
  const signal = (name: NodeJS.Signals) => {
    try {
      if (child.pid) process.kill(-child.pid, name);
    } catch {
      try {
        child.kill(name);
      } catch {}
    }
  };
  signal("SIGTERM");
  setTimeout(() => {
    if (child.exitCode === null && child.signalCode === null) signal("SIGKILL");
  }, 1000).unref();
}

/**
 * Run a command and resolve to what it printed (see `Sh`).
 *
 * A string runs through `/bin/sh`; an array runs as written, with no shell and so no quoting to
 * get wrong. A non-zero exit rejects with `ShError` carrying the output — pass `check: false`
 * where failing is an answer rather than a fault (`grep`, `diff`, `test`).
 */
function sh(
  cmd: string | readonly unknown[],
  options: ShOptions = {},
  label?: string,
): Promise<Sh> {
  const check = options.check ?? true;
  const echo = options.echo ?? false;
  const argv = typeof cmd === "string" ? ["/bin/sh", "-c", cmd] : cmd.map((part) => String(part));
  // `label`: what to call the command when what runs is not what was written (see `$`).
  const shown = label ?? (typeof cmd === "string" ? cmd : argv.join(" "));
  const where = options.cwd !== undefined ? path.resolve(options.cwd) : process.cwd();
  // A program that reads $PWD rather than asking the kernel would otherwise be told where the
  // run began, not where this command starts.
  const env = { ...(options.env ?? process.env), PWD: where };
  // Taken now, while the cell that called is still on the stack. The error itself is made later,
  // in an event handler, where the only frames left are the runtime's own.
  const calledFrom = String(new Error().stack ?? "")
    .split("\n")
    .filter((line) => /^\s+at /.test(line))
    .slice(1);

  return new Promise<Sh>((resolve, reject0) => {
    const reject = (error: ShError) => {
      error.stack = [`${error.name}: ${error.message}`, ...calledFrom].join("\n");
      reject0(error);
    };
    if (stopping) {
      reject(
        new ShError(`command was not started (${stopping}): ${shown}`, new Sh("", "", -1, shown)),
      );
      return;
    }
    const out: Buffer[] = [];
    const err: Buffer[] = [];
    let sizes = [0, 0];
    let truncated = false;
    let done = false;
    let stopped: string | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;

    const result = (code: number) =>
      new Sh(
        Buffer.concat(out).toString("utf8"),
        Buffer.concat(err).toString("utf8"),
        code,
        shown,
        truncated,
        echo,
      );

    const entry = {
      stop(why: string) {
        stopped = why;
        if (child) killTree(child);
      },
    };

    const finish = (code: number) => {
      if (done) return;
      done = true;
      running.delete(entry);
      if (timer) clearTimeout(timer);
      const got = result(code);
      if (stopped === "timeout") {
        reject(
          new ShError(
            `command still running after ${options.timeout}ms and was killed: ${shown}${tail(got)}`,
            got,
            true,
          ),
        );
      } else if (stopped) {
        // The call's own deadline. What the command had printed is worth showing — it is
        // usually the only clue to why it was slow.
        if (!echo) {
          if (got.stdout) process.stdout.write(got.stdout);
          if (got.stderr) process.stderr.write(got.stderr);
        }
        reject(new ShError(`command was stopped (${stopped}): ${shown}`, got, true));
      } else if (check && code !== 0) {
        reject(new ShError(`command exited ${code}: ${shown}${tail(got)}`, got));
      } else {
        resolve(got);
      }
    };

    // A program that is not there. Through the shell that is status 127 and a line on stderr;
    // an array is given the same ending, so `check: false` means one thing either way.
    const unstartable = (error: unknown) => {
      if (done) return;
      done = true;
      running.delete(entry);
      const what = fs.existsSync(where) ? argv[0] : where;
      const reason =
        (error as NodeJS.ErrnoException)?.code === "ENOENT"
          ? "No such file or directory"
          : String((error as Error)?.message ?? error);
      const got = new Sh("", `${what}: ${reason}\n`, 127, shown);
      if (check) reject(new ShError(`couldn't run ${shown}${tail(got)}`, got));
      else resolve(got);
    };

    let child: ChildProcess | undefined;
    try {
      child = spawn(argv[0], argv.slice(1), {
        cwd: where,
        env: env as NodeJS.ProcessEnv,
        // A session of its own, so the whole tree can be signalled as one group.
        detached: true,
        stdio: [options.input !== undefined ? "pipe" : "ignore", "pipe", "pipe"],
      });
    } catch (error) {
      unstartable(error);
      return;
    }
    running.add(entry);
    child.on("error", unstartable);

    const keep = (into: Buffer[], which: 0 | 1) => (chunk: Buffer) => {
      if (echo) (which === 0 ? process.stdout : process.stderr).write(chunk);
      const room = MAX_CAPTURE - sizes[which];
      if (room <= 0) {
        truncated = true;
        return;
      }
      if (chunk.length > room) truncated = true;
      into.push(chunk.length > room ? chunk.subarray(0, room) : chunk);
      sizes[which] += chunk.length;
    };
    child.stdout?.on("data", keep(out, 0));
    child.stderr?.on("data", keep(err, 1));

    if (options.input !== undefined && child.stdin) {
      child.stdin.on("error", () => {});
      child.stdin.end(options.input);
    }
    if (options.timeout !== undefined) {
      timer = setTimeout(() => entry.stop("timeout"), options.timeout);
    }

    const status = (code: number | null, signal: NodeJS.Signals | null) =>
      code ?? (signal ? -(os.constants.signals[signal] ?? 1) : -1);
    // `close` is every pipe at end-of-file; `exit` is the command gone. Wait for the first, but
    // not for long once the second has happened.
    child.on("close", (code, signal) => finish(status(code, signal)));
    child.on("exit", (code, signal) => {
      setTimeout(() => finish(status(code, signal)), DRAIN_GRACE_MS);
    });
  });
}

// ───────────────────────────── $`…`: the conversation's shell ─────────────────────────────

/*
 * `sh` runs a command. `$` is the shell — the one the `Bash` tool is, written as a template:
 *
 *     await $`cd crates/adi-agents && cargo test ${filter}`;
 *
 * Two things make it that, rather than a second spelling of `sh`.
 *
 * What a command leaves behind stays. The directory it ends in and what it exported become this
 * process's own — and this process's own are what the conversation's shell is between calls (see
 * `Shared`). So the next command, the code around it and the `Bash` tool all start from there.
 *
 * What is interpolated is data. A value never becomes part of the script's text: it is assigned
 * to a shell variable ahead of the command, and the command refers to it. There is then nothing a
 * value can contain that the shell will run, however the reference to it is quoted. The quoting
 * decides only how faithfully the value arrives, and it is read off the template (`Quoting`): a
 * value standing bare is one word, inside the template's own "…" or '…' it is part of that
 * string, and in a heredoc it is part of the body.
 */

/** Where in a command a value is about to be put. */
type Place = "bare" | "double" | "single" | "ansi" | "heredoc" | "sealed";

type Heredoc = { word: string; sealed: boolean; tabs: boolean };

/**
 * Follows a command's quoting as its text goes by, so that the place of each value is known.
 *
 * It reads as much of the shell's grammar as keeping its place takes — quotes, escapes,
 * comments, `$( … )`, heredocs — and no more. Where it loses its place anyway, nothing is at
 * stake but fidelity: a value is only ever a variable reference, so quoted wrongly it arrives
 * split or wrapped in quotes it should not have, and is never run.
 */
class Quoting {
  private mode: "bare" | "double" | "single" | "ansi" = "bare";
  /** The quoting to return to at each `)` still open. */
  private outer: Array<"bare" | "double"> = [];
  /** Heredocs named on the line being read; their bodies begin where it ends. */
  private awaited: Heredoc[] = [];
  /** The heredoc whose body is being read. */
  private here: Heredoc | undefined;
  /** That body's current line, or `undefined` once a value is in it — it is not the last line then. */
  private line: string | undefined = "";
  /** Inside a `#` comment that the text read so far has not ended. */
  private comment = false;
  /** Whether a `#` here would begin a comment: at the start of a word. */
  private fresh = true;

  /** Where a value put in at this point stands. */
  place(): Place {
    if (this.here) {
      this.line = undefined;
      return this.here.sealed ? "sealed" : "heredoc";
    }
    this.fresh = false;
    return this.comment ? "bare" : this.mode;
  }

  read(text: string) {
    for (let i = 0; i < text.length; i++) {
      const c = text[i];
      if (this.here) {
        if (c !== "\n") {
          if (this.line !== undefined) this.line += c;
          continue;
        }
        const line = this.line;
        this.line = "";
        if (
          line !== undefined &&
          (this.here.tabs ? line.replace(/^\t+/, "") : line) === this.here.word
        ) {
          this.here = this.awaited.shift();
          this.fresh = true;
        }
      } else if (this.comment) {
        const end = text.indexOf("\n", i);
        if (end < 0) return;
        this.comment = false;
        i = end - 1;
      } else if (this.mode === "single") {
        if (c === "'") this.mode = "bare";
      } else if (this.mode === "ansi") {
        if (c === "\\") i += 1;
        else if (c === "'") this.mode = "bare";
      } else if (this.mode === "double") {
        if (c === "\\") i += 1;
        else if (c === '"') this.mode = "bare";
        else if (c === "$" && text[i + 1] === "(" && text[i + 2] !== "(") {
          this.outer.push("double");
          this.mode = "bare";
          this.fresh = true;
          i += 1;
        }
      } else {
        i = this.bare(text, i);
      }
    }
  }

  /** One character of unquoted text, at `i`. Returns the index of the last character it took. */
  private bare(text: string, i: number): number {
    const fresh = this.fresh;
    this.fresh = false;
    switch (text[i]) {
      case "\\":
        return i + 1;
      case "'":
        this.mode = "single";
        return i;
      case '"':
        this.mode = "double";
        return i;
      case "#":
        if (fresh) {
          this.comment = true;
          return i;
        }
        return i;
      case "$":
        if (text[i + 1] === "'") {
          this.mode = "ansi";
          return i + 1;
        }
        if (text[i + 1] === "(" && text[i + 2] === "(") {
          // Arithmetic, where `<<` is a shift and a quote is not one. Stepped over whole.
          const end = text.indexOf("))", i + 3);
          if (end >= 0) return end + 1;
        }
        if (text[i + 1] === "(") {
          this.outer.push("bare");
          this.fresh = true;
          return i + 1;
        }
        return i;
      case "(":
        this.outer.push("bare");
        this.fresh = true;
        return i;
      case ")":
        this.mode = this.outer.pop() ?? "bare";
        return i;
      case "\n":
        this.fresh = true;
        if (this.awaited.length) {
          this.here = this.awaited.shift();
          this.line = "";
        }
        return i;
      case " ":
      case "\t":
      case ";":
      case "&":
      case "|":
        this.fresh = true;
        return i;
      case "<":
        if (text[i + 1] !== "<") return i;
        // `<<<` is a here-string: a word follows it, like any other.
        if (text[i + 2] === "<") return i + 2;
        return this.heredoc(text, i + 2);
      default:
        return i;
    }
  }

  /** A `<<` has just gone by and `i` is what follows it: note the heredoc it opens. */
  private heredoc(text: string, i: number): number {
    let tabs = false;
    if (text[i] === "-") {
      tabs = true;
      i += 1;
    }
    while (text[i] === " " || text[i] === "\t") i += 1;
    let word = "";
    // A delimiter with any quoting in it seals the body: nothing in it is expanded.
    let sealed = false;
    for (; i < text.length; i++) {
      const c = text[i];
      if (c === "'" || c === '"') {
        const end = text.indexOf(c, i + 1);
        if (end < 0) break;
        word += text.slice(i + 1, end);
        sealed = true;
        i = end;
      } else if (c === "\\") {
        word += text[i + 1] ?? "";
        sealed = true;
        i += 1;
      } else if (/[\s;&|<>()]/.test(c)) {
        break;
      } else {
        word += c;
      }
    }
    if (word) this.awaited.push({ word, sealed, tabs });
    return i - 1;
  }
}

/** A piece of command the caller built and vouches for: put in as written, not as a value. */
class RawText {
  constructor(readonly text: string) {}
}

/** The words a value is: one for each element of an array, otherwise one. */
function wordsOf(value: unknown): string[] {
  if (Array.isArray(value)) return value.flatMap(wordsOf);
  if (value === undefined || value === null) {
    throw new TypeError(
      `$: a value put into the command is ${value}. A command with a hole in it is not run — check the variable, or pass "" if empty is what you mean.`,
    );
  }
  if (value instanceof Sh) return [value.text];
  switch (typeof value) {
    case "string":
      return [value];
    case "number":
    case "bigint":
    case "boolean":
      return [String(value)];
    case "object": {
      // Something that says what it is — a URL, a Date, a Buffer — and not a bag of fields.
      const text = String(value);
      if (text !== "[object Object]") return [text];
    }
  }
  throw new TypeError(
    `$: ${typeof value === "object" ? "an object" : `a ${typeof value}`} cannot be put into a command — pass a string (JSON.stringify it, if JSON is what the command wants).`,
  );
}

/** A template as the script that runs it, and as the command to call it when reporting on it. */
type Built = { script: string; label: string };

function build(pieces: readonly string[], values: readonly unknown[]): Built {
  const quoting = new Quoting();
  const assigned: string[] = [];
  let body = "";
  let label = "";
  const keep = (value: string) => {
    const name = `__adi_${assigned.length + 1}`;
    assigned.push(`${name}=${quote(value)}`);
    return `\${${name}}`;
  };

  pieces.forEach((piece, at) => {
    quoting.read(piece);
    body += piece;
    label += piece;
    if (at >= values.length) return;
    const value = values[at];
    if (value instanceof RawText) {
      // Command, not data — so it is read as command, and moves the quoting like any other.
      quoting.read(value.text);
      body += value.text;
      label += value.text;
      return;
    }
    const words = wordsOf(value);
    const place = quoting.place();
    if (place === "sealed") {
      throw new TypeError(
        "$: a value cannot be put into a heredoc whose delimiter is quoted (<<'EOF'): nothing inside one is expanded. Leave the delimiter bare, or hand the text over with .input(text).",
      );
    }
    if (place === "bare") {
      body += words.map((word) => `"${keep(word)}"`).join(" ");
      label += words.map(quote).join(" ");
      return;
    }
    const whole = words.join(" ");
    const ref = keep(whole);
    // Inside single quotes nothing expands, so the string is closed around the reference and
    // opened again behind it.
    body += place === "single" ? `'"${ref}"'` : place === "ansi" ? `'"${ref}"$'` : ref;
    label += whole;
  });
  return { script: assigned.length ? `${assigned.join("\n")}\n${body}` : body, label };
}

/** The names an `export -p` listing exports, in its order, leaving out the shell's own four. */
function exportedNames(text: string): string[] {
  const names: string[] = [];
  for (const line of text.split("\n")) {
    const name = exportedName(line);
    if (name && !names.includes(name)) names.push(name);
  }
  return names;
}

/** The variable one line of such a listing exports: `""` for one that is not carried, `undefined` for a line that exports nothing. */
function exportedName(line: string): string | undefined {
  for (const lead of ["export ", "declare -x "]) {
    if (!line.startsWith(lead)) continue;
    const name = line.slice(lead.length).split("=", 1)[0].trim();
    return isName(name) && !UNCARRIED.has(name) ? name : "";
  }
  return undefined;
}

/**
 * What each of `names` holds once `sh` has read the listing in `file`. Asked of `sh` because
 * the listing is `sh`'s own output, in whichever spelling that `sh` uses, and it is the only
 * reader always right about it. `undefined` when it could not be asked.
 */
function exportedValues(
  file: string,
  names: readonly string[],
  env: Record<string, string | undefined>,
): Record<string, string> | undefined {
  const found: Record<string, string> = {};
  if (!names.length) return found;
  // Every name was checked to be a plain identifier, which is what makes the `eval` safe.
  const script =
    'if sh -n "$1" 2>/dev/null; then . "$1"; fi; shift; ' +
    "for n do eval 'v=${'\"$n\"'-}; s=${'\"$n\"'+x}'; " +
    '[ -n "$s" ] && printf \'%s=%s\\0\' "$n" "$v"; done; true';
  const ran = spawnSync("/bin/sh", ["-c", script, "sh", file, ...names], {
    env: env as NodeJS.ProcessEnv,
    stdio: ["ignore", "pipe", "ignore"],
    timeout: 30_000,
  });
  if (ran.error || !ran.stdout) return undefined;
  for (const item of ran.stdout.toString("utf8").split("\0")) {
    const eq = item.indexOf("=");
    if (eq > 0) found[item.slice(0, eq)] = item.slice(eq + 1);
  }
  return found;
}

/** Where `$` commands report how they ended: a private directory, made when first needed. */
let stateDir: string | undefined;
let stateCalls = 0;

type StateFiles = { cwd: string; before: string; after: string };

/**
 * Make what a command left behind this process's own: the directory it ended in, and what it
 * exported or unset. A call that was pointed somewhere on purpose (`cwd`, `env`) moves nothing
 * of that kind — where it ran was not where the conversation stood.
 */
function adopt(files: StateFiles, options: ShOptions) {
  if (options.cwd === undefined) {
    try {
      const where = fs.readFileSync(files.cwd, "utf8").trim();
      if (where && where !== process.cwd()) {
        process.chdir(where);
        process.env.PWD = process.cwd();
      }
    } catch {}
  }
  if (options.env !== undefined) return;
  let before: string;
  let after: string;
  try {
    before = fs.readFileSync(files.before, "utf8");
    after = fs.readFileSync(files.after, "utf8");
  } catch {
    return;
  }
  if (after === before) return;
  const had = new Set(before.split("\n"));
  let changed: string[] = [];
  let unsure = false;
  for (const line of after.split("\n")) {
    if (!line || had.has(line)) continue;
    const name = exportedName(line);
    // A changed line that exports nothing is the inside of a value that spans lines: which
    // variable's is not worth working out when all of them can be asked for.
    if (name === undefined) unsure = true;
    else if (name && !changed.includes(name)) changed.push(name);
  }
  const now = exportedNames(after);
  if (unsure) changed = now;
  const values = exportedValues(files.after, changed, process.env);
  if (values) Object.assign(process.env, values);
  for (const name of exportedNames(before)) {
    if (!now.includes(name)) delete process.env[name];
  }
}

/** Run a built command through `/bin/sh`, and stand afterwards where it left the shell. */
function runInShell(built: Built, options: ShOptions): Promise<Sh> {
  stateDir ??= fs.mkdtempSync(path.join(os.tmpdir(), "adi-sh-"));
  const stem = path.join(stateDir, String(++stateCalls));
  const files: StateFiles = {
    cwd: `${stem}.cwd`,
    before: `${stem}.before`,
    after: `${stem}.after`,
  };
  // Recorded by an `EXIT` trap, so that it still happens when the command itself exits, and
  // the command's own status is what the shell then ends with.
  const script =
    `__adi_cwd=${quote(files.cwd)} __adi_before=${quote(files.before)} __adi_after=${quote(files.after)}\n` +
    'export -p > "$__adi_before" 2>/dev/null\n' +
    `trap '__adi_status=$?; pwd -P > "$__adi_cwd" 2>/dev/null; export -p > "$__adi_after" 2>/dev/null; exit $__adi_status' EXIT\n` +
    `${built.script}\n`;
  const settle = (killed: boolean) => {
    // A command stopped at a deadline did not finish, and half of a shell's state is worse
    // than none: nothing is taken from it.
    if (!killed) adopt(files, options);
    for (const file of Object.values(files)) {
      try {
        fs.unlinkSync(file);
      } catch {}
    }
  };
  return sh(script, options, built.label).then(
    (result) => {
      settle(false);
      return result;
    },
    (error) => {
      settle(error instanceof ShError && error.killed);
      throw error;
    },
  );
}

/**
 * A `$` command: awaited for its result, and adjusted before that — `.nothrow()`, `.quiet()`,
 * `.timeout(ms)`, `.cwd(dir)`, `.env({…})`, `.input(text)` — or awaited straight for a value
 * with `.text()`, `.lines()`, `.json()`.
 *
 * It starts on its own a moment after the line that made it, so that a forgotten `await` is not
 * also a command that never ran; what a call is adjusted with has to be said on that line.
 */
class ShCall implements PromiseLike<Sh> {
  private options: ShOptions = {};
  private silent = false;
  private started: Promise<Sh> | undefined;

  /** `built`: the command — or why the template could not be made into one. */
  constructor(private readonly built: Built | Error) {
    queueMicrotask(() => void this.start());
  }

  private start(): Promise<Sh> {
    const { built } = this;
    // A template that is no command fails where its result is awaited, like a command that
    // fails — not on the line that wrote it, where a `.catch()` further along would never see it.
    this.started ??=
      built instanceof Error
        ? Promise.reject(built)
        : runInShell(built, this.options).then((result) =>
            this.silent ? result.quieted() : result,
          );
    return this.started;
  }

  private adjust(change: () => void): this {
    if (this.started)
      throw new Error(
        "this command is already running — adjust it on the line that starts it: await $`…`.nothrow()",
      );
    change();
    return this;
  }

  /** Resolve with the result whatever the exit status, instead of rejecting on a non-zero one. */
  nothrow(): this {
    return this.adjust(() => {
      this.options.check = false;
    });
  }
  /** Show nothing of the output when the call stands on a line of its own. */
  quiet(): this {
    return this.adjust(() => {
      this.silent = true;
    });
  }
  /** Copy the output through as it is produced. */
  echo(): this {
    return this.adjust(() => {
      this.options.echo = true;
    });
  }
  /** Kill the command after this many milliseconds. */
  timeout(ms: number): this {
    return this.adjust(() => {
      this.options.timeout = ms;
    });
  }
  /** Run it in `dir` instead of where the conversation stands — and leave the conversation there. */
  cwd(dir: string): this {
    return this.adjust(() => {
      this.options.cwd = dir;
    });
  }
  /** Run it with exactly this environment. What it exports is then not kept. */
  env(vars: Record<string, string | undefined>): this {
    return this.adjust(() => {
      this.options.env = vars;
    });
  }
  /** Give it this on its standard input. */
  input(data: string | Uint8Array): this {
    return this.adjust(() => {
      this.options.input = data;
    });
  }

  /** stdout with the trailing newline dropped. */
  text(): Promise<string> {
    return this.start().then((result) => result.text);
  }
  lines(): Promise<string[]> {
    return this.start().then((result) => result.lines);
  }
  json(): Promise<unknown> {
    return this.start().then((result) => result.json());
  }

  then<A = Sh, B = never>(
    done?: ((result: Sh) => A | PromiseLike<A>) | null,
    failed?: ((reason: unknown) => B | PromiseLike<B>) | null,
  ): Promise<A | B> {
    return this.start().then(done, failed);
  }
  catch<B = never>(failed?: ((reason: unknown) => B | PromiseLike<B>) | null): Promise<Sh | B> {
    return this.start().catch(failed);
  }
  finally(done?: (() => void) | null): Promise<Sh> {
    return this.start().finally(done);
  }

  /** What a call that was not awaited looks like when a cell ends with it. */
  [Symbol.for("nodejs.util.inspect.custom")](): string {
    const label = this.built instanceof Error ? "…" : this.built.label;
    return `[\$\`${label}\` — not awaited: write \`await $\`…\`\` to have its result]`;
  }
}

/**
 * The shell, as a template tag: `` await $`git status` ``. See the top of this section for what
 * it does with the command and with what is interpolated into it.
 */
function dollar(pieces: TemplateStringsArray, ...values: unknown[]): ShCall {
  if (!Array.isArray(pieces) || !Array.isArray(pieces.raw)) {
    throw new TypeError(
      "$ is a template tag — write $`command`, with backticks. For a command held in a string there is sh(command).",
    );
  }
  try {
    // The raw text, so that what is typed between the backticks is what the shell reads: a
    // `\n` for `printf`, a `\b` for `grep`, a `\$` that is not to be expanded.
    const built = build(pieces.raw, values);
    if (!built.script.trim()) throw new TypeError("$: there is no command between the backticks.");
    return new ShCall(built);
  } catch (error) {
    return new ShCall(error instanceof Error ? error : new Error(String(error)));
  }
}

/** Text to put into a command as written rather than as a value: `` $`ls ${$.raw(flags)}` ``. */
dollar.raw = (text: unknown): RawText => new RawText(String(text));

/**
 * Start a command as a job that outlives the turn, and be woken when it ends — the `Bash`
 * tool's `background`, which is where it goes. So it is there where that tool is a function: in
 * a cell of a code-only agent.
 */
dollar.background = async (pieces: TemplateStringsArray, ...values: unknown[]): Promise<string> => {
  const built = build(pieces.raw, values);
  if (!door?.names.includes("Bash")) {
    throw new Error(
      "$.background starts a job through the Bash tool, which code can call only in a cell of a code-only agent. Here, start it with the Bash tool's own `background`.",
    );
  }
  return (toolFunction("Bash") as (input: unknown) => Promise<string>)({
    command: built.script,
    background: true,
  });
};

/**
 * Where shared values are kept: a path prefix, to which a name and `.json` are added. Set by
 * whoever starts this file — it is the session's own namespace, so the values are swept with the
 * conversation, and the Python interpreter is handed the same prefix.
 */
let sharedPrefix: string | undefined;

function sharedFile(name: unknown): string {
  if (typeof name !== "string" || !/^[A-Za-z0-9_-]+$/.test(name)) {
    throw new TypeError(`a shared name is letters, digits, _ and -, not ${Bun.inspect(name)}`);
  }
  if (sharedPrefix === undefined) throw new Error("there is nowhere to share a value from here");
  return `${sharedPrefix}${name}.json`;
}

/**
 * Keep `value` under `name`, outside this interpreter.
 *
 * What is kept is JSON, so it is found again — with `shared(name)` — wherever JSON can go and a
 * variable cannot: by a background cell, by this conversation after its interpreter is restarted
 * or lost, and by its Python interpreter. For a result, a list of names, a configuration;
 * something large belongs in a file of its own.
 */
function share(name: string, value: unknown): void {
  const file = sharedFile(name);
  let text: string | undefined;
  try {
    text = JSON.stringify(value);
  } catch (error) {
    throw new TypeError(
      `share(${JSON.stringify(name)}): only what JSON can hold can be shared (${(error as Error).message}). Convert it first, or write it to a file.`,
    );
  }
  if (text === undefined) {
    throw new TypeError(
      `share(${JSON.stringify(name)}): only what JSON can hold can be shared, and ${typeof value === "function" ? "a function" : String(value)} is not. Convert it first, or write it to a file.`,
    );
  }
  replaceFile(file, text);
}

/** The value kept under `name` by `share` — or, with no name, the names there are. */
function shared(name?: string): unknown {
  if (name === undefined) {
    if (sharedPrefix === undefined) return [];
    const dir = path.dirname(sharedPrefix);
    const stem = path.basename(sharedPrefix);
    let entries: string[] = [];
    try {
      entries = fs.readdirSync(dir);
    } catch {}
    return entries
      .filter((entry) => entry.startsWith(stem) && entry.endsWith(".json"))
      .map((entry) => entry.slice(stem.length, -".json".length))
      .filter((entry) => /^[A-Za-z0-9_-]+$/.test(entry))
      .sort();
  }
  const file = sharedFile(name);
  let text: string;
  try {
    text = fs.readFileSync(file, "utf8");
  } catch {
    const names = shared() as string[];
    throw new ReferenceError(
      `nothing is shared as ${JSON.stringify(name)}. What is: ${names.join(", ") || "nothing yet"}`,
    );
  }
  return JSON.parse(text);
}

// ───────────────────────────── the platform's tools, as functions ─────────────────────────────

/*
 * A code-only agent has one tool — this interpreter — and reaches every other from inside a
 * cell: `await Read({ path })`, `await mcp.n8n.call("list", {})`. The tools themselves are not
 * here. They are code in whoever is serving the turn (`code_only.rs`), which listens on a socket
 * for as long as a cell runs and says where in the cell's request. A call is one connection: a
 * line of JSON out, a line of JSON back.
 */

/** A tool called from code failed, or could not be reached. The message is what the tool said. */
class ToolError extends Error {
  constructor(
    message: string,
    readonly tool: string,
  ) {
    super(message);
    this.name = "ToolError";
  }
}

/** What a cell is told about its tools: where they are answered, what they are called, and what each says about itself. */
type Door = { socket: string; names: string[]; servers: string[]; docs?: Record<string, string> };

/** Where the cell now running reaches its tools — nowhere, between cells and for an agent that is not code-only. */
let door: Door | undefined;

/**
 * Around a tool that moves the conversation's shell. `Bash` starts where the shell was last
 * recorded and records where it ends, so the cell's own position is written down before the call
 * and read back after it: a `cd` in either is then a `cd` in both, mid-cell as between cells.
 */
let shellSync: { before: () => void; after: () => void } | undefined;

type Answer = { text?: string; value?: unknown };

/**
 * What the tools called by the cell now running answered with. A tool's answer is a plain string,
 * which cannot be told from any other once it has been returned — and a call standing on a line
 * of its own should show what the tool said, as a bare `sh` shows what a command printed, rather
 * than a quoted and escaped rendering of it. So the texts are remembered, and a top-level
 * expression that comes to one of them is shown as written (see `Cells.show`).
 */
const toolTexts = new Set<string>();

/** Send one call through the door and resolve to its answer. */
function ask(tool: string, message: Record<string, unknown>): Promise<Answer> {
  // Taken now, for the reason `sh` takes it: the error is made in an event handler.
  const calledFrom = String(new Error().stack ?? "")
    .split("\n")
    .filter((line) => /^\s+at /.test(line))
    .slice(2);
  return new Promise<Answer>((resolve, reject0) => {
    const reject = (text: string) => {
      const error = new ToolError(text, tool);
      error.stack = [`${error.name}: ${error.message}`, ...calledFrom].join("\n");
      reject0(error);
    };
    if (!door) {
      reject(
        `${tool} can only be called while a cell of yours is running: not from a background script, and not from a callback that outlived its cell`,
      );
      return;
    }
    if (stopping) {
      reject(`${tool} was not called (${stopping})`);
      return;
    }
    let payload: string;
    try {
      payload = JSON.stringify(message);
    } catch (error) {
      reject(
        `${tool}: its arguments are not something JSON can hold (${(error as Error).message})`,
      );
      return;
    }

    let buffer = "";
    let done = false;
    const conn = net.connect(door.socket);
    const finish = (act: () => void) => {
      if (done) return;
      done = true;
      running.delete(entry);
      act();
    };
    // Among the things a deadline stops, beside the commands: a cell waiting on a tool is
    // otherwise a cell that cannot be told to end.
    const entry = {
      stop(why: string) {
        finish(() => reject(`${tool} was stopped (${why})`));
        conn.destroy();
      },
    };
    running.add(entry);
    // Listening from the start — a socket nobody is listening to does not keep what it is sent.
    conn.on("data", (chunk) => {
      buffer += chunk.toString("utf8");
    });
    conn.on("error", (error) =>
      finish(() => reject(`couldn't reach the platform for ${tool}: ${error.message}`)),
    );
    const answered = () =>
      finish(() => {
        let reply: { ok?: boolean; text?: string; value?: unknown };
        try {
          reply = JSON.parse(buffer);
        } catch {
          reject(`${tool} got no answer`);
          return;
        }
        if (reply.ok) resolve(reply);
        else reject(String(reply.text ?? `${tool} failed`));
      });
    conn.on("end", answered);
    conn.on("close", answered);
    conn.write(payload + "\n");
  });
}

const toolFunctions = new Map<string, unknown>();

/** The platform tool called `name`, as a function of one object of arguments. */
function toolFunction(name: string): unknown {
  let made = toolFunctions.get(name);
  if (made) return made;
  const call = async (input: unknown = {}) => {
    if (input === null || typeof input !== "object" || Array.isArray(input)) {
      throw new TypeError(`${name} takes one object of arguments: ${name}({ … })`);
    }
    const moves = name === "Bash";
    if (moves) shellSync?.before();
    try {
      const text = String((await ask(name, { tool: name, input, cwd: process.cwd() })).text ?? "");
      toolTexts.add(text);
      return text;
    } finally {
      if (moves) shellSync?.after();
    }
  };
  Object.defineProperty(call, "name", { value: name });
  // Asked what it is — `Edit.toString()`, the first thing tried by code that does not know —
  // it says how it is called rather than showing the wrapper above.
  call.toString = () =>
    door?.docs?.[name] ??
    toolDocs[name] ??
    `${name}({ … }) — one of your tools, as a function. tools() prints how each is called.`;
  made = call;
  toolFunctions.set(name, made);
  return made;
}

/** What each tool function says about itself, as last told — kept so a function still knows between cells. */
let toolDocs: Record<string, string> = {};

/** Every tool function and how it is called: the list the agent's instructions carry, from inside a cell. */
function toolsList(): string {
  // In the order the agent's instructions list them, which is the order they were offered in —
  // not the alphabetical one a JSON object arrives in.
  const names = [
    ...(door?.names ?? []).filter((name) => name in toolDocs),
    ...Object.keys(toolDocs).filter((name) => !door?.names.includes(name)),
  ];
  const said = names.map((name) => toolDocs[name]);
  const servers = door?.servers ?? [];
  if (servers.length) {
    said.push(
      `MCP servers: ${servers.map((id) => `mcp[${JSON.stringify(id)}]`).join(", ")} — \`await mcp[id].tools()\` lists what one offers, \`await mcp[id].call(name, { … })\` calls one.`,
    );
  }
  const text = said.length
    ? said.join("\n\n")
    : "There are no tool functions here: they exist in a cell of a code-only agent.";
  toolTexts.add(text);
  return text;
}

/** Names a server object is asked for by something other than a script calling a tool. */
const NOT_A_TOOL = new Set([
  "then",
  "toJSON",
  "constructor",
  "prototype",
  "inspect",
  "valueOf",
  "toString",
  "name",
  "length",
]);

const mcpServers = new Map<string, unknown>();

/**
 * One MCP server attached to the agent: `tools()` lists what it offers, `call(name, args)` calls
 * one, and any other name is that call — `mcp.n8n.list_workflows({})`.
 */
function mcpServer(id: string): unknown {
  let made = mcpServers.get(id);
  if (made) return made;
  const call = async (tool: unknown, input: unknown = {}) => {
    if (typeof tool !== "string" || !tool)
      throw new TypeError(`mcp.${id}.call takes the tool's name first: call("name", { … })`);
    if (input === null || typeof input !== "object" || Array.isArray(input)) {
      throw new TypeError(`mcp.${id}.${tool} takes one object of arguments`);
    }
    const answer = await ask(`mcp.${id}.${tool}`, { mcp: id, tool, input });
    if (answer.value !== undefined && answer.value !== null) return answer.value;
    const text = String(answer.text ?? "");
    toolTexts.add(text);
    return text;
  };
  const tools = async () => (await ask(`mcp.${id}.tools`, { mcp: id, list: true })).value ?? [];
  const fixed: Record<string, unknown> = { call, tools };
  made = new Proxy(fixed, {
    get(target, key) {
      if (typeof key !== "string") return undefined;
      if (key in target) return target[key];
      if (NOT_A_TOOL.has(key)) return undefined;
      return (input?: unknown) => call(key, input);
    },
  });
  mcpServers.set(id, made);
  return made;
}

/**
 * Put a cell's tools in reach, or take them away again. The functions are globals, set afresh for
 * every cell that is offered them, so a cell that happened to declare a `Read` of its own does
 * not cost the next one the tool.
 */
function offer(tools: Door | undefined, sync?: { before: () => void; after: () => void }) {
  door = tools;
  shellSync = tools ? sync : undefined;
  toolTexts.clear();
  if (!tools) return;
  toolDocs = tools.docs ?? {};
  for (const name of tools.names) AsyncGlobals[name] = toolFunction(name);
  AsyncGlobals.tools = toolsList;
  AsyncGlobals.ToolError = ToolError;
  AsyncGlobals.mcp = Object.fromEntries(tools.servers.map((id) => [id, mcpServer(id)]));
}

// ───────────────────────────── reading a cell ─────────────────────────────

type Tok = {
  /** `tplc` is a later piece of a template literal already begun — never the start of anything. */
  t: "id" | "punct" | "str" | "tpl" | "tplc" | "re" | "num";
  s: number;
  e: number;
  /** How many brackets were open where it stands. An opener and its closer share one. */
  d: number;
  v: string;
};

const PUNCT = [
  ">>>=",
  "...",
  "===",
  "!==",
  "**=",
  "<<=",
  ">>=",
  ">>>",
  "&&=",
  "||=",
  "??=",
  "=>",
  "==",
  "!=",
  "<=",
  ">=",
  "&&",
  "||",
  "??",
  "?.",
  "++",
  "--",
  "+=",
  "-=",
  "*=",
  "/=",
  "%=",
  "&=",
  "|=",
  "^=",
  "**",
  "<<",
  ">>",
];
const ASSIGNS = new Set([
  "=",
  "+=",
  "-=",
  "*=",
  "/=",
  "%=",
  "**=",
  "<<=",
  ">>=",
  ">>>=",
  "&=",
  "|=",
  "^=",
  "&&=",
  "||=",
  "??=",
  "++",
  "--",
]);
/** Words after which a `/` opens a regular expression rather than dividing. */
const BEFORE_REGEX = new Set([
  "return",
  "typeof",
  "instanceof",
  "in",
  "of",
  "new",
  "delete",
  "void",
  "throw",
  "case",
  "do",
  "else",
  "yield",
  "await",
]);
const isIdStart = (c: string) =>
  (c >= "a" && c <= "z") ||
  (c >= "A" && c <= "Z") ||
  c === "_" ||
  c === "$" ||
  c === "#" ||
  c > "\x7f";
const isIdPart = (c: string) => isIdStart(c) || (c >= "0" && c <= "9");
const isDigit = (c: string) => c >= "0" && c <= "9";

/**
 * Split JavaScript into tokens. Total on any input — an unterminated string runs to the end —
 * because what it reads has already been through a real parser, and where it is wrong anyway the
 * compile check after the rewriting is what catches it.
 */
function tokenize(src: string): Tok[] {
  const toks: Tok[] = [];
  const n = src.length;
  let i = 0;
  let depth = 0;
  /** The depth inside each `${…}` now open, so the `}` that ends one is told from any other. */
  const holes: number[] = [];
  const push = (t: Tok["t"], s: number, e: number, d = depth) =>
    toks.push({ t, s, e, d, v: src.slice(s, e) });

  /** Read template text from `from` to the backtick that ends it or the `${` that interrupts it. */
  const template = (from: number, first: boolean): number => {
    let j = from;
    while (j < n) {
      const c = src[j];
      if (c === "\\") j += 2;
      else if (c === "`") {
        push(first ? "tpl" : "tplc", first ? from - 1 : from, j + 1);
        return j + 1;
      } else if (c === "$" && src[j + 1] === "{") {
        push(first ? "tpl" : "tplc", first ? from - 1 : from, j + 2);
        depth += 1;
        holes.push(depth);
        return j + 2;
      } else j += 1;
    }
    push(first ? "tpl" : "tplc", first ? from - 1 : from, n);
    return n;
  };

  while (i < n) {
    const c = src[i];
    if (c === " " || c === "\t" || c === "\n" || c === "\r") {
      i += 1;
    } else if (c === "/" && src[i + 1] === "/") {
      const end = src.indexOf("\n", i);
      i = end < 0 ? n : end;
    } else if (c === "/" && src[i + 1] === "*") {
      const end = src.indexOf("*/", i + 2);
      i = end < 0 ? n : end + 2;
    } else if (c === '"' || c === "'") {
      let j = i + 1;
      while (j < n && src[j] !== c && src[j] !== "\n") j += src[j] === "\\" ? 2 : 1;
      push("str", i, Math.min(j + 1, n));
      i = Math.min(j + 1, n);
    } else if (c === "`") {
      i = template(i + 1, true);
    } else if (c === "}" && holes.length > 0 && holes[holes.length - 1] === depth) {
      holes.pop();
      depth -= 1;
      i = template(i + 1, false);
    } else if (isDigit(c) || (c === "." && isDigit(src[i + 1] ?? ""))) {
      let j = i + 1;
      while (
        j < n &&
        (isIdPart(src[j]) ||
          src[j] === "." ||
          ((src[j] === "+" || src[j] === "-") &&
            /[eE]/.test(src[j - 1]) &&
            !/^0[xXbBoO]/.test(src.slice(i, i + 2))))
      )
        j += 1;
      push("num", i, j);
      i = j;
    } else if (isIdStart(c)) {
      let j = i + 1;
      while (j < n && isIdPart(src[j])) j += 1;
      push("id", i, j);
      i = j;
    } else if (c === "/") {
      const prev = toks[toks.length - 1];
      const regex =
        !prev ||
        (prev.t === "punct" && ![")", "]", "}", "++", "--"].includes(prev.v)) ||
        (prev.t === "id" && BEFORE_REGEX.has(prev.v));
      if (regex) {
        let j = i + 1;
        let inClass = false;
        while (j < n && src[j] !== "\n") {
          if (src[j] === "\\") j += 2;
          else if (src[j] === "[") {
            inClass = true;
            j += 1;
          } else if (src[j] === "]") {
            inClass = false;
            j += 1;
          } else if (src[j] === "/" && !inClass) break;
          else j += 1;
        }
        j = Math.min(j + 1, n);
        while (j < n && isIdPart(src[j])) j += 1;
        push("re", i, j);
        i = j;
      } else {
        const two = src[i + 1] === "=" ? 2 : 1;
        push("punct", i, i + two);
        i += two;
      }
    } else if (c === "(" || c === "[" || c === "{") {
      push("punct", i, i + 1);
      depth += 1;
      i += 1;
    } else if (c === ")" || c === "]" || c === "}") {
      depth = Math.max(0, depth - 1);
      push("punct", i, i + 1);
      i += 1;
    } else {
      const long = PUNCT.find((op) => src.startsWith(op, i));
      const len = long ? long.length : 1;
      push("punct", i, i + len);
      i += len;
    }
  }
  return toks;
}

/** Statements that are not expressions and declare nothing a later cell needs: left exactly as they are. */
const CONTROL = new Set([
  "if",
  "for",
  "while",
  "do",
  "switch",
  "try",
  "throw",
  "return",
  "break",
  "continue",
  "debugger",
  "with",
]);
/** Words that continue the statement before them even at the start of a line. */
const CONTINUES = new Set(["else", "catch", "finally"]);

type Edit = { s: number; e: number; text: string };

type Rewritten = {
  /** The function body to run. */
  body: string;
  /** For each line of `body`, the line of the cell it came from, as Bun printed it. */
  lines: string[];
};

/**
 * Rewrite one transpiled cell so that what it declares outlives it (see the file header).
 * `echo` is whether top-level expressions are also handed to `__adi.show` — the half left out
 * when the full rewriting does not compile.
 */
function rewrite(js: string, echo: boolean): Rewritten {
  const toks = tokenize(js);
  const edits: Edit[] = [];
  const hoisted: { s: number; e: number; name: string }[] = [];
  const startsLine = (tok: Tok) => tok.s === 0 || js[tok.s - 1] === "\n";

  // Where each top-level statement begins: with no bracket open, a token that starts a line or
  // follows a semicolon, and neither closes something nor continues what came before. The
  // semicolon half matters for the one-line preludes the transpiler puts in front of a cell.
  const starts: number[] = [];
  for (let k = 0; k < toks.length; k += 1) {
    const tok = toks[k];
    const before = toks[k - 1];
    const afterSemicolon =
      before !== undefined && before.d === 0 && before.t === "punct" && before.v === ";";
    if (tok.d !== 0 || tok.t === "tplc" || !(startsLine(tok) || afterSemicolon)) continue;
    if (tok.t === "punct" && (tok.v === "}" || tok.v === ")" || tok.v === "]")) continue;
    if (tok.t === "id" && CONTINUES.has(tok.v)) continue;
    const opened = starts.length ? toks[starts[starts.length - 1]] : undefined;
    if (tok.v === "while" && opened?.v === "do") continue;
    starts.push(k);
  }

  const blank = (s: number, e: number) =>
    edits.push({ s, e, text: js.slice(s, e).replace(/[^\n]/g, "") });

  for (let at = 0; at < starts.length; at += 1) {
    let first = starts[at];
    const end = at + 1 < starts.length ? starts[at + 1] : toks.length;
    const last = end - 1;
    if (last < first) continue;
    const semi = toks[last].t === "punct" && toks[last].v === ";" && toks[last].d === 0;
    const stop = toks[last].e;

    // `export` means nothing in a cell: what it marks is kept, and a bare list of names is dropped.
    if (toks[first].v === "export" && toks[first].t === "id") {
      const next = toks[first + 1];
      if (!next || next.v === "{" || next.v === "*") {
        blank(toks[first].s, stop);
        continue;
      }
      const drop = next.v === "default" ? (toks[first + 2]?.s ?? next.e) : next.s;
      edits.push({ s: toks[first].s, e: drop, text: "" });
      first += next.v === "default" ? 2 : 1;
      if (first > last) continue;
    }

    const head = toks[first];
    const after = toks[first + 1];

    if (head.t === "id" && head.v === "import" && after && after.v !== "(" && after.v !== ".") {
      edits.push({ s: head.s, e: stop, text: importStatement(js.slice(head.s, stop)) });
      continue;
    }

    if (
      head.t === "id" &&
      (head.v === "const" || head.v === "let" || head.v === "var") &&
      after &&
      (after.t === "id" || after.v === "{" || after.v === "[")
    ) {
      if (!semi) continue;
      // Each declarator becomes an assignment of its own. Nothing here adds or removes a line.
      edits.push({ s: head.s, e: after.s, text: "" });
      let k = first + 1;
      while (k < last) {
        const begin = k;
        let patternEnd = k;
        if (toks[k].v === "{" || toks[k].v === "[") {
          patternEnd = k + 1;
          while (
            patternEnd < last &&
            !(
              toks[patternEnd].d === 0 &&
              toks[patternEnd].t === "punct" &&
              (toks[patternEnd].v === "}" || toks[patternEnd].v === "]")
            )
          )
            patternEnd += 1;
        }
        let stopAt = patternEnd + 1;
        while (
          stopAt < last &&
          !(toks[stopAt].d === 0 && toks[stopAt].v === "," && toks[stopAt].t === "punct")
        )
          stopAt += 1;
        const initialized = toks[patternEnd + 1]?.v === "=" && patternEnd + 1 < stopAt;
        const name = toks[begin].v;
        if (initialized) {
          edits.push({ s: toks[begin].s, e: toks[begin].s, text: "(" });
          edits.push({ s: toks[stopAt - 1].e, e: toks[stopAt - 1].e, text: ")" });
        } else if (head.v === "var") {
          // A bare `var x` keeps a value x already has, and makes it exist when it does not.
          edits.push({
            s: toks[begin].s,
            e: toks[begin].e,
            text: `(${JSON.stringify(name)} in globalThis) || (${name} = void 0)`,
          });
        } else {
          edits.push({ s: toks[begin].s, e: toks[begin].e, text: `${name} = void 0` });
        }
        if (stopAt < last) edits.push({ s: toks[stopAt].s, e: toks[stopAt].e, text: ";" });
        k = stopAt + 1;
      }
      continue;
    }

    const fn =
      head.t === "id" && (head.v === "function" || (head.v === "async" && after?.v === "function"));
    if (fn) {
      let k = first + (head.v === "async" ? 2 : 1);
      if (toks[k]?.v === "*") k += 1;
      const name = toks[k];
      if (!name || name.t !== "id") continue;
      // Moved to the top, as a declaration is: a cell may call a function above where it defines it.
      hoisted.push({ s: head.s, e: stop, name: name.v });
      blank(head.s, stop);
      continue;
    }

    if (head.t === "id" && head.v === "class" && after?.t === "id" && after.v !== "extends") {
      edits.push({ s: head.s, e: head.s, text: `${after.v} = ` });
      edits.push({ s: stop, e: stop, text: ";" });
      continue;
    }

    if (head.t === "id" && CONTROL.has(head.v)) continue;
    if (head.t === "punct" && (head.v === "{" || head.v === ";")) continue;
    if (head.t === "id" && after?.v === ":" && after.t === "punct") continue;

    // An expression. A directive would make the whole cell strict, and strict code cannot
    // create the globals this rewriting relies on, so the one statement that is only a string
    // is dropped rather than shown.
    if (first === last - 1 && head.t === "str" && /^["']use strict["']$/.test(head.v)) {
      blank(head.s, stop);
      continue;
    }
    if (!echo || !semi) continue;
    let assigns = false;
    for (let k = first; k < last; k += 1) {
      if (toks[k].d === 0 && toks[k].t === "punct" && ASSIGNS.has(toks[k].v)) assigns = true;
    }
    // `x = 5` and `i++` are done for what they change, not for what they are worth.
    if (assigns) continue;
    // Every expression is offered; `show` prints the last one whatever it is and an earlier one
    // only when it is a command's result. JavaScript returns something from nearly everything —
    // a timer, a length, a promise — and a cell that printed all of it would bury what it meant to say.
    const final = at === starts.length - 1;
    edits.push({ s: head.s, e: head.s, text: "__adi.show(" });
    edits.push({ s: toks[last].s, e: toks[last].s, text: final ? ", true)" : ")" });
  }

  // `import(…)` and `import.meta` mean nothing in code that is not a module, wherever they are.
  const claimed = new Set(edits.filter((edit) => edit.e > edit.s).map((edit) => edit.s));
  for (let k = 0; k < toks.length - 1; k += 1) {
    const tok = toks[k];
    if (tok.t !== "id" || tok.v !== "import" || claimed.has(tok.s)) continue;
    if (toks[k + 1].v === "(") edits.push({ s: tok.s, e: tok.e, text: "__adi.import" });
    else if (toks[k + 1].v === "." && toks[k + 2]?.v === "meta")
      edits.push({ s: tok.s, e: toks[k + 2].e, text: "__adi.meta" });
  }

  /** `js` from `from` to `to` with the edits inside that range made — all of them, or all but the one that covers it. */
  const apply = (from: number, to: number, whole = true): string => {
    const inside = edits
      .filter(
        (edit) => edit.s >= from && edit.e <= to && (whole || !(edit.s === from && edit.e === to)),
      )
      .sort((a, b) => a.s - b.s || a.e - b.e);
    let text = "";
    let at = from;
    for (const edit of inside) {
      if (edit.s < at) continue;
      text += js.slice(at, edit.s) + edit.text;
      at = edit.e;
    }
    return text + js.slice(at, to);
  };

  // A hoisted function keeps the rewriting of whatever is inside it — an `import()` in its body
  // is still an import — so only the edit that blanks it where it stood is left out.
  const top = hoisted.map((fn) => `${fn.name} = ${apply(fn.s, fn.e, false)};`);
  const topLines = hoisted.flatMap((fn) => js.slice(fn.s, fn.e).split("\n"));
  const rest = apply(0, js.length);
  return {
    body: top.length ? top.join("\n") + "\n" + rest : rest,
    lines: [...topLines, ...js.split("\n")],
  };
}

/** One `import … from "…"` statement, as Bun prints it, as the awaited import that does the same. */
function importStatement(text: string): string {
  const bare = /^import\s*(["'`])(.+?)\1\s*(?:with\s*\{[^}]*\})?\s*;?$/.exec(text);
  if (bare) return `await __adi.import(${JSON.stringify(bare[2])});`;
  const from = /^import\s+([\s\S]+?)\s+from\s*(["'`])(.+?)\2\s*(?:with\s*\{[^}]*\})?\s*;?$/.exec(
    text,
  );
  if (!from) return text;
  const [, clause, , spec] = from;
  const source = `await __adi.import(${JSON.stringify(spec)})`;
  const named = /\{([\s\S]*)\}/.exec(clause);
  const star = /\*\s*as\s+([\w$]+)/.exec(clause);
  const lead = clause
    .replace(/\{[\s\S]*\}/, "")
    .replace(/\*\s*as\s+[\w$]+/, "")
    .replace(/,/g, "")
    .trim();
  const picks: string[] = [];
  if (lead) picks.push(`default: ${lead}`);
  for (const item of named ? named[1].split(",") : []) {
    const [name, alias] = item.trim().split(/\s+as\s+/);
    if (name) picks.push(alias ? `${name}: ${alias}` : name);
  }
  if (star) {
    return `${star[1]} = ${source};${lead ? ` ${lead} = ${star[1]}.default;` : ""}`;
  }
  return `({ ${picks.join(", ")} } = ${source});`;
}

// ───────────────────────────── running a cell ─────────────────────────────

/** Thrown by `process.exit()` inside a cell: it ends the cell, not the conversation's interpreter. */
class CellExit extends Error {
  constructor(readonly code: number) {
    super(`process.exit(${code})`);
  }
}

const AsyncGlobals = globalThis as unknown as Record<string, unknown>;

class Cells {
  count = 0;
  /** What each cell's lines were, so a stack frame in a function defined cells ago can quote its line. */
  private sources = new Map<string, string[]>();
  // `deadCodeElimination` off, or a bare `x` on the last line — the commonest thing a cell ends
  // with — is thrown away as having no effect. `target: "node"`, or a cell that calls `require`
  // is given a line redefining it from `import.meta`, which a cell does not have.
  private transpiler = new Bun.Transpiler({
    loader: "ts",
    target: "node",
    deadCodeElimination: false,
    trimUnusedImports: false,
  } as never);

  constructor() {
    Object.assign(AsyncGlobals, { sh, ShError, Sh, share, shared, fs, path, os, $: dollar });
  }

  /**
   * Print what a top-level expression came to: a command's output wherever it stands, so three
   * bare `await sh(…)` lines read like the three commands they are — a tool's answer likewise,
   * for a code-only agent — and any other value only when it is the cell's `last` statement, as
   * a notebook would.
   */
  private show = (value: unknown, last = false) => {
    if (value instanceof Sh) {
      const text = value.shown();
      if (text) console.log(text);
      return;
    }
    // What a tool answered, standing on a line of its own: shown as the tool said it.
    if (typeof value === "string" && toolTexts.has(value)) {
      if (value.trim()) console.log(value.replace(/\n+$/, ""));
      return;
    }
    if (!last || value === undefined) return;
    console.log(Bun.inspect(value, { depth: 4, colors: false }));
  };

  /** `import()` for a cell: resolved from where the conversation stands, not from this file. */
  private import = async (spec: unknown, options?: unknown): Promise<unknown> => {
    let target = String(spec);
    if (!/^(?:node:|bun:|data:|https?:|file:)/.test(target)) {
      try {
        target = Bun.resolveSync(target, process.cwd());
      } catch {
        // Not found from here. Left as written, so a built-in still loads and anything else
        // fails with the runtime's own message.
      }
    }
    if (path.isAbsolute(target) && !target.includes(`${path.sep}node_modules${path.sep}`)) {
      // A file of the project's own: read again whenever it has changed, since changing it and
      // importing it again is exactly what somebody working on it does next.
      try {
        target += `?v=${Math.trunc(fs.statSync(target).mtimeMs)}`;
      } catch {}
    }
    return options === undefined ? import(target) : import(target, options as never);
  };

  /**
   * What `import.meta` is in a cell. There is no file, so it describes where the conversation
   * stands — read when asked, because a cell may well move before it asks.
   */
  private meta(require: unknown) {
    return {
      require,
      main: true,
      get env() {
        return process.env;
      },
      get dir() {
        return process.cwd();
      },
      get dirname() {
        return process.cwd();
      },
      get path() {
        return path.join(process.cwd(), "cell.ts");
      },
      get filename() {
        return path.join(process.cwd(), "cell.ts");
      },
      get url() {
        return pathToFileURL(path.join(process.cwd(), "cell.ts")).href;
      },
    };
  }

  /** Run `code`. Resolves to how it ended; what it printed went to stdout and stderr. */
  async run(code: string): Promise<"ok" | "error"> {
    this.count += 1;
    const name = `cell-${this.count}`;
    let js: string;
    try {
      js = this.transpiler.transformSync(code);
    } catch (error) {
      console.error(syntaxError(error));
      return "error";
    }
    // A cell that names `__dirname` is handed one that describes the string it was transpiled
    // from. What it means is where the conversation stands.
    js = js.replace(
      'var __dirname = "", __filename = "input.ts";',
      'var __dirname = process.cwd(), __filename = process.cwd() + "/cell.ts";',
    );

    // Compiled before anything runs, least ambitious last: a rewriting that is wrong about a
    // cell must cost that cell its echo, or at worst its persistence — never its validity.
    let fn: ((adi: unknown, require: unknown) => Promise<void>) | undefined;
    let failure: unknown;
    let plain = false;
    for (const attempt of ["echo", "keep", "plain"] as const) {
      const { body, lines } =
        attempt === "plain" ? { body: js, lines: js.split("\n") } : rewrite(js, attempt === "echo");
      // For whoever is working on the rewriting itself: what a cell became, on its way to running.
      if (process.env.ADI_TSCODE_DEBUG) console.error(`[${name} ${attempt}]\n${body}\n[/${name}]`);
      try {
        fn = (0, eval)(`(async (__adi, require) => {\n${body}\n})\n//# sourceURL=${name}`);
        this.sources.set(name, lines);
        plain = attempt === "plain";
        break;
      } catch (error) {
        failure ??= error;
      }
    }
    if (!fn) {
      console.error(this.describe(failure));
      return "error";
    }
    if (this.sources.size > 200) this.sources.delete(this.sources.keys().next().value as string);

    const require = createRequire(path.join(process.cwd(), "cell.js"));
    const adi = { show: this.show, import: this.import, meta: this.meta(require) };
    try {
      await fn(adi, require);
    } catch (error) {
      if (error instanceof CellExit) {
        if (error.code === 0) return "ok";
        console.error(`process.exit(${error.code})`);
        return "error";
      }
      console.error(this.describe(error));
      return "error";
    } finally {
      if (plain) {
        console.error(
          "(this cell ran as written: what it declared at the top level is not kept for later calls. Assign to `globalThis.name` to keep a value.)",
        );
      }
    }
    return "ok";
  }

  /** An error as the model should read it: what was thrown, and the lines of its own cells on the way. */
  describe(error: unknown, depth = 0): string {
    if (!(error instanceof Error))
      return `Uncaught ${Bun.inspect(error, { depth: 3, colors: false })}`;
    const out = [`${error.name}: ${error.message}`];
    const frames = String(error.stack ?? "")
      .split("\n")
      .filter((line) => /^\s+at /.test(line));
    let shown = 0;
    for (const frame of frames) {
      if (shown >= 12) break;
      // This file's own frames and the runtime's internals say nothing about the cell.
      if (
        frame.includes(HERE) ||
        /\((?:node|bun|native):|\(native\)| native:\d|at (?:node|bun):/.test(frame)
      )
        continue;
      const cell = /^\s+at (?:(.+?) \()?(cell-\d+):(\d+):\d+\)?$/.exec(frame);
      if (cell) {
        const [, where, name, line] = cell;
        // Line 1 is the wrapper this file put around the cell, which is nobody's code.
        if (Number(line) < 2) continue;
        const text = this.sources.get(name)?.[Number(line) - 2]?.trim();
        const called = where?.replace(/^async /, "");
        const fnName = called && called !== "<anonymous>" ? ` in ${called}` : "";
        out.push(`    at <${name.replace("-", " ")}>${fnName}${text ? `: ${text}` : ""}`);
      } else {
        out.push(frame.replace(/^\s+/, "    "));
      }
      shown += 1;
    }
    if (error instanceof AggregateError) {
      for (const inner of error.errors.slice(0, 5))
        out.push("  - " + this.describe(inner, depth + 1).replace(/\n/g, "\n    "));
    }
    if (error.cause !== undefined && depth < 3)
      out.push("Caused by: " + this.describe(error.cause, depth + 1));
    return out.join("\n");
  }
}

/** What the transpiler said was wrong, with the line it was wrong on. */
function syntaxError(error: unknown): string {
  const all = (error as { errors?: unknown[] })?.errors ?? [error];
  return all
    .slice(0, 5)
    .map((item) => {
      const message = String((item as Error)?.message ?? item);
      const position = (item as { position?: { line?: number; lineText?: string } })?.position;
      if (!position?.line) return `SyntaxError: ${message}`;
      return `SyntaxError: ${message}\n    line ${position.line}: ${(position.lineText ?? "").trim()}`;
    })
    .join("\n");
}

// ───────────────────────────── the conversation's state ─────────────────────────────

const isName = (key: string) => /^[A-Za-z_][A-Za-z0-9_]*$/.test(key);
const quote = (value: string) => `'${value.replace(/'/g, `'\\''`)}'`;

function replaceFile(file: string, text: string) {
  const scratch = `${file}.${process.pid}.next`;
  fs.writeFileSync(scratch, text);
  fs.renameSync(scratch, file);
}

/**
 * The directory and the exported variables a conversation carries — the same two files the
 * `Bash` tool reads and writes (see `shell.rs`), so the conversation has one location and one
 * environment whichever tool moved it.
 */
class Shared {
  /** The file's text as last read or written, and what it said. */
  private text: string | undefined;
  private carried: Record<string, string> = {};

  constructor(
    private cwdFile: string,
    private envFile: string,
    private home: string,
  ) {}

  /**
   * What the conversation has exported. The file is `sh`'s own `export -p` output whenever Bash
   * wrote it last, and the only reader always right about that format is `sh` — so the names are
   * taken from the text, and `sh` is asked to source it and print what each ended up holding.
   */
  private read(base: Record<string, string>): Record<string, string> {
    let text: string;
    try {
      text = fs.readFileSync(this.envFile, "utf8");
    } catch {
      return {};
    }
    if (text === this.text) return { ...this.carried };
    const carried = exportedValues(this.envFile, exportedNames(text), base);
    if (!carried) return {};
    this.text = text;
    this.carried = carried;
    return { ...carried };
  }

  /** Stand where the conversation stands: the launch environment as the caller has it now, what was exported on top, in the recorded directory. */
  load(base: Record<string, string>) {
    const carried = this.read(base);
    for (const key of Object.keys(process.env)) delete process.env[key];
    Object.assign(process.env, base, carried);
    let target = this.home;
    try {
      const recorded = fs.readFileSync(this.cwdFile, "utf8").trim();
      if (recorded && fs.statSync(recorded).isDirectory()) target = recorded;
    } catch {}
    try {
      process.chdir(target);
    } catch {
      process.chdir(this.home);
    }
    process.env.PWD = process.cwd();
  }

  /** Record where the cell left things, for the next call of either tool. */
  save(base: Record<string, string>) {
    try {
      replaceFile(this.cwdFile, fs.realpathSync(process.cwd()) + "\n");
    } catch {}
    const carried: Record<string, string> = {};
    let text = "";
    for (const key of Object.keys(process.env).sort()) {
      const value = process.env[key];
      if (value === undefined || UNCARRIED.has(key) || !isName(key) || base[key] === value)
        continue;
      carried[key] = value;
      text += `export ${key}=${quote(value)}\n`;
    }
    const same =
      Object.keys(carried).length === Object.keys(this.carried).length &&
      Object.keys(carried).every((key) => this.carried[key] === carried[key]);
    // Nothing exported, changed or removed, so the file already says this — possibly in Bash's
    // own spelling. Rewriting it in ours would only cost the next reader a parse.
    if (same && this.text !== undefined) return;
    try {
      replaceFile(this.envFile, text);
      this.text = text;
      this.carried = carried;
    } catch {}
  }
}

// ───────────────────────────── the kernel ─────────────────────────────

const omitted = (bytes: number) =>
  `\n\n[… ${bytes} bytes omitted here. Print less of it — what you computed is still in your variables, so slice or summarize it rather than running this again.]\n\n`;

/** `length` bytes of a file from `from`, cut to `limit` by dropping the middle: the end is where an error is. */
function printed(file: string, from: number, length: number, limit: number): string {
  if (length <= 0) return "";
  const fd = fs.openSync(file, "r");
  try {
    const read = (at: number, size: number) => {
      const buffer = Buffer.alloc(size);
      const got = fs.readSync(fd, buffer, 0, size, at);
      return buffer.subarray(0, got).toString("utf8");
    };
    if (length <= limit) return read(from, length);
    const head = Math.floor((limit * 3) / 5);
    const end = Math.floor((limit * 2) / 5);
    return read(from, head) + omitted(length - head - end) + read(from + length - end, end);
  } finally {
    fs.closeSync(fd);
  }
}

type Request = {
  op?: string;
  code?: string;
  timeout_ms?: number;
  max_output?: number;
  env?: Record<string, string>;
  tools?: Door;
};

/** One connection: a caller, the request it sent, and whether it is still there to be answered. */
type Call = {
  conn: net.Socket;
  /** The caller hung up — a cancelled turn takes its tool server with it. */
  gone: boolean;
  /** What to do when it does; replaced as the call moves from waiting to running. */
  onGone: () => void;
  /** The request, or nothing if the caller left before sending one. */
  line: Promise<string | undefined>;
};

class Kernel {
  private cells = new Cells();
  private shared: Shared;
  private dir: string;
  private socket: string;
  private last = Date.now();
  private busy = false;
  private waiting: Call[] = [];
  private exit = process.exit.bind(process);

  constructor(private args: Record<string, string>) {
    sharedPrefix = args.shared;
    this.shared = new Shared(args["shell-cwd"], args["shell-env"], args.home);
    // A socket path has ~100 bytes to live in, and a session directory is often longer than that
    // on its own — so the socket lives in a private temp directory (mkdtemp makes it 0700, which
    // is the whole of the access control) and the state file says where.
    let dir = fs.mkdtempSync(path.join(os.tmpdir(), "adi-ts-"));
    if (Buffer.byteLength(dir) > 80) {
      fs.rmdirSync(dir);
      dir = fs.mkdtempSync("/tmp/adi-ts-");
    }
    this.dir = dir;
    this.socket = path.join(dir, "k.sock");
  }

  serve() {
    // `process.exit()` in a cell ends the cell. In a callback that outlives its cell there is
    // nothing to end, and the throw is reported like any other error from one.
    (process as { exit: unknown }).exit = (code?: number) => {
      throw new CellExit(Number(code ?? process.exitCode ?? 0));
    };
    // A callback or a promise nobody awaited, failing after its cell has returned. By default
    // that ends the process, and with it every variable the conversation holds.
    const stray = (kind: string) => (error: unknown) => {
      if (error instanceof CellExit) return;
      console.error(`${kind}: ${this.cells.describe(error)}`);
    };
    process.on("uncaughtException", stray("Uncaught, in a callback"));
    process.on("unhandledRejection", stray("Unhandled rejection"));
    process.on("SIGTERM", () => this.leave("terminated"));
    // The deadline is this process's own business; a stray interrupt must not end the conversation.
    process.on("SIGINT", () => {});

    // Half-open, so that the caller going away is an event of its own rather than the end of our
    // side too: it is how a cancelled turn is noticed while its cell is still running.
    const server = net.createServer({ allowHalfOpen: true }, (conn) => this.accept(conn));
    server.listen(this.socket, () => {
      fs.chmodSync(this.socket, 0o600);
      this.announce({
        pid: process.pid,
        socket: this.socket,
        protocol: Number(this.args.protocol ?? 0),
        runtime: `bun ${Bun.version}`,
      });
    });
    const idle = Number(this.args.idle) * 1000;
    setInterval(() => {
      if (!this.mine()) this.leave(undefined);
      if (!this.busy && Date.now() - this.last > idle) this.leave("idle");
    }, 2000);
  }

  private announce(record: unknown) {
    replaceFile(this.args.state, JSON.stringify(record) + "\n");
  }

  /** Whether the state file still names this process. Gone means the session was deleted; another pid means a newer kernel took over. */
  private mine(): boolean {
    try {
      return JSON.parse(fs.readFileSync(this.args.state, "utf8")).pid === process.pid;
    } catch {
      return false;
    }
  }

  private leave(why: string | undefined): never {
    if (why !== undefined && this.mine()) {
      // A tombstone, so the next call can say the interpreter it finds is a new one.
      try {
        this.announce({ ended: why });
      } catch {}
    }
    try {
      fs.unlinkSync(this.socket);
    } catch {}
    try {
      fs.rmdirSync(this.dir);
    } catch {}
    if (stateDir) {
      try {
        fs.rmSync(stateDir, { recursive: true, force: true });
      } catch {}
    }
    this.exit(0);
  }

  /**
   * Take a connection. Its request is read from the moment it arrives, whatever the interpreter
   * is doing — a socket nobody is listening to does not keep what it is sent, and a call that
   * came in during another cell must still be there when that cell ends.
   */
  private accept(conn: net.Socket) {
    const call: Call = { conn, gone: false, onGone: () => {}, line: Promise.resolve(undefined) };
    conn.on("error", () => {});
    const closed = () => {
      call.gone = true;
      call.onGone();
    };
    conn.on("end", closed);
    conn.on("close", closed);
    call.line = new Promise<string | undefined>((resolve) => {
      let buffer = "";
      conn.on("data", (chunk) => {
        buffer += chunk.toString("utf8");
        const end = buffer.indexOf("\n");
        if (end >= 0) resolve(buffer.slice(0, end));
      });
      call.onGone = () => resolve(undefined);
    });
    this.waiting.push(call);
    this.next();
  }

  /** One cell at a time: a call that arrives during another waits its turn. */
  private next() {
    if (this.busy) return;
    const call = this.waiting.shift();
    if (!call) return;
    this.busy = true;
    void this.answer(call)
      .catch((error) => console.error(`tscode: ${this.cells.describe(error)}`))
      .finally(() => {
        this.busy = false;
        this.last = Date.now();
        this.next();
      });
  }

  private async answer(call: Call) {
    const { conn } = call;
    const line = await call.line;
    if (line === undefined) return;
    let request: Request;
    try {
      request = JSON.parse(line);
    } catch {
      conn.destroy();
      return;
    }
    if (request.op === "ping") {
      conn.end(JSON.stringify({ pid: process.pid }) + "\n");
      return;
    }

    const base = request.env ?? {};
    const limit = Number(request.max_output ?? 32_000);
    const began = Date.now();
    this.shared.load(base);

    // This process's stdout and stderr are one file, opened by whoever started it, so what a
    // child process writes is caught along with every `console.log`. What is in it before the
    // cell starts was printed since the last call, by something the conversation left running.
    const capture = this.args.capture;
    const before = fs.statSync(capture).size;

    // Whoever asked has already gone — a turn cancelled while this call waited its turn. Running
    // JavaScript cannot be stopped once started, so the kindest thing to do with a cell nobody
    // will read the result of is not to start it.
    if (call.gone) return;

    let stoppedBy: string | undefined;
    const stop = (why: string) => {
      if (stoppedBy) return;
      stoppedBy = why;
      stopping = why === "timeout" ? "the call's deadline passed" : "the call was cancelled";
      for (const command of [...running]) command.stop(stopping);
    };
    const timers: ReturnType<typeof setTimeout>[] = [];
    const stuck = new Promise<"stuck">((resolve) => {
      const giveUp = (why: string) => {
        stop(why);
        timers.push(setTimeout(() => resolve("stuck"), STOP_GRACE_MS));
      };
      if (request.timeout_ms) timers.push(setTimeout(() => giveUp("timeout"), request.timeout_ms));
      call.onGone = () => giveUp("abandoned");
    });

    // A code-only agent's cell is told where its tools are answered. For as long as it runs.
    offer(request.tools, {
      before: () => this.shared.save(base),
      after: () => this.shared.load(base),
    });
    const outcome = await Promise.race([this.cells.run(request.code ?? ""), stuck]);
    offer(undefined);
    // Cleared with the cell: a deadline left armed would fire into whichever cell came next.
    call.onGone = () => {};
    for (const timer of timers) clearTimeout(timer);
    stopping = undefined;

    const size = fs.statSync(capture).size;
    let output = printed(capture, before, size - before, limit);
    if (before > 0) {
      output = `[printed since your last call]\n${printed(capture, 0, before, Math.floor(limit / 4)).replace(/\n*$/, "\n")}[this call]\n${output}`;
    }
    const reply = {
      status:
        outcome === "stuck" ? "stuck" : stoppedBy && outcome === "error" ? "interrupted" : outcome,
      stopped_by: stoppedBy ?? null,
      output,
      elapsed_ms: Date.now() - began,
      cell: this.cells.count,
    };

    if (outcome === "stuck") {
      // The cell is still running and cannot be stopped from here, so the process that holds it
      // goes — and says so first, so the caller hears why rather than a closed connection.
      await new Promise<void>((resolve) => {
        if (call.gone) resolve();
        else conn.end(JSON.stringify(reply) + "\n", () => resolve());
        setTimeout(resolve, 1000);
      });
      this.leave("stuck");
    }

    this.shared.save(base);
    try {
      fs.truncateSync(capture, 0);
    } catch {}
    if (!call.gone) conn.end(JSON.stringify(reply) + "\n");
  }
}

function options(argv: string[]): Record<string, string> {
  const found: Record<string, string> = {};
  for (let i = 0; i + 1 < argv.length; i += 2) {
    if (argv[i].startsWith("--")) found[argv[i].slice(2)] = argv[i + 1];
  }
  return found;
}

const [mode, ...rest] = process.argv.slice(2);
if (mode === "kernel") {
  new Kernel(options(rest)).serve();
} else if (mode === "run" && rest[0]) {
  const given = options(rest.slice(1));
  sharedPrefix = given.shared;
  const cells = new Cells();
  // A code-only agent's tools are functions only while a cell runs. Here they are names that
  // say so when called, which is more use than a `ReferenceError`.
  if (given.absent) {
    for (const name of given.absent.split(",")) AsyncGlobals[name] = toolFunction(name);
    AsyncGlobals.ToolError = ToolError;
  }
  process.on("unhandledRejection", (error) => {
    console.error(`Unhandled rejection: ${cells.describe(error)}`);
    process.exitCode = 1;
  });
  if ((await cells.run(fs.readFileSync(rest[0], "utf8"))) !== "ok") process.exitCode = 1;
} else {
  console.error(
    "usage: tscode.ts kernel --state F --capture F --shell-cwd F --shell-env F --shared PREFIX --home DIR --idle S --protocol N\n       tscode.ts run SCRIPT [--shared PREFIX]",
  );
  process.exitCode = 2;
}
