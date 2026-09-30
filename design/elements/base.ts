// The base every adi element is built on, and the three decisions each one inherits from it.
//
// **A shadow root.** An element's CSS cannot leak onto the page and the page's cannot reach in,
// so a button looks the same dropped into the Leptos panel, a server-rendered page, or a bare
// HTML file. What *does* reach in is `design/tokens.css`: custom properties inherit through the
// shadow boundary. That is why every rule in this directory is written as `var(--…)` and why no
// element carries a colour, a size or a radius of its own (DESIGN.md §8: never restate a hex).
//
// **One stylesheet per class, not per instance.** `sheet()` builds a constructable stylesheet
// once and every instance of that class adopts the same object, so a table of eighty buttons
// parses one stylesheet between them rather than eighty copies.
//
// **Build once, update often.** `template()` runs on the first connect and never again;
// `update()` runs on every attribute change after it. Rebuilding a shadow root wholesale on an
// attribute change is what loses focus, selection and scroll position in anything that contains
// a real `<input>` — see `field.js`, where that is the whole design.
//
// TypeScript, served as JavaScript: `scripts/elements.sh build` strips the types file by file, so
// every `./base.js` import across this directory resolves to what this file becomes.

/** Compile CSS once into a stylesheet every instance of a class can adopt. */
export function sheet(css: string): CSSStyleSheet {
  const compiled = new CSSStyleSheet();
  compiled.replaceSync(css);
  return compiled;
}

/**
 * Register a tag, unless something already has.
 *
 * `customElements.define` throws on a repeat, and one module graph reaching the browser twice
 * under two URLs (`./button.js` and `/elements/button.js` are two modules) is a thing that
 * happens. A second registration should be a no-op, not a page that dies on load.
 */
export function define<T extends CustomElementConstructor>(tag: string, cls: T): T {
  if (!customElements.get(tag)) customElements.define(tag, cls);
  return cls;
}

const ENTITIES: Record<string, string> = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" };

/** Text, safe to interpolate into one of the template strings below. */
export function esc(value: unknown): string {
  return String(value ?? "").replace(/[&<>"']/g, (c) => ENTITIES[c] ?? c);
}

// Shared by every element: the box model, and the type the design system starts from (§4).
// Deliberately thin — anything beyond this belongs to the component that needs it.
const BASE = sheet(`
  *, *::before, *::after { box-sizing: border-box; }
  :host {
    font-family: var(--sans);
    font-size: var(--fs-ui-sm);
    line-height: 1.5;
    font-feature-settings: "tnum";
    color: var(--ink);
  }
  :host([hidden]) { display: none !important; }
  button { font: inherit; color: inherit; background: none; border: 0; padding: 0; cursor: pointer; }
  a { color: inherit; text-decoration: none; }
  .mono { font-family: var(--mono); font-size: var(--fs-mono); color: var(--code); }
  /* Ink, never orange, and only under a keyboard (DESIGN.md §8). */
  :focus-visible { outline: 1.5px solid var(--focus); outline-offset: 2px; }
`);

/**
 * Call `handle()` with an element's own light-DOM content, now and whenever it changes.
 *
 * `connectedCallback` runs *before* the children exist whenever an element was written by the
 * HTML parser or by `innerHTML`: the element is upgraded the moment it is inserted, and what is
 * inside it arrives after. So anything that reads its own light DOM — `<adi-select>` taking its
 * `<option>`s, `<adi-textarea>` taking its default text — has to be told about them rather than
 * looking once and finding nothing. (Slotted content does not need this: a slot is live.)
 */
export function watchChildren(element: Node, handle: () => void): void {
  handle();
  new MutationObserver(handle).observe(element, {
    childList: true,
    characterData: true,
    subtree: true,
  });
}

/**
 * A custom event's `detail`, as the listener knows it was sent.
 *
 * The DOM types every listener's argument as a plain `Event`, so what an element's event carries
 * is a contract between two files the checker cannot see across. This is where that is said —
 * once, by name — rather than as a cast in every listener.
 */
export function detail<T>(event: Event): T {
  return (event as CustomEvent<T>).detail;
}

/** The base class: a shadow root, the shared sheet plus the class's own, and the two phases. */
export class AdiElement extends HTMLElement {
  /** The class's own stylesheet, adopted after the shared one. Build it with `sheet()`. */
  static sheet: CSSStyleSheet | null = null;

  /** Attached in the constructor, so never the `null` the DOM types allow for. */
  declare readonly shadowRoot: ShadowRoot;

  #built = false;

  constructor() {
    super();
    this.attachShadow({ mode: "open" });
  }

  connectedCallback() {
    if (!this.#built) {
      const own = (this.constructor as typeof AdiElement).sheet;
      this.shadowRoot.adoptedStyleSheets = own ? [BASE, own] : [BASE];
      this.shadowRoot.innerHTML = this.template();
      this.#built = true;
      this.setup();
    }
    this.update();
  }

  attributeChangedCallback(_name?: string, _old?: string | null, _value?: string | null): void {
    // The parser sets attributes before the element is connected as often as after, and
    // `connectedCallback` ends with the same `update()` — so anything arriving early is read
    // there rather than lost here.
    if (this.#built) this.update();
  }

  /** The shadow root's markup, built once. Slots for anything the caller supplies. */
  template(): string {
    return "<slot></slot>";
  }

  /** Called after the first build: wire listeners to what `template()` just created. */
  setup(): void {}

  /** Called after every build and every attribute change: reflect the element's state. */
  update(): void {}

  /** One node from this element's shadow root, or `null` — before the first connect, always. */
  $<E extends Element = HTMLElement>(selector: string): E | null {
    return this.shadowRoot.querySelector<E>(selector);
  }

  /**
   * One node from this element's shadow root that `template()` always creates.
   *
   * For code that runs after the build (`setup()`, `update()`, and everything they start), where a
   * missing node is a typo in a selector rather than a state to handle — so it throws, naming both,
   * instead of handing back a `null` that fails three calls later.
   */
  must<E extends Element = HTMLElement>(selector: string): E {
    const node = this.shadowRoot.querySelector<E>(selector);
    if (!node) throw new Error(`<${this.localName}> has no ${selector}`);
    return node;
  }

  /** An attribute's value, or `fallback` when it is absent. */
  attr(name: string, fallback = ""): string {
    const value = this.getAttribute(name);
    return value === null ? fallback : value;
  }

  /**
   * An attribute constrained to a set of values, defaulting to the first.
   *
   * The design system is a closed list of variants, not a free-form style prop; a typo should
   * land on the default rather than render an element with no styling at all.
   */
  pick<T extends string>(name: string, allowed: readonly T[]): T {
    const value = this.getAttribute(name);
    return allowed.find((a) => a === value) ?? (allowed[0] as T);
  }

  /**
   * Fire an event from the element itself.
   *
   * `composed`, because an event raised inside a shadow root does not cross the boundary
   * otherwise — and a listener bound on `<adi-segmented>` is the whole API.
   */
  emit(type: string, detail?: unknown): boolean {
    return this.dispatchEvent(new CustomEvent(type, { detail, bubbles: true, composed: true }));
  }
}
