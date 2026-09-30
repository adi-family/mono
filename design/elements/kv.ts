import { AdiElement, define } from "./base.js";

class AdiKv extends AdiElement {
}

define("adi-kv", AdiKv);

export { AdiKv };

declare global {
  interface HTMLElementTagNameMap {
    "adi-kv": AdiKv;
  }
}
