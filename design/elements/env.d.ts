// Declarations the checker needs and the browser never sees: a `.d.ts` has no runtime, and the
// build skips it.

/** An element's stylesheet. The build inlines it as its text (`--loader:.css=text`), so `sheet()`
 *  compiles it in the same module that imports it — no request, no flash of an unstyled element. */
declare module "*.css" {
  const css: string;
  export default css;
}
