import css from "./elements.css";

const STYLES = new CSSStyleSheet();
STYLES.replaceSync(css);

export function define<T extends CustomElementConstructor>(tag: string, cls: T): T {
  if (!customElements.get(tag)) customElements.define(tag, cls);
  return cls;
}

const ENTITIES: Record<string, string> = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" };

export function esc(value: unknown): string {
  return String(value ?? "").replace(/[&<>"']/g, (c) => ENTITIES[c] ?? c);
}

// Parser-created children can arrive after connectedCallback.
export function watchChildren(element: Node, handle: () => void): void {
  handle();
  new MutationObserver(handle).observe(element, {
    childList: true,
    characterData: true,
    subtree: true,
  });
}

export function detail<T>(event: Event): T {
  return (event as CustomEvent<T>).detail;
}

export class AdiElement extends HTMLElement {
  declare readonly shadowRoot: ShadowRoot;

  #built = false;

  constructor() {
    super();
    this.attachShadow({ mode: "open" });
  }

  connectedCallback() {
    if (!this.#built) {
      this.shadowRoot.adoptedStyleSheets = [STYLES];
      this.shadowRoot.innerHTML = this.template();
      this.#built = true;
      this.setup();
    }
    this.update();
  }

  attributeChangedCallback(_name?: string, _old?: string | null, _value?: string | null): void {
    if (this.#built) this.update();
  }

  template(): string {
    return "<slot></slot>";
  }

  setup(): void {}

  update(): void {}

  $<E extends Element = HTMLElement>(selector: string): E | null {
    return this.shadowRoot.querySelector<E>(selector);
  }

  /** A required template node; call after the first build. */
  must<E extends Element = HTMLElement>(selector: string): E {
    const node = this.shadowRoot.querySelector<E>(selector);
    if (!node) throw new Error(`<${this.localName}> has no ${selector}`);
    return node;
  }

  attr(name: string, fallback = ""): string {
    const value = this.getAttribute(name);
    return value === null ? fallback : value;
  }

  pick<T extends string>(name: string, allowed: readonly T[]): T {
    const value = this.getAttribute(name);
    return allowed.find((a) => a === value) ?? (allowed[0] as T);
  }

  emit(type: string, detail?: unknown): boolean {
    return this.dispatchEvent(new CustomEvent(type, { detail, bubbles: true, composed: true }));
  }
}
